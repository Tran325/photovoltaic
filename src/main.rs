// use axum::{Router, Server};
use std::sync::Arc;
use std::time::Duration;
use log::{info, warn, error, debug};
use std::time::Instant;
use clap::Parser;
use rand::seq::SliceRandom;
use rand::Rng;
use regex::Regex;
mod telegram;

// Solana imports
use solana_sdk::{
    pubkey::Pubkey,
    transaction::Transaction,
};
use std::str::FromStr;
use spl_associated_token_account::get_associated_token_address;

// Core modules
mod config;
mod jupiter_api;
mod solana;
mod arbitrage;
mod token_rotation;
mod rate_limiter;
mod utils;
mod flash_loan;
mod jito;

use crate::jupiter_api::JupiterClient;
use crate::solana::SolanaClient;
use crate::arbitrage::{ArbitrageScanner, ArbitrageExecutor, AtaCache, init_ata_cache, add_to_ata_cache, get_ata_cache};
use crate::token_rotation::TokenRotator;
use crate::rate_limiter::RateLimiter;
use crate::telegram::TelegramNotifier;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::collections::HashMap;
use std::clone::Clone;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

// Define static counters for trades and profit
static TOTAL_TRADES_COUNTER: AtomicU64 = AtomicU64::new(0);
static TOTAL_PROFIT_COUNTER: AtomicI64 = AtomicI64::new(0);

// Global flag to signal cache saving thread to exit
static SHUTDOWN_SIGNAL: AtomicBool = AtomicBool::new(false);

// CLI arguments
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Trade size in SOL
    #[arg(short, long, value_name = "SOL")]
    size: Option<f64>,
    
    /// Minimum profit threshold percentage
    #[arg(short, long, value_name = "PERCENT")]
    profit: Option<f64>,
    
    /// Token rotation interval in minutes
    #[arg(short, long, value_name = "MINUTES")]
    interval: Option<u64>,
    
    /// Number of worker threads
    #[arg(short, long, value_name = "THREADS")]
    threads: Option<u64>,

    /// Create address lookup table with all token accounts
    #[arg(long)]
    account: bool,
    
    /// Validate quote manufacturing against real quotes
    #[arg(long)]
    validate_quotes: bool,
}

// Constants - use the same as in arbitrage.rs
const ATA_CACHE_FILE: &str = "ata_cache.json";

// Import additional modules for parallelization
use tokio::task;
use crate::flash_loan::FlashLoanContext;

// Entry point
#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // Initialize logger
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));
    
    // Parse command line arguments
    let args = Args::parse();
    
    // Load environment variables with improved method
    utils::load_env_file()?;
    info!("Environment variables loaded successfully");
    
    // Initialize bot configuration first so it's available for the account command
    let mut config = config::load_config();
    
    // Check if we should create the address lookup table
    if args.account {
        // Initialize solana client with the loaded config
        let solana_client = Arc::new(SolanaClient::new(&config.default_rpc, &config.private_key)?);
        
        // Create address lookup table and exit
        match create_token_account_lookup_table(&solana_client).await {
            Ok(address) => {
                info!("Successfully created address lookup table: {}", address);
                return Ok(());
            },
            Err(e) => {
                error!("Failed to create address lookup table: {}", e);
                return Err(anyhow::anyhow!("Failed to create address lookup table: {}", e));
            }
        }
    }
    
    // Check if we should validate quote manufacturing
    if args.validate_quotes {
        // Initialize clients for validation
        let solana_client = Arc::new(SolanaClient::new(&config.default_rpc, &config.private_key)?);
        let jupiter_client = Arc::new(JupiterClient::new(config.jupiter_api_level as u32));
        
        // Use tokens that are likely to have good liquidity for validation
        info!("Validating quote manufacturing with SOL and popular tokens...");
        
        // Get predefined tokens for validation
        let sol_token = JupiterClient::get_wsol_token();
        
        // Test with a few popular tokens
        let test_tokens = vec![
            // USDC from predefined function
            JupiterClient::get_usdc_token(),
            
            // BONK token - define manually with only needed fields
            jupiter_api::Token {
                address: "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string(),
                symbol: "BONK".to_string(),
                name: "Bonk".to_string(),
                decimals: 5,
                logo_uri: "".to_string(),
                tags: vec![],
                daily_volume: None,
            },
        ];
        
        // Create arbitrage scanner for validation
        let arbitrage_scanner = ArbitrageScanner::new(
            jupiter_client.clone(),
            Arc::new(config.clone()),
            sol_token.clone(),
            test_tokens[0].clone(),
        );
        
        // Run validation for each token
        for token in test_tokens {
            info!("Validating quote manufacturing for SOL <-> {}", token.symbol);
            if let Err(e) = arbitrage_scanner.validate_quote_manufacturing(&sol_token, &token).await {
                error!("Failed to validate quote manufacturing for {}: {}", token.symbol, e);
            }
        }
        
        info!("Quote manufacturing validation complete");
        return Ok(());
    }
    
    // Initialize Telegram notifier
    let telegram_notifier = TelegramNotifier::new();
    
    // Override config with command line arguments if provided
    if let Some(size) = args.size {
        info!("Setting trade size from CLI: {} SOL", size);
        config.trade_size_sol = size;
    }
    
    if let Some(profit) = args.profit {
        info!("Setting profit threshold from CLI: {}%", profit);
        config.min_profit_threshold = profit;
    }
    
    if let Some(interval) = args.interval {
        info!("Setting token rotation interval from CLI: {} minutes", interval);
        config.token_rotation_interval_minutes = interval;
    }
    
    if let Some(threads) = args.threads {
        info!("Setting thread count from CLI: {}", threads);
        config.thread_amount = threads;
        
        // Recalculate API interval based on new thread count
        config.calculate_api_interval();
    }
    
    // Set Tokio worker threads based on configuration
    if let Ok(var) = std::env::var("TOKIO_WORKER_THREADS") {
        info!("Using TOKIO_WORKER_THREADS from environment: {}", var);
    } else {
        // Set the number of worker threads for Tokio runtime
        std::env::set_var("TOKIO_WORKER_THREADS", config.thread_amount.to_string());
        info!("Setting TOKIO_WORKER_THREADS to {}", config.thread_amount);
    }
    
    // Log configuration settings
    info!("Configuration loaded:");
    info!("  Token Rotation Interval: {} minutes", config.token_rotation_interval_minutes);
    info!("  Jupiter API Level: {}", config.jupiter_api_level);
    info!("  Machine Amount: {}", config.machine_amount);
    info!("  Thread Amount: {}", config.thread_amount);
    info!("  API Interval: {}ms", config.api_interval_ms);
    info!("  Using CPI Swap: {}", config.is_use_swap_cpi);
    info!("  Using Token Rotation: {}", config.is_use_token_rotation);
    info!("  Using Flash Loan: {}", config.use_flash_loan);
    info!("  Using Jito Bundle: {}", config.use_jito_bundle);
    if config.use_jito_bundle {
        info!("  Jito Tip Amount: {} lamports ({} SOL)", 
            config.jito_tip_lamports,
            config.jito_tip_lamports as f64 / 1_000_000_000.0);
    }

    // Send startup notification via Telegram
    if let Err(e) = telegram_notifier.send_message(&format!(
        "🚀 <b>Jupiter Bot Started</b>\n\n\
        💰 Trade Size: {} SOL\n\
        📊 Min Profit: {}%\n\
        🔄 Token Rotation: {} minutes\n\
        🧵 Threads: {}\n\
        🔌 RPC: {}\n\
        🎯 Jito Bundle: {}", 
        config.trade_size_sol,
        config.min_profit_threshold,
        config.token_rotation_interval_minutes,
        config.thread_amount,
        config.default_rpc.split("//").last().unwrap_or("default"),
        if config.use_jito_bundle { "Enabled" } else { "Disabled" }
    )).await {
        warn!("Failed to send Telegram startup notification: {}", e);
    } else {
        info!("Sent startup notification via Telegram");
    }

    let config = Arc::new(config);

    // Create temp directory
    utils::create_temp_dir()?;

    // Start the bot
    info!("Starting Jupiter Arbitrage Bot... {}", config.default_rpc);

    // Initialize clients
    let solana_client = Arc::new(SolanaClient::new(&config.default_rpc, &config.private_key)?);

    // Initialize the ATA cache - either load from file or build from scratch
    initialize_ata_cache(&solana_client).await?;

    // Initialize rate limiter with the calculated API interval
    let rate_limiter = Arc::new(RateLimiter::new(0, 3000)); // Set to 0ms interval and 3000 RPM (50 RPS)

    let jupiter_client = Arc::new({
        let client = JupiterClient::new(3)
            .with_solana_client(solana_client.clone())
            .with_rate_limiter(rate_limiter.clone())
            .with_cache_file_path(config.cache_file_path.clone());
        
        // Set the rate limit to maximum level for paid API with 50 req/sec
        client.set_rate_limit(4); // Level 4 = 500 RPS
        
        // Set cache save interval to 5 minutes (300 seconds)
        client.set_cache_save_interval(300);
        
        client
    });
    
    // Initialize FlashLoanContext with caching
    if config.use_flash_loan {
        if let Err(e) = initialize_flash_loan_context(&solana_client, &config).await {
            warn!("Failed to initialize FlashLoanContext: {}", e);
        }
    }
    
    // Prewarm connections to Jupiter API endpoints for faster initial requests
    let prewarm_future = jupiter_client.prewarm_connections();
    
    // Fetch initial market data in parallel while connections are being prewarmed
    let market_data_future = fetch_market_data_parallel(&jupiter_client, &solana_client);
    
    // Execute both operations in parallel
    let (prewarm_result, market_data_result) = tokio::join!(prewarm_future, market_data_future);
    
    if let Err(e) = prewarm_result {
        warn!("Failed to prewarm Jupiter API connections: {}", e);
        
        // Send error notification via Telegram
        if let Err(notify_err) = telegram_notifier.send_error_notification(
            &format!("Failed to prewarm Jupiter API connections: {}", e)
        ).await {
            warn!("Failed to send Telegram error notification: {}", notify_err);
        }
    } else {
        info!("Successfully prewarmed Jupiter API connections");
    }
    
    if let Err(e) = market_data_result {
        warn!("Failed to fetch market data in parallel: {}", e);
    }
    
    // Attach the jupiter client to the solana client for integrated access
    let solana_client_with_jupiter = {
        let client = solana_client.as_ref().clone().with_jupiter_client(jupiter_client.clone());
        Arc::new(client)
    };
    let solana_client = solana_client_with_jupiter;
    
    // Initialize token rotation if enabled
    let token_rotator = if config.is_use_token_rotation {
        let rotator = Arc::new(TokenRotator::new(
            jupiter_client.clone(),
            config.token_rotation_interval_minutes,
            config.current_machine_index,
            config.machine_amount
        ));
        
        // Initialize token rotation
        match rotator.initialize().await {
            Ok(_) => {
                // Initialize thread-specific tokens
                match rotator.initialize_thread_tokens(config.thread_amount as usize) {
                    Ok(_) => {
                        Some(rotator)
                    },
                    Err(e) => {
                        let error_msg = format!("Failed to initialize thread tokens: {}", e);
                        error!("{}", error_msg);
                        
                        // Send error notification via Telegram
                        if let Err(notify_err) = telegram_notifier.send_error_notification(&error_msg).await {
                            warn!("Failed to send Telegram error notification: {}", notify_err);
                        }
                        
                        None
                    }
                }
            },
            Err(e) => {
                let error_msg = format!("Failed to initialize token rotation: {}", e);
                error!("{}", error_msg);
                
                // Send error notification via Telegram
                if let Err(notify_err) = telegram_notifier.send_error_notification(&error_msg).await {
                    warn!("Failed to send Telegram error notification: {}", notify_err);
                }
                
                None
            }
        }
    } else {
        info!("Token rotation is disabled. Using dynamic token selection for each request.");
        None
    };
    
    // Get base token (SOL)
    let token_a = JupiterClient::get_wsol_token();
    
    // Create a vector to store thread handles
    let mut thread_handles = Vec::new();
    
    // Get a list of tokens to use when rotation is disabled
    let trending_tokens = if !config.is_use_token_rotation {
        match jupiter_client.fetch_trending_tokens().await {
            Ok(tokens) => {
                let filtered_tokens: Vec<_> = tokens
                    .into_iter()
                    .filter(|token| {
                        // Exclude WSOL and tokens with low volume
                        token.address != "So11111111111111111111111111111111111111112" && 
                        token.daily_volume.unwrap_or(0.0) > 10000.0
                    })
                    .collect();
                
                info!("Using {} trending tokens for dynamic selection", filtered_tokens.len());
                
                // Format a message for Telegram notification
                let mut token_list = String::new();
                for (i, token) in filtered_tokens.iter().enumerate().take(10) {
                    info!("Token #{}: {} ({}), Volume: ${:.2}", 
                        i+1, 
                        token.symbol, 
                        token.address, 
                        token.daily_volume.unwrap_or(0.0));
                    
                    token_list.push_str(&format!("{}. {} - ${:.2}M\n", 
                        i+1, 
                        token.symbol, 
                        token.daily_volume.unwrap_or(0.0) / 1_000_000.0));
                }
                
                if filtered_tokens.len() > 10 {
                    info!("... and {} more tokens", filtered_tokens.len() - 10);
                    token_list.push_str(&format!("... and {} more tokens\n", filtered_tokens.len() - 10));
                }
                
                // Send token list via Telegram
                if let Err(e) = telegram_notifier.send_message(&format!(
                    "📊 <b>Selected Trading Tokens</b>\n\n{}",
                    token_list
                )).await {
                    warn!("Failed to send token list via Telegram: {}", e);
                }
                
                // Create a shared Arc for all threads to access
                Arc::new(filtered_tokens)
            },
            Err(e) => {
                let error_msg = format!("Failed to fetch tokens: {}", e);
                error!("{}", error_msg);
                
                // Send error notification via Telegram
                if let Err(notify_err) = telegram_notifier.send_error_notification(&error_msg).await {
                    warn!("Failed to send Telegram error notification: {}", notify_err);
                }
                
                // Fallback to USDC if we can't fetch tokens
                Arc::new(vec![JupiterClient::get_usdc_token()])
            }
        }
    } else {
        Arc::new(Vec::new()) // Not used when rotation is enabled
    };
    
    // Start worker threads
    for thread_id in 0..config.thread_amount as usize {
        // Clone shared resources for this thread
        let jupiter_client_clone = jupiter_client.clone();
        let solana_client_clone = solana_client.clone();
        let config_clone = config.clone();
        let token_rotator_clone = token_rotator.clone();
        let token_a_clone = token_a.clone();
        let rate_limiter_clone = rate_limiter.clone();
        let trending_tokens_clone = trending_tokens.clone();
        
        // Get initial token for this thread
        let initial_token_b = if config.is_use_token_rotation {
            match token_rotator_clone.as_ref().unwrap().get_token_for_thread(thread_id) {
                Ok(token) => token,
                Err(_) => JupiterClient::get_usdc_token(),
            }
        } else {
            // When rotation is disabled, select a random token from trending tokens
            if trending_tokens_clone.is_empty() {
                JupiterClient::get_usdc_token()
            } else {
                let mut rng = rand::thread_rng();
                trending_tokens_clone[rng.gen_range(0..trending_tokens_clone.len())].clone()
            }
        };
        
        // Set the thread ID in the Jupiter client
        jupiter_client_clone.set_thread_id(thread_id);
        
        // Set thread ID for rate limiter
        rate_limiter_clone.set_thread_id(thread_id);
        
        // Use the existing SolanaClient clone
        let solana_client_clone = solana_client_clone.clone();
        
        // Spawn a new task for this thread
        let handle = tokio::spawn(async move {
            info!("Started arbitrage thread {} for pair {} -> {} -> {}", 
                thread_id, token_a_clone.symbol, initial_token_b.symbol, token_a_clone.symbol);
        
            // Initialize arbitrage scanner for this thread
            let mut arbitrage_scanner = ArbitrageScanner::new(
                jupiter_client_clone.clone(),
                config_clone.clone(),
                token_a_clone.clone(),  // This is WSOL
                initial_token_b.clone(),
            );
            
            // Initialize arbitrage executor
            let mut arbitrage_executor = ArbitrageExecutor::new(
                solana_client_clone.clone(),
                config_clone.clone(),
            );
            
            // Initialize flash loan instructions at startup
            if config_clone.use_flash_loan {
                if let Err(e) = arbitrage_executor.initialize_flash_loan().await {
                    error!("Failed to initialize flash loan instructions: {}", e);
                    // Continue running, will fall back to creating instructions on the fly
                } else {
                    info!("Thread {} flash loan instructions initialized successfully", thread_id);
                }
            }
            
            // Thread-specific variables 
            let mut scan_count = 0;
            let mut last_status_log = Instant::now();
            let start_time = Instant::now();
            let mut last_rotation_time = Instant::now();
            // Add constant for token rotation check frequency (scans)
            let token_rotation_check_frequency = 50; // Check for rotation every 50 scans
            // Track trending token index when rotation is disabled
            // Start each thread at a different position in the trending tokens list
            let mut trending_token_index = if !trending_tokens_clone.is_empty() {
                thread_id % trending_tokens_clone.len()
            } else {
                0
            };
            
            info!("Thread {} started with token pair: {} -> {} -> {}", 
                thread_id, token_a_clone.symbol, arbitrage_scanner.token_b.symbol, token_a_clone.symbol);
            
            // Main arbitrage scanning loop
            loop {
                scan_count += 1;
                
                // Record and log progress
                if scan_count % 1000 == 0 {
                    let elapsed = start_time.elapsed();
                    let scans_per_sec = scan_count as f64 / elapsed.as_secs_f64();
                    info!("Thread {} completed {} scans in {:?} ({:.2} scans/sec)", 
                        thread_id, scan_count, elapsed, scans_per_sec);
                }
                
                // Handle token rotation - different logic based on IS_USE_TOKEN_ROTATION setting
                if config_clone.is_use_token_rotation {
                    // Use TokenRotator for managed token rotation
                    if scan_count % token_rotation_check_frequency == 0 {
                        if let Some(token_rotator) = &token_rotator_clone {
                            match token_rotator.rotate_token_for_thread(thread_id) {
                                Ok(new_token) => {
                                    // Update token in the arbitrage scanner
                                    arbitrage_scanner.update_token_b(new_token.clone());
                                    debug!("Thread {} rotated to new token: {}", thread_id, new_token.symbol);
                                },
                                Err(e) => {
                                    warn!("Thread {} failed to rotate token: {}", thread_id, e);
                                }
                            }
                        }
                    }
                } else if !trending_tokens_clone.is_empty() && scan_count % 5 == 0 {
                    // When IS_USE_TOKEN_ROTATION=false, rotate through trending tokens on each scan
                    // Use sequential rotation through the trending tokens list
                    trending_token_index = (trending_token_index + 1) % trending_tokens_clone.len();
                    let new_token = trending_tokens_clone[trending_token_index].clone();
                    
                    // Update token in the arbitrage scanner
                    arbitrage_scanner.update_token_b(new_token.clone());
                    debug!("Thread {} rotated to trending token: {} (sequential {})", 
                        thread_id, new_token.symbol, trending_token_index + 1);
                }
            
                // Check if we're rate limited and can make a request
                if !rate_limiter_clone.is_limited() {
                    // Scan for arbitrage opportunity - this function uses batch API calls internally
                    match arbitrage_scanner.scan_for_opportunity().await {
                        Ok(Some(opportunity)) => {
                            info!("Thread {} found arbitrage opportunity with {}% profit", 
                                thread_id, opportunity.profit_percentage);
                            info!("Opportunity details: {} -> {} -> {}, Profit: {} SOL", 
                                token_a_clone.symbol,
                                arbitrage_scanner.token_b.symbol,
                                token_a_clone.symbol,
                                opportunity.profit_lamports as f64 / 1_000_000_000.0);
                                
                            // Capture start time for performance tracking
                            let execution_start = Instant::now();
                                    
                            // Execute the arbitrage if profitable
                            match arbitrage_executor.execute_arbitrage(opportunity).await {
                                Ok(result) => {
                                    // Log execution time
                                    let execution_time = execution_start.elapsed();
                                    info!("Thread {} arbitrage execution took {:?}", thread_id, execution_time);
                                    
                                    if result.success {
                                        // Update statistics
                                        TOTAL_TRADES_COUNTER.fetch_add(1, Ordering::SeqCst);
                                        
                                        // Calculate profit in SOL
                                        let profit_sol = if result.output_amount > result.input_amount {
                                            (result.output_amount - result.input_amount) as f64 / 1_000_000_000.0
                                        } else {
                                            0.0
                                        };
                                                
                                        TOTAL_PROFIT_COUNTER.fetch_add((profit_sol * 1_000_000_000.0) as i64, Ordering::SeqCst);
                                                
                                        info!("Thread {} successfully executed arbitrage: {}% profit, {:?}", 
                                            thread_id, result.profit_percentage, result.signature);
                                    } else {
                                        warn!("Thread {} arbitrage execution failed: {}", 
                                            thread_id, result.error.unwrap_or_else(|| "Unknown error".to_string()));
                                    }
                                },
                                Err(e) => {
                                    error!("Thread {} error executing arbitrage: {}", thread_id, e);
                                    let error_message = e.to_string();
                                    let re = Regex::new(r"custom program error: 0x1\b").unwrap(); // \b ensures exact match
                                    let wallet_pubkey = solana_client_clone.wallet_pubkey();
                                    let balance = solana_client_clone.get_balance(&wallet_pubkey).await.unwrap_or(0);
                                    if re.is_match(&error_message) && balance < 10000000 {
                                        info!("Attempting to close WSOL token account...");
                                        match close_wsol_and_reopen_token_account(&solana_client_clone).await {
                                            Ok(signature) => info!("Successfully closed WSOL token account. Signature: {}", signature),
                                            Err(close_err) => error!("Failed to close WSOL token account: {}", close_err),
                                        }
                                    }
                                }
                            }
                        },
                        Ok(None) => {
                            // No opportunity found, small sleep to avoid CPU spinning
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        },
                        Err(e) => {
                            error!("Thread {} error scanning for opportunities: {}", thread_id, e);
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                } else {
                    // Rate limited, sleep for a short period
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        });
        
        thread_handles.push(handle);
    }
    
    // Send notification that all threads are running
    if let Err(e) = telegram_notifier.send_message(&format!(
        "✅ <b>All {} threads started successfully</b>\n\n\
        Now scanning for arbitrage opportunities between SOL and other tokens.",
        config.thread_amount
    )).await {
        warn!("Failed to send Telegram thread status notification: {}", e);
    }
    
    // Wait for all threads to complete (they won't in this case, but good practice)
    for handle in thread_handles {
        if let Err(e) = handle.await {
            let error_msg = format!("Thread panicked: {}", e);
            error!("{}", error_msg);
            
            // Send error notification via Telegram
            if let Err(notify_err) = telegram_notifier.send_error_notification(&error_msg).await {
                warn!("Failed to send Telegram error notification: {}", notify_err);
            }
        }
    }
    
    // Start a dedicated thread for cache saving
    let jupiter_client_clone = jupiter_client.clone();
    let cache_thread_handle = tokio::spawn(async move {
        info!("Starting dedicated instruction cache saving thread");
        let save_interval = Duration::from_secs(300); // 5 minutes
        
        loop {
            // Check if shutdown signal is received
            if SHUTDOWN_SIGNAL.load(Ordering::SeqCst) {
                info!("Cache saving thread received shutdown signal, performing final save");
                // Do a final save before exiting
                jupiter_client_clone.save_instructions_cache();
                break;
            }
            
            // Sleep for 5 minutes
            tokio::time::sleep(save_interval).await;
            
            // Save the instruction cache
            debug!("Periodic instruction cache save triggered");
            jupiter_client_clone.save_instructions_cache();
        }
        
        info!("Cache saving thread exited cleanly");
    });
    
    // Set up Ctrl+C handler for graceful shutdown
    let shutdown_signal = Arc::new(AtomicBool::new(false));
    let shutdown_signal_clone = shutdown_signal.clone();
    
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                info!("Received Ctrl+C, initiating graceful shutdown");
                shutdown_signal_clone.store(true, Ordering::SeqCst);
                SHUTDOWN_SIGNAL.store(true, Ordering::SeqCst);
            }
            Err(err) => {
                error!("Failed to listen for Ctrl+C: {}", err);
            }
        }
    });
    
    // Prepare for shutdown at the end of main
    info!("Jupiter bot shutting down, saving cache");
    SHUTDOWN_SIGNAL.store(true, Ordering::SeqCst);
    
    // Wait for cache thread to complete (with timeout)
    match tokio::time::timeout(Duration::from_secs(10), cache_thread_handle).await {
        Ok(result) => {
            match result {
                Ok(_) => info!("Cache saving thread exited successfully"),
                Err(e) => warn!("Cache saving thread exited with error: {}", e),
            }
        },
        Err(_) => {
            warn!("Cache saving thread did not exit within timeout period");
        }
    }
    
    info!("Jupiter bot shutdown complete");
    
    Ok(())
}

// Calculate maximum requests per minute based on paid API level (50 req/sec)
fn calculate_max_requests_per_minute(_config: &config::Config) -> u32 {
    // Return 3000 RPM (50 RPS) for paid API
    3000
}

// Helper function to close WSOL token account
async fn close_wsol_and_reopen_token_account(solana_client: &Arc<SolanaClient>) -> anyhow::Result<String> {
    let wsol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")?;
    let wallet_pubkey = solana_client.wallet_pubkey();
    
    // Get the associated token account for WSOL
    let wsol_token_account = get_associated_token_address(
        &wallet_pubkey,
        &wsol_mint,
    );
    
    info!("Attempting to close WSOL token account: {}", wsol_token_account);
    
    // Check if the account exists
    match solana_client.get_account(&wsol_token_account).await {
        Ok(account) => {
            info!("Found WSOL token account with balance: {} lamports", account.lamports);
            
            // Create close account instruction
            let close_instruction = spl_token::instruction::close_account(
                &spl_token::id(),
                &wsol_token_account,
                &wallet_pubkey,
                &wallet_pubkey,
                &[&wallet_pubkey],
            )?;
            
        // If account doesn't exist, create it
        let create_ata_instruction = spl_associated_token_account::instruction::create_associated_token_account(
                &wallet_pubkey,
                &wallet_pubkey,
                &wsol_mint,
                &spl_token::id(),
            );
            
            let recent_blockhash = solana_client.get_latest_blockhash().await?;
            let transaction = Transaction::new_signed_with_payer(
                &[ close_instruction , create_ata_instruction],
                Some(&wallet_pubkey),
                &[solana_client.get_keypair()],
                recent_blockhash,
            );
            
            let signature = solana_client.send_transaction(&transaction).await?;
            info!("Created new WSOL token account with signature: {}", signature);
            
            // Add WSOL to the ATA cache
            add_to_ata_cache(&wsol_mint);
            
            // Save updated cache to file
            match arbitrage::save_ata_cache_to_file(&get_ata_cache()) {
                Ok(_) => info!("Updated ATA cache with new WSOL account"),
                Err(e) => warn!("Failed to save ATA cache after WSOL recreation: {}", e),
            }
            
            Ok(signature)
        }
        Err(e) => {
            warn!("WSOL token account not found or error accessing it: {}", e);
            Err(e)
        }
    }
}

// Update the initialize_ata_cache function to use arbitrage::save_ata_cache_to_file
async fn initialize_ata_cache(solana_client: &Arc<SolanaClient>) -> anyhow::Result<()> {
    let start_time = Instant::now();
    info!("Initializing ATA cache...");
    
    // First try to load the cache from file
    if let Ok(cache) = load_ata_cache_from_file() {
        init_ata_cache(cache);
        info!("Loaded ATA cache from file");
    } else {
        info!("No ATA cache file found or unable to load, building from scratch");
        
        // Get the wallet pubkey
        let wallet_pubkey = solana_client.wallet_pubkey();
        
        // Get all token accounts associated with this wallet
        info!("Fetching token accounts for wallet: {}", wallet_pubkey);
        let token_accounts = solana_client.get_token_accounts(&wallet_pubkey).await?;
        
        // Create cache entries for each token mint
        let mut ata_cache = AtaCache {
            accounts: HashMap::new(),
        };
        
        for account in token_accounts {
            // Extract the mint from the token account
            if let Some(mint) = account.mint {
                let mint_pubkey = Pubkey::from_str(&mint)?;
                ata_cache.accounts.insert(mint_pubkey, true);
                info!("Added token mint to cache: {}", mint);
            }
        }
        
        // Initialize the cache
        init_ata_cache(ata_cache.clone());
        info!("ATA cache initialized with {} entries:::::::::::::::", ata_cache.accounts.len());
        
        // Save the cache to file for future use
        arbitrage::save_ata_cache_to_file(&ata_cache)?;
    }
    
    let elapsed = start_time.elapsed();
    info!("ATA cache initialization completed in {:?}", elapsed);
    
    Ok(())
}

// Helper function to load ATA cache from file
fn load_ata_cache_from_file() -> anyhow::Result<AtaCache> {
    let path = Path::new(ATA_CACHE_FILE);
    
    if !path.exists() {
        return Err(anyhow::anyhow!("ATA cache file does not exist"));
    }
    
    // Read the file
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    
    // Parse JSON
    let cache: AtaCache = serde_json::from_str(&contents)?;
    
    Ok(cache)
}

// Update the create_token_account_lookup_table function to fix compile errors
async fn create_token_account_lookup_table(solana_client: &Arc<SolanaClient>) -> anyhow::Result<String> {
    use solana_sdk::pubkey::Pubkey;
    use solana_address_lookup_table_program::instruction::{create_lookup_table, extend_lookup_table};
    use solana_sdk::signature::{Keypair, Signer};
    use std::str::FromStr;
    use solana_client::rpc_client::RpcClient;
    use solana_sdk::commitment_config::CommitmentConfig;
    use solana_sdk::transaction::Transaction;
    use solana_sdk::hash::Hash;
    use std::time::{SystemTime, UNIX_EPOCH};
    use log::{info, warn, error};
    
    info!("Creating address lookup table for token accounts...");
    
    // Check if the lookup table already exists in the environment
    let lookup_table_addr_str = std::env::var("TOKEN_ACCOUNT_ADDRESS_LOOKUP_TABLE").unwrap_or_default();
    
    if !lookup_table_addr_str.is_empty() {
        info!("Using existing token account address lookup table: {}", lookup_table_addr_str);
        
        // Validate the existing lookup table
        let pubkey = Pubkey::from_str(&lookup_table_addr_str)?;
        match solana_client.get_account(&pubkey).await {
            Ok(account) => {
                if !account.data.is_empty() {
                    info!("Existing lookup table found and validated");
                    return Ok(lookup_table_addr_str);
                } else {
                    warn!("Existing lookup table is empty, creating a new one");
                }
            },
            Err(e) => {
                warn!("Failed to fetch existing lookup table: {}, creating a new one", e);
            }
        }
    }
    
    let wallet_pubkey = solana_client.wallet_pubkey();
    
    // Load all token accounts owned by the wallet
    info!("Finding all token accounts owned by {}", wallet_pubkey);
    
    // Get recent blockhash
    let blockhash = solana_client.get_latest_blockhash().await?;
    
    // Get recent slot with finalized commitment
    let recent_slot = solana_client.get_slot_with_commitment(
        CommitmentConfig::finalized()
    ).await?;
    
    // Create the lookup table - FIXED: Use wallet_pubkey for both payer and authority
    let (create_ix, lookup_table_address) = create_lookup_table(
        wallet_pubkey,
        wallet_pubkey, // Use wallet as authority instead of a temporary keypair
        recent_slot,   // Use finalized slot
    );
    
    info!("Creating new lookup table with address: {}", lookup_table_address);
    
    // Submit transaction to create the lookup table
    let create_tx = Transaction::new_signed_with_payer(
        &[create_ix],
        Some(&wallet_pubkey),
        &[solana_client.get_keypair()], // Only need wallet signer now
        blockhash,
    );
    
    // Send and confirm the transaction
    info!("Creating address lookup table...");
    let signature = solana_client.send_and_confirm_transaction(&create_tx).await?;
    info!("Created address lookup table: {} with signature: {}", lookup_table_address, signature);
    
    // Wait a moment for the table to be confirmed
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    
    // Get all token accounts - use the correct method from SolanaClient
    let token_accounts = solana_client.get_token_accounts(&wallet_pubkey).await?;
    
    info!("Found {} token accounts to add to lookup table", token_accounts.len());
    
    // Process addresses in chunks to avoid transaction size limits
    const MAX_ADDRESSES_PER_EXTEND: usize = 30;
    // Convert token account addresses to Pubkeys
    let token_pubkeys: Vec<Pubkey> = token_accounts
        .iter()
        .map(|a| Pubkey::from_str(&a.address).unwrap())
        .collect();
    
    // Process addresses in batches
    for (i, chunk) in token_pubkeys.chunks(MAX_ADDRESSES_PER_EXTEND).enumerate() {
        // Get recent blockhash for each transaction
        let blockhash = solana_client.get_latest_blockhash().await?;
        
        // Create extend instruction - FIXED: Match the parameter order from flash_loan.rs
        let extend_ix = extend_lookup_table(
            lookup_table_address,
            wallet_pubkey,         // Authority is wallet_pubkey now
            Some(wallet_pubkey),   // Use Some() for payer
            chunk.to_vec(),
        );
        
        // Create and send transaction
        let extend_tx = Transaction::new_signed_with_payer(
            &[extend_ix],
            Some(&wallet_pubkey),
            &[solana_client.get_keypair()], // Only need wallet signer now
            blockhash,
        );
        
        // Send and confirm the transaction
        info!("Extending address lookup table batch {}/{}...", 
             i + 1, 
             (token_pubkeys.len() + MAX_ADDRESSES_PER_EXTEND - 1) / MAX_ADDRESSES_PER_EXTEND);
        let signature = solana_client.send_and_confirm_transaction(&extend_tx).await?;
        info!("Extended lookup table batch with {} addresses, signature: {}", 
             chunk.len(),
             signature);
             
        // Sleep briefly to avoid rate limits
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }
    
    // Return the address as a string
    let lookup_table_address_str = lookup_table_address.to_string();
    
    // Suggest adding this to the .env file
    info!("Successfully created token account lookup table with address: {}", lookup_table_address_str);
    info!("Add this to your .env file: TOKEN_ACCOUNT_ADDRESS_LOOKUP_TABLE={}", lookup_table_address_str);
    
    Ok(lookup_table_address_str)
}

// Add this function to demonstrate parallel API requests
async fn fetch_market_data_parallel(
    jupiter_client: &Arc<JupiterClient>,
    solana_client: &Arc<SolanaClient>
) -> anyhow::Result<()> {
    info!("Fetching market data in parallel using tokio::join!");
    
    // Create multiple async tasks to run concurrently
    let trending_tokens_future = jupiter_client.fetch_trending_tokens();
    let sol_price_future = jupiter_client.get_quote(
        "So11111111111111111111111111111111111111112", // WSOL
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // USDC
        1_000_000_000, // 1 SOL
        100 // 1% slippage
    );
    let slot_future = solana_client.get_slot();
    
    // Execute all tasks in parallel using tokio::join!
    let (trending_tokens_result, sol_price_result, slot_result) = 
        tokio::join!(trending_tokens_future, sol_price_future, slot_future);
    
    // Process results
    let trending_tokens = trending_tokens_result?;
    let sol_price = sol_price_result?;
    let slot = slot_result?;
    
    info!("Parallel API requests completed:");
    info!("  - Retrieved {} trending tokens", trending_tokens.len());
    info!("  - Current SOL price: {} USDC", 
          u64::from_str(&sol_price.out_amount).unwrap_or(0) as f64 / 1_000_000.0);
    info!("  - Current Solana slot: {}", slot);
    
    Ok(())
}

// Example of using FlashLoanContext caching
async fn initialize_flash_loan_context(
    solana_client: &Arc<SolanaClient>,
    config: &Arc<config::Config>
) -> anyhow::Result<()> {
    if !config.use_flash_loan {
        return Ok(());
    }
    
    // Get environment variables
    let lending_market_authority = std::env::var("LENDING_MARKET_AUTHORITY")
        .unwrap_or_else(|_| "9aSbXf3iXvWrZNSVoTQvPPNrviNzFN3BJeGbRUjCCJfA".to_string());
    let lending_market = std::env::var("LENDING_MARKET_ADDRESS")
        .unwrap_or_else(|_| "D4fFGQ1vYA8thJfRrZTZxiAxCh5U147hbMSJgJ3ULyVz".to_string());
    let reserve = std::env::var("RESERVE_ADDRESS")
        .unwrap_or_else(|_| "5h5XZJghdRSzmXKbHGZpYCvYeGR3Hij1or9P6BsHP5ep".to_string());
    let reserve_liquidity_mint = std::env::var("RESERVE_LIQUIDITY_MINT")
        .unwrap_or_else(|_| "So11111111111111111111111111111111111111112".to_string());
    let reserve_source_liquidity = std::env::var("RESERVE_SOURCE_LIQUIDITY")
        .unwrap_or_else(|_| "2rn6UeRaWB6XQnCaCrn1UZKxQYfGzabaAVsxkFm1X5h4".to_string());
    let reserve_liquidity_fee_receiver = std::env::var("RESERVE_LIQUIDITY_FEE_RECEIVER")
        .unwrap_or_else(|_| "EyNnzpo2526CuC2xKLbc2YSVDNRXvr5hyoFe7dGPsqk6".to_string());
    let referrer_token_state = std::env::var("REFERRER_TOKEN_STATE")
        .unwrap_or_else(|_| "F21zCuN1PYfDC11QuAZDFNapjRuzmGMwFY7ByPo8xnJk".to_string());
    let referrer_account = std::env::var("REFERRER_ACCOUNT")
        .unwrap_or_else(|_| "4yFmuhNaYoYvxQkYUUjmLDN8Qn4ZPuJJ14uLZSNi9JBv".to_string());
    
    // Initialize FlashLoanContext with caching
    info!("Initializing cached FlashLoanContext...");
    let _fl_context = FlashLoanContext::get_or_create(
        solana_client.clone(),
        &lending_market_authority,
        &lending_market,
        &reserve,
        &reserve_liquidity_mint,
        &reserve_source_liquidity,
        &reserve_liquidity_fee_receiver,
        &referrer_token_state,
        &referrer_account
    ).await?;
    
    info!("FlashLoanContext initialized and cached successfully");
    
    Ok(())
}

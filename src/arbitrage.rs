use crate::jupiter_api::{JupiterClient, Quote, Token, SwapInstructions, AccountData, QuoteRequest, InstructionData};
use crate::solana::SolanaClient;
use crate::config::Config;
use crate::flash_loan::{FlashLoanContext, FlashBorrowReserveLiquidityArgs, FlashRepayReserveLiquidityArgs, create_flash_borrow_instruction, create_flash_repay_instruction, create_or_get_flash_loan_lookup_table};
use crate::jito;
use crate::telegram::TelegramNotifier;
use anyhow::{Result, anyhow};
use log::{info,  debug, error, warn, trace};
use std::sync::Arc;
use base64::{Engine, prelude::BASE64_STANDARD};
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    instruction::{Instruction, AccountMeta},
    pubkey::Pubkey,
    address_lookup_table_account::AddressLookupTableAccount,
    message::v0::Message as MessageV0,
    transaction::{VersionedTransaction, Transaction},
    hash::Hash,
    message::VersionedMessage,
    signer::Signer,
    system_instruction,
};
use solana_address_lookup_table_program::{self, state::AddressLookupTable};
use serde_json;
use reqwest;
use serde::Deserialize;
use std::str::FromStr;
use std::env;
use jupiter_sdk::generated::instructions::route::{
    Route, 
    RouteInstructionArgs
};
use jupiter_sdk::generated::types::{RoutePlanStep, Swap, Side};
use jupiter_sdk::generated::programs::JUPITER_ID;
use spl_associated_token_account::get_associated_token_address;
use std::collections::{HashMap, HashSet};
use tokio;
use std::time::Duration;
use std::sync::Mutex;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use lazy_static::lazy_static;
use spl_token;
use crate::utils::{get_env_var, get_env_var_bool};
use serde::{Deserializer, Serialize};
use solana_sdk::instruction::InstructionError;

// Add this for the ATA cache
use serde::de::Error;

// Define the cache file name
const ATA_CACHE_FILE: &str = "ata_cache.json";

// Define the WSOL mint address and minimum WSOL balance to keep
const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";
// Minimum balance to keep in WSOL (0.0001 SOL in lamports)
const MIN_WSOL_BALANCE: u64 = 100_000;

// Add a lazy static for the IS_CHECK_TOKEN_EXIST environment variable
lazy_static! {
    static ref IS_CHECK_TOKEN_EXIST: bool = {
        get_env_var_bool("IS_CHECK_TOKEN_EXIST", true)
    };
}

// Define a global ATA cache to track which token accounts have been created
lazy_static! {
    static ref ATA_CACHE: Mutex<HashMap<Pubkey, bool>> = Mutex::new(HashMap::new());
}

// Define a serializable version of the ATA cache for loading/saving
#[derive(Serialize, Deserialize, Clone)]
pub struct AtaCache {
    #[serde(deserialize_with = "deserialize_string_map")]
    #[serde(serialize_with = "serialize_pubkey_map")]
    pub accounts: HashMap<Pubkey, bool>,
}

// Custom deserializer for the string keys in the JSON to convert to Pubkey
fn deserialize_string_map<'de, D>(deserializer: D) -> Result<HashMap<Pubkey, bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let string_map: HashMap<String, bool> = HashMap::deserialize(deserializer)?;
    let mut pubkey_map = HashMap::new();
    
    for (key_str, value) in string_map {
        let pubkey = Pubkey::from_str(&key_str).map_err(D::Error::custom)?;
        pubkey_map.insert(pubkey, value);
    }
    
    Ok(pubkey_map)
}

// Custom serializer for the Pubkey keys to convert to strings
fn serialize_pubkey_map<S>(
    pubkey_map: &HashMap<Pubkey, bool>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    
    let mut map = serializer.serialize_map(Some(pubkey_map.len()))?;
    for (pubkey, value) in pubkey_map {
        map.serialize_entry(&pubkey.to_string(), value)?;
    }
    map.end()
}

// Functions to manipulate the ATA cache
pub fn add_to_ata_cache(mint: &Pubkey) {
    let mut cache = ATA_CACHE.lock().unwrap();
    cache.insert(*mint, true);
}

pub fn is_in_ata_cache(mint: &Pubkey) -> bool {
    let cache = ATA_CACHE.lock().unwrap();
    cache.contains_key(mint)
}

pub fn init_ata_cache(cache_data: AtaCache) {
    let mut cache = ATA_CACHE.lock().unwrap();
    *cache = cache_data.accounts;
    info!("Initialized ATA cache with {} entries", cache.len());
}

pub fn get_ata_cache() -> AtaCache {
    let cache = ATA_CACHE.lock().unwrap();
    AtaCache {
        accounts: cache.clone(),
    }
}

pub struct ArbitrageScanner {
    pub jupiter_client: Arc<JupiterClient>,
    pub config: Arc<Config>,
    pub token_a: Token,
    pub token_b: Token,
    pub iteration: u64,
    pub max_profit_spotted: f64,
}

// Add these constants for log configuration
const LOG_OPPORTUNITIES_INTERVAL: u64 = 1000; // Only log opportunities every 1000 iterations
const LOG_PROFIT_INTERVAL: u64 = 200; // Log profit stats every 200 iterations
const VERBOSE_LOGGING: bool = false; // Set this to false to reduce log volume

impl ArbitrageScanner {
    pub fn new(
        jupiter_client: Arc<JupiterClient>,
        config: Arc<Config>,
        token_a: Token,
        token_b: Token,
    ) -> Self {            
        Self {
            jupiter_client,
            config,
            token_a,
            token_b,
            iteration: 0,
            max_profit_spotted: 0.0,
        }
    }
    
    // Add method to update token_b during execution
    pub fn update_token_b(&mut self, new_token: Token) {
        // Reset iteration and max profit spotted when changing token
        self.iteration = 0;
        self.max_profit_spotted = 0.0;
        
        // Update the token
        self.token_b = new_token;
    }
    
    pub async fn scan_for_opportunity(&mut self) -> Result<Option<ArbitrageOpportunity>> {
        // Increment iteration counter
        self.iteration += 1;
        
        // Log manufacturing settings on first iteration
        if self.iteration == 1 {
            if self.config.use_manufactured_quotes {
                info!("Quote manufacturing is ENABLED (model: {}, factor: {:.2})", 
                      self.config.price_impact_model, 
                      self.config.price_impact_factor);
                
                if self.config.validate_large_quotes {
                    info!("Large quote validation is ENABLED (threshold: {} SOL)", 
                          self.config.validation_threshold_sol);
                } else {
                    info!("Large quote validation is DISABLED");
                }
            } else {
                info!("Quote manufacturing is DISABLED - using real quotes for all transactions");
            }
        }
        
        // Get configuration values
        let trade_size_sol = self.config.trade_size_sol;
        let min_profit_threshold = self.config.min_profit_threshold;
        let slippage_bps = self.config.slippage_bps as u64;
        
        // Scan for arbitrage opportunities
        let opportunity = self.scan_for_arbitrage_opportunities(
            &self.token_a,
            &self.token_b,
            trade_size_sol,
            min_profit_threshold,
            slippage_bps,
        ).await?;
        
        // If we found an opportunity, update max profit spotted
        if let Some(ref opp) = opportunity {
            if opp.profit_percentage > self.max_profit_spotted {
                self.max_profit_spotted = opp.profit_percentage;
            }
            
            // Only log every so often
            if self.iteration % LOG_OPPORTUNITIES_INTERVAL == 0 {
                let quote_src = if opp.manufactured_quotes { "manufactured" } else { "real" };
                info!("Found opportunity with profit {:.4}% (using {} quotes), max profit spotted so far: {:.4}%", 
                      opp.profit_percentage,
                      quote_src,
                      self.max_profit_spotted);
            }
        } else if self.iteration % LOG_PROFIT_INTERVAL == 0 {
            // Log max profit periodically
            debug!("Iteration {}, no opportunity found, max profit spotted so far: {:.4}%", 
                   self.iteration, 
                   self.max_profit_spotted);
        }
        
        Ok(opportunity)
    }
    
    // Refined manufacture_quote function with improved price impact modeling
    fn manufacture_quote(
        &self,
        small_quote: &Quote,
        new_in_amount: u64,
        slippage_bps: u64,
    ) -> Quote {
        // Parse the original amounts
        let original_in_amount = small_quote.in_amount.parse::<u64>().unwrap_or(1);
        let original_out_amount = small_quote.out_amount.parse::<u64>().unwrap_or(1);
        
        if original_in_amount == 0 {
            return small_quote.clone();
        }
        
        // Calculate the exchange rate from the small quote
        let exchange_rate = original_out_amount as f64 / original_in_amount as f64;
        
        // Calculate the scale factor between new amount and original amount
        let scale_factor = new_in_amount as f64 / original_in_amount as f64;
        
        // Use the price impact model from config
        let impact_model = self.config.price_impact_model.to_lowercase();
        let impact_factor = self.config.price_impact_factor;
            
        // Calculate impact scaling based on the selected model
        let impact_scaling = match impact_model.as_str() {
            "linear" => {
                // Linear scaling - impact directly proportional to size
                scale_factor * impact_factor
            },
            "sqrt" => {
                // Square root scaling - impact increases with sqrt of size (default)
                if scale_factor > 1.0 {
                    (scale_factor.sqrt()) * impact_factor
                } else {
                    scale_factor * impact_factor
                }
            },
            "square" => {
                // Square scaling - impact increases with square of size (more aggressive)
                if scale_factor > 1.0 {
                    (scale_factor * scale_factor).sqrt() * impact_factor
                } else {
                    scale_factor * impact_factor
                }
            },
            "log" => {
                // Logarithmic scaling - impact increases with log of size (more conservative)
                if scale_factor > 1.0 {
                    (1.0 + scale_factor.ln()) * impact_factor
                } else {
                    scale_factor * impact_factor
                }
            },
            _ => {
                // Default to sqrt model
                if scale_factor > 1.0 {
                    (scale_factor.sqrt()) * impact_factor
                } else {
                    scale_factor * impact_factor
                }
            }
        };
        
        // Extract the original price impact as a decimal
        let original_impact = match small_quote.price_impact_pct.parse::<f64>() {
            Ok(impact) => impact / 100.0,  // Convert from percentage to decimal
            Err(_) => 0.0,                // Default to 0 if parsing fails
        };
        
        // Calculate adjusted price impact for the new amount
        let adjusted_impact = original_impact * impact_scaling;
        
        // Apply minimum impact constraint to prevent unrealistic quotes
        let min_impact = original_impact * 0.5;  // Never go below half the original impact
        let adjusted_impact = adjusted_impact.max(min_impact);
        
        // Cap maximum impact to prevent extreme values
        let max_impact = 0.1;  // Maximum 10% price impact
        let adjusted_impact = adjusted_impact.min(max_impact);
        
        // Calculate new output amount with adjusted impact
        let estimated_out_amount = (new_in_amount as f64 * exchange_rate * (1.0 - adjusted_impact)) as u64;
        
        // Calculate the minimum acceptable output with slippage
        let other_amount_threshold = (estimated_out_amount as f64 * (1.0 - (slippage_bps as f64 / 10000.0))) as u64;
        
        // Create a new quote with manufactured values
        let mut new_quote = small_quote.clone();
        new_quote.in_amount = new_in_amount.to_string();
        new_quote.out_amount = estimated_out_amount.to_string();
        new_quote.other_amount_threshold = other_amount_threshold.to_string();
        
        // Update the price impact - convert back to percentage string
        let impact_percentage = (adjusted_impact * 100.0).to_string();
        new_quote.price_impact_pct = impact_percentage;
        
        // Adjust the route plan amounts
        for route in &mut new_quote.route_plan {
            // Adjust input, output and fee proportionally for each route in the plan
            route.swap_info.in_amount = new_in_amount.to_string();
            route.swap_info.out_amount = estimated_out_amount.to_string();
            
            // Adjust fee amount proportionally
            if let Ok(original_fee) = route.swap_info.fee_amount.parse::<u64>() {
                let new_fee = (original_fee as f64 * scale_factor) as u64;
                route.swap_info.fee_amount = new_fee.to_string();
            }
        }
        
        info!("Manufactured quote: {} -> {} with rate {} (original rate: {}), impact: {}%",
               new_quote.in_amount,
               new_quote.out_amount,
               estimated_out_amount as f64 / new_in_amount as f64,
               exchange_rate,
               adjusted_impact * 100.0);
        
        new_quote
    }
    
    pub async fn scan_for_arbitrage_opportunities(
        &self,
        token_a: &Token,
        token_b: &Token,
        trade_size_sol: f64,
        min_profit_threshold: f64,
        slippage_bps: u64,
    ) -> Result<Option<ArbitrageOpportunity>> {
        // Calculate trade amount in lamports (1 SOL = 10^9 lamports)
        let mut trade_amount_lamports = (trade_size_sol * 1_000_000_000.0) as u64;
        
        // Check if dynamic trade size is enabled from environment variable
        let use_dynamic_trade_size = get_env_var_bool("USE_DYNAMIC_TRADE_SIZE", false);
        
        // Use small quotes to determine profitability and optimal size
        // Use 1 SOL for initial profitability calculation
        let one_sol_lamports = 1_000_000_000;
        
        // Request both quotes simultaneously to reduce latency
        let small_quote_a_to_b_future = self.jupiter_client.get_quote(
            &token_a.address,
            &token_b.address,
            one_sol_lamports,
            slippage_bps,
        );
        
        // For the second quote, we need to use a reasonable amount of token_b
        // Use 1 full token of token_b (adjusted for decimals) instead of just 1 unit
        let token_b_amount = 10u64.pow(token_b.decimals as u32);
        
        let one_token_b_amount = token_b_amount;
        
        debug!("Using {} ({} units) for initial reverse quote", 
             token_b.symbol, one_token_b_amount);
        
        let small_quote_b_to_a_future = self.jupiter_client.get_quote(
            &token_b.address,
            &token_a.address,
            one_token_b_amount,
            slippage_bps,
        );
        
        // Wait for both futures to complete
        let (small_quote_a_to_b_result, small_quote_b_to_a_result) = 
            tokio::join!(small_quote_a_to_b_future, small_quote_b_to_a_future);
        
        // Process the results
        let small_quote_a_to_b = match small_quote_a_to_b_result {
            Ok(quote) => quote,
            Err(e) => {
                warn!("Failed to get small quote from {} to {}: {}", token_a.symbol, token_b.symbol, e);
                return Ok(None);
            }
        };
        
        let small_quote_b_to_a = match small_quote_b_to_a_result {
            Ok(quote) => quote,
            Err(e) => {
                warn!("Failed to get small quote from {} to {}: {}", token_b.symbol, token_a.symbol, e);
                return Ok(None);
            }
        };
        
        // Parse the output amounts
        let small_out_amount_a_to_b = match small_quote_a_to_b.out_amount.parse::<u64>() {
            Ok(amount) => amount,
            Err(e) => {
                warn!("Failed to parse small out_amount from quote: {}", e);
                return Ok(None);
            }
        };
        
        let small_out_amount_b_to_a = match small_quote_b_to_a.out_amount.parse::<u64>() {
            Ok(amount) => amount,
            Err(e) => {
                warn!("Failed to parse small out_amount from quote: {}", e);
                return Ok(None);
            }
        };
        
        // Calculate the exchange rates
        let sol_to_token_b_rate = small_out_amount_a_to_b as f64 / one_sol_lamports as f64;
        let token_b_to_sol_rate = small_out_amount_b_to_a as f64 / one_token_b_amount as f64;
        
        // Calculate the round-trip profit ratio for 1 SOL
        let profit_ratio = sol_to_token_b_rate * token_b_to_sol_rate * one_sol_lamports as f64;
        
        let profit_percentage = (profit_ratio - one_sol_lamports as f64) / one_sol_lamports as f64 * 100.0;
            
        info!("  Profit ratio for 1 SOL: {:.6} ({:.4}%)", 
            profit_ratio / one_sol_lamports as f64,
            profit_percentage);
        info!("use_dynamic_trade_size +++++++++=====: {}", use_dynamic_trade_size);
        
        // Return None when profit percentage is negative so we can quickly scan other tokens
        if profit_percentage <= 0.0 {
            debug!("No arbitrage opportunity found for {} -> {} (profit: {:.4}%)", 
                token_a.symbol, token_b.symbol, profit_percentage);
            return Ok(None);
        }
        
        // Calculate optimal trade size if profitable
        if use_dynamic_trade_size {
            // Calculate the optimal trade size based on the profit threshold and ratio
            // n = profit_threshold / profit_percentage * 100
            let jito_tip_lamports = self.config.jito_tip_lamports as f64;
            let optimal_size_sol = min_profit_threshold / profit_percentage + jito_tip_lamports / (profit_ratio - one_sol_lamports as f64);
            
            // Apply reasonable limits to the calculated trade size
            let max_trade_size_sol = 30.0; // Maximum 30 SOL per trade
            let min_trade_size_sol = 1.0;  // Minimum 0.6 SOL per trade
            let dynamic_trade_size_sol = optimal_size_sol.max(min_trade_size_sol).min(max_trade_size_sol);
            
            // Update the trade amount
            trade_amount_lamports = (dynamic_trade_size_sol * 1_000_000_000.0) as u64;
            
            info!("  Applied dynamic trade size: {:.6} SOL ({} lamports)", 
                dynamic_trade_size_sol, 
                trade_amount_lamports);
        } else {
            // Use fixed trade size (from config)
            info!("  Using fixed trade size: {:.6} SOL ({} lamports)", 
                trade_size_sol, 
                trade_amount_lamports);
        }
        
        // Check whether to use manufactured quotes based on config
        let use_manufactured_quotes = self.config.use_manufactured_quotes;
        
        // Determine if validation is needed based on trade size
        let trade_size_sol = trade_amount_lamports as f64 / 1_000_000_000.0;
        let need_validation = self.config.validate_large_quotes && trade_size_sol > self.config.validation_threshold_sol;
        
        let (quote_a_to_b, quote_b_to_a, out_amount_a_to_b, out_amount_b_to_a) = 
            if use_manufactured_quotes && !need_validation {
                info!("Using manufactured quotes for arbitrage calculation");
                
                // Manufacture quotes for the actual trade amounts instead of making API calls
                // Create first quote (A->B) based on small quote
                let quote_a_to_b = self.manufacture_quote(
                    &small_quote_a_to_b,
                    trade_amount_lamports,
                    slippage_bps
                );
                
                // Parse the output amount from the first manufactured quote
                let out_amount_a_to_b = match quote_a_to_b.out_amount.parse::<u64>() {
                    Ok(amount) => amount,
                    Err(e) => {
                        warn!("Failed to parse out_amount from manufactured quote: {}", e);
                        return Ok(None);
                    }
                };
                
                // Create second quote (B->A) based on small quote
                let quote_b_to_a = self.manufacture_quote(
                    &small_quote_b_to_a,
                    out_amount_a_to_b,
                    slippage_bps
                );
                
                // Parse the output amount from the second manufactured quote
                let out_amount_b_to_a = match quote_b_to_a.out_amount.parse::<u64>() {
                    Ok(amount) => amount,
                    Err(e) => {
                        warn!("Failed to parse out_amount from manufactured quote: {}", e);
                        return Ok(None);
                    }
                };
                
                (quote_a_to_b, quote_b_to_a, out_amount_a_to_b, out_amount_b_to_a)
            } else {
                // If need_validation is true, we'll use real quotes
                if need_validation {
                    info!("Trade size ({} SOL) exceeds validation threshold ({} SOL), using real quotes", 
                           trade_size_sol, self.config.validation_threshold_sol);
                } else {
                    info!("Using real quotes for arbitrage calculation (manufacturing disabled)");
                }
                
                // Request both quotes simultaneously
                let quote_a_to_b_future = self.jupiter_client.get_quote(
                    &token_a.address,
                    &token_b.address,
                    trade_amount_lamports,
                    slippage_bps,
                );
                
                // We need to wait for the first quote to get the output amount for the second quote
                let quote_a_to_b = match quote_a_to_b_future.await {
                    Ok(quote) => quote,
                    Err(e) => {
                        warn!("Failed to get quote from {} to {}: {}", token_a.symbol, token_b.symbol, e);
                        return Ok(None);
                    }
                };
                
                // Parse the output amount from the first quote
                let out_amount_a_to_b = match quote_a_to_b.out_amount.parse::<u64>() {
                    Ok(amount) => amount,
                    Err(e) => {
                        warn!("Failed to parse out_amount from quote: {}", e);
                        return Ok(None);
                    }
                };
                
                // Get quote for the reverse direction
                let quote_b_to_a = match self.jupiter_client.get_quote(
                    &token_b.address,
                    &token_a.address,
                    out_amount_a_to_b,
                    slippage_bps,
                ).await {
                    Ok(quote) => quote,
                    Err(e) => {
                        warn!("Failed to get quote from {} to {}: {}", token_b.symbol, token_a.symbol, e);
                        return Ok(None);
                    }
                };
                
                // Parse the output amount from the second quote
                let out_amount_b_to_a = match quote_b_to_a.out_amount.parse::<u64>() {
                    Ok(amount) => amount,
                    Err(e) => {
                        warn!("Failed to parse out_amount from quote: {}", e);
                        return Ok(None);
                    }
                };
                
                (quote_a_to_b, quote_b_to_a, out_amount_a_to_b, out_amount_b_to_a)
            };
        
        // Calculate profit in lamports
        let profit_lamports = out_amount_b_to_a as i64 - trade_amount_lamports as i64;
        
        // Calculate profit percentage
        let profit_percentage = (profit_lamports as f64 / trade_amount_lamports as f64) * 100.0;
        
        // Log the opportunity details
        info!("Arbitrage opportunity: {} -> {} -> {}", token_a.symbol, token_b.symbol, token_a.symbol);
        info!("  Input: {} SOL ({} lamports)", trade_amount_lamports as f64 / 1_000_000_000.0, trade_amount_lamports);
        info!("  Output: {} SOL ({} lamports)", out_amount_b_to_a as f64 / 1_000_000_000.0, out_amount_b_to_a);
        
        // Enhanced logging to show quote source
        let quote_source = if use_manufactured_quotes && !need_validation {
            "manufactured quotes"
        } else {
            "real quotes"
        };
        
        info!("  Profit: {} SOL ({} lamports, {:.4}%) [using {}]", 
            profit_lamports as f64 / 1_000_000_000.0, 
            profit_lamports,
            profit_percentage,
            quote_source);
        
        // Check if the profit meets the minimum threshold
        if profit_percentage < min_profit_threshold {
            info!("Profit {:.4}% is below minimum threshold of {:.4}%, skipping", 
                profit_percentage, min_profit_threshold);
            return Ok(None);
        }
        
        // Create the opportunity
        Ok(Some(ArbitrageOpportunity {
            token_a: token_a.clone(),
            token_b: token_b.clone(),
            quote_a_to_b,
            quote_b_to_a,
            profit_lamports,
            profit_percentage,
            trade_amount: trade_amount_lamports.to_string(),
            manufactured_quotes: use_manufactured_quotes && !need_validation,
        }))
    }

    // Add this new function
    pub async fn validate_quote_manufacturing(&self, token_a: &Token, token_b: &Token) -> Result<()> {
        info!("Validating quote manufacturing accuracy...");
        
        // Define test amounts in SOL
        let test_amounts = vec![1.0, 2.0, 5.0, 10.0, 20.0];
        let slippage_bps = self.config.slippage_bps as u64;
        
        // Get a small quote first for manufacturing
        let one_sol_lamports = 1_000_000_000;
        let small_quote_a_to_b = match self.jupiter_client.get_quote(
            &token_a.address,
            &token_b.address,
            one_sol_lamports,
            slippage_bps,
        ).await {
            Ok(quote) => quote,
            Err(e) => {
                error!("Failed to get small quote for validation: {}", e);
                return Err(anyhow!("Failed to get small quote for validation: {}", e));
            }
        };
        
        info!("Comparing manufactured quotes against real quotes:");
        info!("Amount | Man. Output | Real Output | Diff (%) | Impact Diff");
        info!("--------------------------------------------------------");
        
        for &amount in &test_amounts {
            // Convert to lamports
            let amount_lamports = (amount * 1_000_000_000.0) as u64;
            
            // Get manufactured quote
            let manufactured_quote = self.manufacture_quote(
                &small_quote_a_to_b,
                amount_lamports,
                slippage_bps
            );
            
            // Get real quote from API
            let real_quote = match self.jupiter_client.get_quote(
                &token_a.address,
                &token_b.address,
                amount_lamports,
                slippage_bps,
            ).await {
                Ok(quote) => quote,
                Err(e) => {
                    warn!("Failed to get real quote for {} SOL: {}", amount, e);
                    continue;
                }
            };
            
            // Parse outputs
            let manufactured_output = manufactured_quote.out_amount.parse::<u64>().unwrap_or(0);
            let real_output = real_quote.out_amount.parse::<u64>().unwrap_or(0);
            
            // Calculate difference percentage
            let diff_percentage = if real_output > 0 {
                ((manufactured_output as f64 - real_output as f64) / real_output as f64) * 100.0
            } else {
                0.0
            };
            
            // Calculate price impact difference
            let manufactured_impact = manufactured_quote.price_impact_pct.parse::<f64>().unwrap_or(0.0);
            let real_impact = real_quote.price_impact_pct.parse::<f64>().unwrap_or(0.0);
            let impact_diff = manufactured_impact - real_impact;
            
            info!(
                "{:5.1} | {:11} | {:11} | {:+7.2}% | {:+7.4}%", 
                amount, 
                manufactured_output, 
                real_output, 
                diff_percentage,
                impact_diff
            );
        }
        
        info!("Quote manufacturing validation complete");
        Ok(())
    }
}

impl ArbitrageExecutor {
    pub fn new(
        solana_client: Arc<SolanaClient>,
        config: Arc<Config>,
    ) -> Self {
        Self {
            solana_client,
            config,
            telegram_notifier: TelegramNotifier::new(),
            flash_borrow_instruction: None,
            flash_repay_instruction: None,
            flash_loan_ctx: None,
            flash_loan_lookup_table: None,
        }
    }
    
    // Add a method to initialize flash loan instructions
    pub async fn initialize_flash_loan(&mut self) -> Result<()> {
        if !self.config.use_flash_loan {
            debug!("Flash loan is disabled, skipping flash loan initialization");
            return Ok(());
        }
        
        info!("Initializing flash loan instructions at startup...");
        
        // Start timing
        let start_time = std::time::Instant::now();
        
        // Create flash loan context
        let flash_loan_ctx = FlashLoanContext::new(
            Arc::clone(&self.solana_client),
            &get_env_var("LENDING_MARKET_AUTHORITY", ""),
            &get_env_var("LENDING_MARKET_ADDRESS", ""),
            &get_env_var("RESERVE_ADDRESS", ""),
            &get_env_var("RESERVE_LIQUIDITY_MINT", ""),
            &get_env_var("RESERVE_SOURCE_LIQUIDITY", ""),
            &get_env_var("RESERVE_LIQUIDITY_FEE_RECEIVER", ""),
            &get_env_var("REFERER_TOKEN_STATE", ""),
            &get_env_var("REFERER_ACCOUNT", "")
        ).await?;
        
        // Fixed flash loan amount (10,000 SOL in lamports)
        let flash_amount: u64 = 10000000000000;
        
        // Create flash borrow and repay args
        let borrow_args = FlashBorrowReserveLiquidityArgs {
            amount: flash_amount,
        };

        let repay_args = FlashRepayReserveLiquidityArgs {
            amount: flash_amount,
            borrow_instruction_index: 0, // This will be adjusted in execute_arbitrage
        };
        
        // Create flash loan instructions
        let borrow_ix = create_flash_borrow_instruction(&flash_loan_ctx, &borrow_args)?;
        let repay_ix = create_flash_repay_instruction(&flash_loan_ctx, &repay_args)?;
        
        // Create or get flash loan lookup table
        let lookup_table_addr = create_or_get_flash_loan_lookup_table(&self.solana_client).await?;
        
        // Save the instructions, context, and lookup table address
        self.flash_borrow_instruction = Some(borrow_ix);
        self.flash_repay_instruction = Some(repay_ix);
        self.flash_loan_ctx = Some(flash_loan_ctx);
        self.flash_loan_lookup_table = Some(lookup_table_addr);
        
        // Record elapsed time
        let elapsed = start_time.elapsed();
        info!("Flash loan instructions created successfully for fixed amount of 10,000 SOL in {:?}", elapsed);
        
        Ok(())
    }
    

    async fn execute_transaction(&self, opportunity: &ArbitrageOpportunity, token_account_lookup_table: &str) -> Result<String> {
        // Start timing for performance measurement
        let start_time = std::time::Instant::now();
        let mut ata_creator_tx = Transaction::new_with_payer(&[], Some(&self.solana_client.wallet_pubkey()));

        // Log details about the transaction configuration from environment
        if self.config.use_jito_bundle {
            info!("  Jito tip amount: {} lamports ({:.6} SOL)", 
                self.config.jito_tip_lamports,
                self.config.jito_tip_lamports as f64 / 1_000_000_000.0);
        }

        // Log DEX routes from quotes
        info!("A -> B Route ({} -> {}):", 
            opportunity.token_a.symbol, 
            opportunity.token_b.symbol);
        
        for (i, route) in opportunity.quote_a_to_b.route_plan.iter().enumerate() {
            // Fix percentage calculation - percent value is already in correct units
            let percentage = route.percent as f32;
            info!("  [{}] {}% via {} (AMM: {})", 
                i + 1,
                percentage, 
                route.swap_info.label,
                route.swap_info.amm_key);
        }
        
        info!("B -> A Route ({} -> {}):", 
            opportunity.token_b.symbol, 
            opportunity.token_a.symbol);
        
        for (i, route) in opportunity.quote_b_to_a.route_plan.iter().enumerate() {
            // Fix percentage calculation - percent value is already in correct units
            let percentage = route.percent as f32;
            info!("  [{}] {}% via {} (AMM: {})", 
                i + 1,
                percentage, 
                route.swap_info.label,
                route.swap_info.amm_key);
        }

        if *IS_CHECK_TOKEN_EXIST {
            // Ensure all required token accounts exist
            ata_creator_tx = self.ensure_token_accounts_for_arbitrage(opportunity).await?;
        }
        
        // Check for errors in both futures
        let swap_instructions_a_to_b = swap_instructions_a_to_b?;
        let swap_instructions_b_to_a = swap_instructions_b_to_a?;

        let mut combined_accounts = Vec::new();
        let accounts_a_to_b: Result<Vec<_>> = swap_instructions_a_to_b.swap_instruction.accounts
            .iter()
            .skip(9) 
            .map(|a| convert_to_account_meta(a))
            .collect();
        
        let accounts_b_to_a: Result<Vec<_>> = swap_instructions_b_to_a.swap_instruction.accounts
            .iter()
            .skip(9) 
            .map(|a| convert_to_account_meta(a))
            .collect();

        let accounts_a_to_b = accounts_a_to_b?;
        let accounts_b_to_a = accounts_b_to_a?;
            
        // Extend combined_accounts with the vectors
        combined_accounts.extend(accounts_a_to_b.clone());
        combined_accounts.extend(accounts_b_to_a.clone());

        let is_use_combined_ix = check_is_use_combined_ix(
            &opportunity.quote_a_to_b,
            &opportunity.quote_b_to_a,
        );
        
        // Use a more descriptive log message
        if is_use_combined_ix {
            info!("Using COMBINED instruction mode: YES (eligible AMM types detected)");
        } else {
            info!("Using COMBINED instruction mode: NO (incompatible AMM types)");
        }
        
        // Get recent blockhash
        let recent_blockhash = self.solana_client.get_latest_blockhash().await?;
        info!("Got recent blockhash: {:?}", recent_blockhash);
        
        // Add flash loan lookup table if flash loan is used
        if is_use_flash_loan {
            if let Some(lookup_table_addr_str) = &self.flash_loan_lookup_table {
                info!("Adding flash loan address lookup table: {}", lookup_table_addr_str);
                
                let address = Pubkey::from_str(lookup_table_addr_str)?;
                
                // Fetch and deserialize lookup table
                match self.solana_client.get_account(&address).await {
                    Ok(account) => {
                        match AddressLookupTable::deserialize(&account.data) {
                            Ok(lookup_table) => {
                                lookup_tables.push(AddressLookupTableAccount {
                                    key: address,
                                    addresses: lookup_table.addresses.to_vec(),
                                });
                                info!("Successfully added flash loan lookup table with {} addresses", 
                                      lookup_table.addresses.len());
                            },
                            Err(e) => {
                                warn!("Failed to deserialize flash loan lookup table: {}", e);
                            }
                        }
                    },
                    Err(e) => {
                        warn!("Failed to fetch flash loan lookup table account: {}", e);
                    }
                }
            } else {
                warn!("Flash loan is enabled but no lookup table address is available");
            }
        }

        // Check if Jito bundles are enabled
        let is_use_jito = true;
        // let is_use_jito = self.config.use_jito_bundle;
        
        // Check if we should combine Jito instructions
        let combine_jito_instructions = self.config.combine_jito_instructions;
        
        // If we're using Jito and want to combine instructions, add the tip instruction
        if is_use_jito && combine_jito_instructions {
            info!("Adding Jito tip instruction directly to the transaction...");
            
            // Add the Jito tip instruction
            let tip_account = jito::get_jito_tip_account()?;
            let tip_ix = system_instruction::transfer(
                &self.solana_client.wallet_pubkey(),
                &tip_account,
                self.config.jito_tip_lamports
            );
            
            // For better priority, add the tip at the beginning of the instructions
            instructions.insert(2, tip_ix); // Insert after compute budget instructions
            
            info!("Added Jito tip instruction of {} lamports to {} account", 
                self.config.jito_tip_lamports, 
                tip_account.to_string());
        }

        // Record elapsed time
        let elapsed = start_time.elapsed();
        info!("Transaction execution took {:?}", elapsed);

        // Get recent blockhash for transaction
        let recent_blockhash = self.solana_client.get_latest_blockhash().await?;

        // Create and sign transaction
        let transaction = Transaction::new_signed_with_payer(
            &instructions,
            Some(&self.solana_client.wallet_pubkey()),
            &[self.solana_client.get_keypair()],
            recent_blockhash
        );

        // Create versioned transaction with lookup tables
        let tx = create_versioned_transaction(
            &self.solana_client,
            instructions,
            lookup_tables,
            recent_blockhash,
        )?;

        // log instructions length
        info!("Instructions length: {}", tx.message.instructions().len());
        
    }

    // Add this function to properly check if a token account exists before creating it
    pub async fn ensure_token_accounts_for_arbitrage(
        &self, 
        opportunity: &ArbitrageOpportunity
    ) -> Result<Transaction> {
        let wallet_pubkey = self.solana_client.wallet_pubkey();
        
        // Get all unique token mints involved in the arbitrage
        let mut unique_mints = HashSet::new();
        
        // If there's a multi-step route, add intermediate tokens
        for step in &opportunity.quote_a_to_b.route_plan {
            unique_mints.insert(step.swap_info.input_mint.clone());
            unique_mints.insert(step.swap_info.output_mint.clone());
        }
        
        for step in &opportunity.quote_b_to_a.route_plan {
            unique_mints.insert(step.swap_info.input_mint.clone());
            unique_mints.insert(step.swap_info.output_mint.clone());
        }
        
        // Only make RPC calls for mints not in the cache
        if !needs_checking.is_empty() {
            for mint in needs_checking {
                let mint_pubkey = Pubkey::from_str(&mint)?;
                let token_account = get_associated_token_address(&wallet_pubkey, &mint_pubkey);
                
                // Check if account exists, create if needed
                let account_exists = self.solana_client.get_account(&token_account).await.is_ok();
                
                if !account_exists {
                    info!("Creating token account for mint: {}", mint);
                    let create_ata_ix = spl_associated_token_account::instruction::create_associated_token_account(
                        &wallet_pubkey,
                        &wallet_pubkey,
                        &mint_pubkey,
                        &spl_token::id(),
                    );
                    instructions.push(create_ata_ix);
                    accounts_created = true;
                }
                
                // Add to cache regardless of whether it exists or we're creating it
                add_to_ata_cache(&mint_pubkey);
                cache_modified = true;
            }
        }
    }
}

// Define the ArbitrageOpportunity struct at the top level
#[derive(Debug)]
pub struct ArbitrageOpportunity {
    pub token_a: Token,
    pub token_b: Token,
    pub quote_a_to_b: Quote,
    pub quote_b_to_a: Quote,
    pub profit_lamports: i64,
    pub profit_percentage: f64,
    pub trade_amount: String,
    pub manufactured_quotes: bool,  // New field to track if quotes were manufactured
}

// Define the ArbitrageResult struct at the top level
pub struct ArbitrageResult {
    pub signature: Option<String>,
    pub error: Option<String>,
    pub profit_percentage: f64,
    pub success: bool,
    pub input_amount: u64,
    pub output_amount: u64,
    pub simulated: bool,
}

// Helper function to convert AccountData to AccountMeta
fn convert_to_account_meta(account: &AccountData) -> Result<AccountMeta> {
    Ok(AccountMeta {
        pubkey: Pubkey::from_str(&account.pubkey)?,
        is_signer: account.is_signer,
        is_writable: account.is_writable,
    })
}


fn check_is_use_combined_ix(
    quote_a_to_b: &Quote,
    quote_b_to_a: &Quote,
) -> bool {
    // let checklist = Vec::from(["Stabble Weighted Swap", "Raydium", "Meteora DLMM", "Lifinity V2", "whirlpool"]);
    let checklist = Vec::from(["Stabble Weighted Swap", "Raydium", "Raydium CLMM","Meteora DLMM", "Lifinity V2", "whirlpool", "Lifinity", "Saber", "Perps", "Meteora" , "Cropper", "1DEX", "ZeroFi" , "Stabble Stable Swap", "OpenBook V2" , "Pump.fun Amm" , "SolFi", ]);
    
    // Check if all route plans in quote_a_to_b are in the checklist
    let all_a_to_b_in_checklist = quote_a_to_b.route_plan.iter()
        .all(|route_plan| checklist.contains(&route_plan.swap_info.label.as_str()));
    
    // Check if all route plans in quote_b_to_a are in the checklist
    let all_b_to_a_in_checklist = quote_b_to_a.route_plan.iter()
        .all(|route_plan| checklist.contains(&route_plan.swap_info.label.as_str()));
    
    // Only return true if all route plans from both quotes are in the checklist
    all_a_to_b_in_checklist && all_b_to_a_in_checklist
}

fn create_versioned_transaction(
    solana_client: &SolanaClient,
    instructions: Vec<Instruction>,
    address_lookup_tables: Vec<AddressLookupTableAccount>,
    recent_blockhash: Hash,
) -> Result<VersionedTransaction> {
    info!("Creating versioned transaction with {} instructions", instructions.len());
    
    // Create v0 message
    let message = MessageV0::try_compile(
        &solana_client.wallet_pubkey(),
        &instructions,
        &address_lookup_tables,
        recent_blockhash,
    )?;

    // Get the keypair for signing
    let signer = solana_client.get_keypair();
    info!("Using signer pubkey: {}", signer.pubkey());
    
    // Create and sign versioned transaction
    let tx = VersionedTransaction::try_new(
        VersionedMessage::V0(message),
        &[signer],
    )?;
    
    info!("Created and signed versioned transaction");
    info!("- Number of signatures: {}", tx.signatures.len());
    
    Ok(tx)
}

// Helper function to save ATA cache to file
pub fn save_ata_cache_to_file(cache: &AtaCache) -> Result<()> {
    let path = Path::new(ATA_CACHE_FILE);
    
    // Serialize to JSON
    let json = serde_json::to_string_pretty(cache)?;
    
    // Write to file
    let mut file = File::create(path)?;
    file.write_all(json.as_bytes())?;
    
    info!("Saved ATA cache to file with {} entries", cache.accounts.len());
    
    Ok(())
}

// Initialize the ATA cache - either load from file or build from scratch
pub async fn initialize_ata_cache(solana_client: &Arc<SolanaClient>) -> Result<()> {
    let path = Path::new(ATA_CACHE_FILE);
    
    // Try to load from file first
    if path.exists() {
        match load_ata_cache_from_file() {
            Ok(cache) => {
                let mut ata_cache = ATA_CACHE.lock().unwrap();
                *ata_cache = cache.accounts;
                info!("Loaded ATA cache from file with {} entries", ata_cache.len());
                return Ok(());
            },
            Err(e) => {
                warn!("Failed to load ATA cache from file: {}", e);
                // Continue to build from scratch
            }
        }
    }
    
    // If we couldn't load from file, build the cache from scratch
    info!("Building ATA cache from scratch...");
    let wallet_pubkey = solana_client.wallet_pubkey();
    
    // Get all token accounts for the wallet
    match solana_client.get_token_accounts(&wallet_pubkey).await {
        Ok(token_accounts) => {
            let mut ata_cache = ATA_CACHE.lock().unwrap();
            
            for account in token_accounts {
                if let Some(mint) = account.mint {
                    if let Ok(mint_pubkey) = Pubkey::from_str(&mint) {
                        ata_cache.insert(mint_pubkey, true);
                    }
                }
            }
            
            info!("Built ATA cache with {} entries", ata_cache.len());
            
            // Save the cache to file
            let cache = AtaCache {
                accounts: ata_cache.clone(),
            };
            
            if let Err(e) = save_ata_cache_to_file(&cache) {
                warn!("Failed to save ATA cache to file: {}", e);
            }
            
            Ok(())
        },
        Err(e) => {
            error!("Failed to get token accounts: {}", e);
            Err(anyhow!("Failed to initialize ATA cache: {}", e))
        }
    }
}

// Helper function to load ATA cache from file
pub fn load_ata_cache_from_file() -> Result<AtaCache> {
    let path = Path::new(ATA_CACHE_FILE);
    
    // Read file
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    
    // Deserialize JSON
    let cache: AtaCache = serde_json::from_str(&contents)?;
    
    Ok(cache)
}
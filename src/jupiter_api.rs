use serde::{Deserialize, Serialize};
use reqwest::Client;
use std::time::{Duration, Instant};
use anyhow::{Result, anyhow};
use log::{info, warn, debug, error};
use std::sync::{Arc, Mutex};
use std::cmp::min;
use crate::solana::SolanaClient;
use crate::rate_limiter::RateLimiter;
use lazy_static::lazy_static;
use std::collections::HashMap;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub tokens: Vec<Token>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub address: String,
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    #[serde(default)]
    pub logo_uri: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub daily_volume: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    #[serde(default)]
    #[serde(alias = "inputMint")]
    pub input_mint: String,
    #[serde(default)]
    #[serde(alias = "outputMint")]
    pub output_mint: String,
    #[serde(default)]
    #[serde(alias = "inAmount")]
    pub in_amount: String,
    #[serde(default)]
    #[serde(alias = "outAmount")]
    pub out_amount: String,
    #[serde(default)]
    #[serde(alias = "otherAmountThreshold")]
    pub other_amount_threshold: String,
    #[serde(default)]
    #[serde(alias = "swapMode")]
    pub swap_mode: String,
    #[serde(alias = "slippageBps")]
    pub slippage_bps: u64,
    #[serde(default)]
    #[serde(alias = "platformFee")]
    pub platform_fee: Option<PlatformFee>,
    #[serde(default)]
    #[serde(alias = "priceImpactPct")]
    pub price_impact_pct: String,
    #[serde(default)]
    #[serde(alias = "routePlan")]
    pub route_plan: Vec<RoutePlanInfo>,
    #[serde(default)]
    #[serde(alias = "contextSlot")]
    pub context_slot: u64,
    #[serde(default)]
    pub address_lookup_tables: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformFee {
    pub amount: String,
    #[serde(alias = "feeBps")]
    pub fee_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePlanInfo {
    #[serde(alias = "swapInfo")]
    pub swap_info: SwapInfo,
    pub percent: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapInfo {
    pub label: String,
    #[serde(alias = "ammKey")]
    pub amm_key: String,
    #[serde(alias = "inputMint")]
    pub input_mint: String,
    #[serde(alias = "outputMint")]
    pub output_mint: String,
    #[serde(alias = "inAmount")]
    pub in_amount: String,
    #[serde(alias = "outAmount")]
    pub out_amount: String,
    #[serde(alias = "feeAmount")]
    pub fee_amount: String,
    #[serde(alias = "feeMint")]
    pub fee_mint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapTransaction {
    #[serde(rename = "swapTransaction")]
    pub swap_transaction: String,
    #[serde(rename = "lastValidBlockHeight", default)]
    pub last_valid_block_height: Option<u64>,
    #[serde(rename = "prioritizationFeeLamports", default)]
    pub prioritization_fee_lamports: Option<u64>,
    #[serde(rename = "computeUnitLimit", default)]
    pub compute_unit_limit: Option<u64>,
    #[serde(rename = "prioritizationType", default)]
    pub prioritization_type: Option<PrioritizationType>,
    #[serde(rename = "addressesByLookupTableAddress", default)]
    pub addresses_by_lookup_table_address: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrioritizationType {
    #[serde(rename = "computeBudget", default)]
    pub compute_budget: Option<ComputeBudget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeBudget {
    #[serde(default)]
    pub microlamports: Option<u64>,
    #[serde(rename = "estimatedMicroLamports", default)]
    pub estimated_microlamports: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicSlippageReport {
    #[serde(rename = "slippageBps", default)]
    pub slippage_bps: Option<u64>,
    #[serde(rename = "otherAmount", default)]
    pub other_amount: Option<String>,
    #[serde(rename = "simulatedIncurredSlippageBps", default)]
    pub simulated_incurred_slippage_bps: Option<i64>,
    #[serde(rename = "amplificationRatio", default)]
    pub amplification_ratio: Option<String>,
    #[serde(rename = "categoryName", default)]
    pub category_name: Option<String>,
    #[serde(rename = "heuristicMaxSlippageBps", default)]
    pub heuristic_max_slippage_bps: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationError {
    #[serde(rename = "errorCode")]
    pub error_code: String,
    pub error: String,
}

// Add a new struct for the swap instructions API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapInstructions {
    #[serde(rename = "setupInstructions", default)]
    pub setup_instructions: Option<Vec<InstructionData>>,
    
    #[serde(rename = "swapInstruction")]
    pub swap_instruction: InstructionData,
    
    #[serde(rename = "addressLookupTableAddresses", default)]
    pub address_lookup_table_addresses: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionData {
    #[serde(rename = "programId")]
    pub program_id: String,
    
    pub accounts: Vec<AccountData>,
    
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountData {
    pub pubkey: String,
    #[serde(rename = "isSigner")]
    pub is_signer: bool,
    #[serde(rename = "isWritable")]
    pub is_writable: bool,
}

// Add a new struct for batch quote requests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteRequest {
    #[serde(rename = "inputMint")]
    pub input_mint: String,
    #[serde(rename = "outputMint")]
    pub output_mint: String,
    #[serde(rename = "amount")]
    pub amount: String,
    #[serde(rename = "slippageBps")]
    pub slippage_bps: u64,
}

// Add key types for caching
type QuoteCacheKey = (String, String, u64, u64); // (input_mint, output_mint, amount, slippage_bps)
type SwapInstructionsCacheKey = (String, String, String, String, u64); // (input_mint, output_mint, in_amount, out_amount, slippage_bps)

// Structure for serialized cache to save to file
#[derive(Serialize, Deserialize)]
struct SerializedInstructionsCache {
    entries: Vec<(SwapInstructionsCacheKey, SwapInstructions)>,
}

// Global counter for API requests across all threads
lazy_static! {
    static ref GLOBAL_REQUEST_COUNT: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    static ref LAST_LOG_TIME: Arc<Mutex<Instant>> = Arc::new(Mutex::new(Instant::now()));
    static ref THREAD_REQUEST_COUNTS: Arc<Mutex<HashMap<usize, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    static ref ENDPOINT_REQUEST_COUNTS: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    // Add cache-related timestamps
    static ref LAST_CACHE_SAVE_TIME: Arc<Mutex<Instant>> = Arc::new(Mutex::new(Instant::now()));
    static ref CACHE_MODIFIED: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    static ref CACHE_SAVE_INTERVAL_SECS: Arc<AtomicU64> = Arc::new(AtomicU64::new(300)); // Default 5 minutes (300 seconds)
}

// Add a new struct for batch swap instructions request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapInstructionsRequest {
    #[serde(rename = "quoteResponse")]
    pub quote_response: Quote,
    #[serde(rename = "userPublicKey")]
    pub user_public_key: String,
    #[serde(rename = "wrapAndUnwrapSol")]
    pub wrap_and_unwrap_sol: bool,
    #[serde(rename = "useSharedAccounts")]
    pub use_shared_accounts: bool,
    #[serde(rename = "dynamicComputeUnitLimit")]
    pub dynamic_compute_unit_limit: bool,
}

pub struct JupiterClient {
    client: Client,
    max_retries: u32,
    rate_limiter: Option<Arc<RateLimiter>>,
    solana_client: Option<Arc<SolanaClient>>,
    current_thread_id: Arc<Mutex<usize>>,
    quote_cache: Arc<Mutex<LruCache<QuoteCacheKey, Quote>>>,
    instructions_cache: Arc<Mutex<LruCache<SwapInstructionsCacheKey, SwapInstructions>>>,
    cache_file_path: String,
}

impl JupiterClient {
    pub fn new(max_retries: u32) -> Self {
        // Configure HTTP client with optimized settings
        let client = Client::builder()
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .pool_max_idle_per_host(100)  // Increase connection pooling
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| {
                warn!("Failed to build optimized HTTP client, using default");
                Client::new()
            });
        
        // Create NonZero values for cache sizes
        let quote_cache_size = NonZeroUsize::new(100).unwrap();
        let instructions_cache_size = NonZeroUsize::new(50).unwrap();
        
        // Default cache file path
        let cache_file_path = "instructions_cache.json".to_string();
        
        // Create a new JupiterClient
        let mut jupiter_client = Self {
            client,
            max_retries,
            rate_limiter: None,
            solana_client: None,
            current_thread_id: Arc::new(Mutex::new(0)),
            // Initialize LRU caches with proper NonZero sizes
            quote_cache: Arc::new(Mutex::new(LruCache::new(quote_cache_size))),
            instructions_cache: Arc::new(Mutex::new(LruCache::new(instructions_cache_size))),
            cache_file_path,
        };
        
        // Load cache from file if it exists
        jupiter_client.load_instructions_cache();
        
        jupiter_client
    }
    
    // Load the instructions cache from file
    fn load_instructions_cache(&self) {
        let path = Path::new(&self.cache_file_path);
        if !path.exists() {
            info!("Instructions cache file does not exist. Starting with empty cache.");
            return;
        }
        
        match fs::read_to_string(path) {
            Ok(cache_str) => {
                match serde_json::from_str::<SerializedInstructionsCache>(&cache_str) {
                    Ok(serialized_cache) => {
                        let mut cache = self.instructions_cache.lock().unwrap();
                        for (key, value) in serialized_cache.entries {
                            cache.put(key, value);
                        }
                        info!("Successfully loaded instructions cache from file with {} entries", cache.len());
                    },
                    Err(e) => {
                        warn!("Failed to deserialize instructions cache from file: {}", e);
                    }
                }
            },
            Err(e) => {
                warn!("Failed to read instructions cache file: {}", e);
            }
        }
    }
    
    // Modify save_instructions_cache to be public
    pub fn save_instructions_cache(&self) {
        // Get current time
        let now = Instant::now();
        let mut last_save_time = LAST_CACHE_SAVE_TIME.lock().unwrap();
        
        // Check if cache was modified
        if !CACHE_MODIFIED.load(Ordering::SeqCst) {
            return;
        }
        
        // Check if enough time has passed since last save
        let save_interval = Duration::from_secs(CACHE_SAVE_INTERVAL_SECS.load(Ordering::SeqCst));
        if now.duration_since(*last_save_time) < save_interval {
            // Not time to save yet
            return;
        }
        
        let cache = self.instructions_cache.lock().unwrap();
        
        // If cache is empty, don't save
        if cache.len() == 0 {
            return;
        }
        
        // Convert LruCache to a Vec of entries for serialization
        let entries: Vec<(SwapInstructionsCacheKey, SwapInstructions)> = cache
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        
        // Store the length before moving entries
        let entries_len = entries.len();
        
        let serialized_cache = SerializedInstructionsCache { entries };
        
        match serde_json::to_string(&serialized_cache) {
            Ok(cache_str) => {
                match fs::write(&self.cache_file_path, cache_str) {
                    Ok(_) => {
                        info!("Successfully saved instructions cache to file with {} entries", entries_len);
                        // Update last save time and reset modified flag
                        *last_save_time = now;
                        CACHE_MODIFIED.store(false, Ordering::SeqCst);
                    },
                    Err(e) => {
                        warn!("Failed to write instructions cache to file: {}", e);
                    }
                }
            },
            Err(e) => {
                warn!("Failed to serialize instructions cache: {}", e);
            }
        }
    }
    
    // Add a method to check if cache should be saved
    fn check_and_save_cache(&self) {
        let now = Instant::now();
        let last_save_time = LAST_CACHE_SAVE_TIME.lock().unwrap();
        
        // Check if enough time has passed since last save
        let save_interval = Duration::from_secs(CACHE_SAVE_INTERVAL_SECS.load(Ordering::SeqCst));
        if now.duration_since(*last_save_time) >= save_interval && CACHE_MODIFIED.load(Ordering::SeqCst) {
            // Release lock before saving
            drop(last_save_time);
            self.save_instructions_cache();
        }
    }
    
    // Helper method to get the Jupiter API key from environment
    fn get_api_key(&self) -> String {
        std::env::var("JUPITER_API_KEY").unwrap_or_else(|_| {
            warn!("JUPITER_API_KEY not found in environment, using default");
            "b5e5d25a-7897-47c2-a963-75975aa2c7dd".to_string()
        })
    }
    
    pub fn with_solana_client(mut self, solana_client: Arc<SolanaClient>) -> Self {
        self.solana_client = Some(solana_client);
        self
    }
    
    pub fn with_rate_limiter(mut self, rate_limiter: Arc<RateLimiter>) -> Self {
        self.rate_limiter = Some(rate_limiter);
        self
    }
    
    // Method to set custom cache file path
    pub fn with_cache_file_path(mut self, cache_file_path: String) -> Self {
        self.cache_file_path = cache_file_path;
        self.load_instructions_cache(); // Reload cache with new path
        self
    }
    
    // Set the rate limit based on Jupiter API level
    pub fn set_rate_limit(&self, api_level: u64) {
        let rpm = match api_level {
            0 => 60,      // Free tier: 1 RPS / 60 RPM
            1 => 600,     // 10 RPS / 600 RPM
            2 => 3000,    // 50 RPS / 3000 RPM
            3 => 6000,    // 100 RPS / 6000 RPM
            4 => 30000,   // 500 RPS / 30000 RPM
            _ => 60,      // Default to free tier
        };
        
        // Pass the API level to the rate limiter if it exists
        if let Some(rate_limiter) = &self.rate_limiter {
                rate_limiter.set_api_level(api_level);
        }
        
        info!("Jupiter API rate limit set to {} requests per minute (Level {})", rpm, api_level);
    }
    
    // Add a method to set the current thread ID
    pub fn set_thread_id(&self, thread_id: usize) {
        let mut current_id = self.current_thread_id.lock().unwrap();
        *current_id = thread_id;
    }
    
    fn check_rate_limit(&self, endpoint: &str) -> Result<()> {
        // Update global counter for stats
        let mut global_count = GLOBAL_REQUEST_COUNT.lock().unwrap();
        *global_count += 1;
        
        // Update thread-specific counter
        let thread_id = *self.current_thread_id.lock().unwrap();
        let mut thread_counts = THREAD_REQUEST_COUNTS.lock().unwrap();
        *thread_counts.entry(thread_id).or_insert(0) += 1;
        
        // Update endpoint-specific counter
        let mut endpoint_counts = ENDPOINT_REQUEST_COUNTS.lock().unwrap();
        *endpoint_counts.entry(endpoint.to_string()).or_insert(0) += 1;
        
        // Log global stats every 1000 requests
        if *global_count % 1000 == 0 {
            let mut last_log = LAST_LOG_TIME.lock().unwrap();
            let elapsed = last_log.elapsed();
            let rate = 1000.0 / elapsed.as_secs_f64();
            
            info!("API request stats: {} total requests, {:.2} requests/second", 
                *global_count, rate);
            
            // Log thread distribution
            info!("Thread distribution:");
            for (tid, count) in thread_counts.iter() {
                let percentage = (*count as f64 / *global_count as f64) * 100.0;
                info!("  Thread {}: {} calls ({:.1}%)", tid, count, percentage);
            }
            
            // Log endpoint distribution
            info!("Endpoint distribution:");
            for (ep, count) in endpoint_counts.iter() {
                let percentage = (*count as f64 / *global_count as f64) * 100.0;
                info!("  {}: {} calls ({:.1}%)", ep, count, percentage);
            }
                
            *last_log = Instant::now();
        }
        
        // If we have a rate limiter, use it with the current thread ID and endpoint
        if let Some(rate_limiter) = &self.rate_limiter {
            let thread_id = *self.current_thread_id.lock().unwrap();
            if rate_limiter.is_limited() || !rate_limiter.increment_request_count_for_thread(thread_id, Some(endpoint)) {
                let time_to_wait = rate_limiter.get_current_delay();
                warn!("Rate limited by RateLimiter for thread {} on endpoint {}. Current delay: {}ms", 
                      thread_id, endpoint, time_to_wait);
                return Err(anyhow!("Rate limit exceeded. Try again later"));
            }
        }
        
        Ok(())
    }
    
    pub async fn get_quote(&self, input_mint: &str, output_mint: &str, amount: u64, slippage_bps: u64) -> Result<Quote> {
        let thread_id = *self.current_thread_id.lock().unwrap();
        
        // Check if we're in a backoff period
        if let Some(rate_limiter) = &self.rate_limiter {
            if rate_limiter.is_in_backoff(thread_id) {
                let delay = rate_limiter.get_backoff_delay();
                warn!("Thread {} in backoff period, waiting {}ms before fetching quote", thread_id, delay);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
        
        self.check_rate_limit("quote")?;
        
        let url = format!(
            "https://api.jup.ag/swap/v1/quote?inputMint={}&outputMint={}&amount={}&slippageBps={}",
            input_mint, output_mint, amount, slippage_bps
        );
        
        let api_key = self.get_api_key();
        
        let mut retries = 0;
        loop {
            // Build the request with API key
            let response = self.client
                .get(&url)
                .header("x-api-key", &api_key)
                .send()
                .await;

            match response {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let response_text = resp.text().await?;
                        match serde_json::from_str::<Quote>(&response_text) {
                            Ok(quote) => {
                                let mut quote = quote;
                                if quote.input_mint.is_empty() {
                                    quote.input_mint = input_mint.to_string();
                                }
                                if quote.output_mint.is_empty() {
                                    quote.output_mint = output_mint.to_string();
                                }
                                return Ok(quote);
                            },
                            Err(e) => {
                                error!("Failed to parse quote response: {}", e);
                                error!("Response text: {}", &response_text);
                                if retries >= self.max_retries {
                                    return Err(anyhow!("Failed to parse API response after {} retries", retries));
                                }
                            }
                        }
                    } else {
                        let status = resp.status();
                        let error_text = resp.text().await.unwrap_or_default();
                        error!("API error: {} - {}", status, error_text);
                        
                        // Handle 429 rate limit errors specifically
                        if status.as_u16() == 429 {
                            error!("Rate limit (429) error received from Jupiter API (unexpected with paid 50 req/sec API)");
                            
                            // Use minimal backoff strategy for actual API rate limits
                            let backoff_ms = 500 * retries as u64 + 100; // Starts at 600ms, then 1100ms, 1600ms etc
                            error!("API returned 429 error, backing off for {}ms before retry", backoff_ms);
                            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                            
                            retries += 1;
                            continue;
                        }
                        
                        if retries >= self.max_retries {
                            return Err(anyhow!("API error after {} retries: {}", retries, status));
                        }
                    }
                },
                Err(e) => {
                    error!("Request error: {}", e);
                    if retries >= self.max_retries {
                        return Err(anyhow!("Request failed after {} retries: {}", retries, e));
                    }
                }
            }
            
            retries += 1;
            
            // Exponential backoff for retries
            let backoff_ms = 500 * 2u64.pow(retries as u32).min(10000);
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
    }
    
    pub async fn fetch_trending_tokens(&self) -> Result<Vec<Token>> {
        let thread_id = *self.current_thread_id.lock().unwrap();
        
        // Check if we're in a backoff period
        if let Some(rate_limiter) = &self.rate_limiter {
            if rate_limiter.is_in_backoff(thread_id) {
                let delay = rate_limiter.get_backoff_delay();
                warn!("Thread {} in backoff period, waiting {}ms before fetching tokens", thread_id, delay);
                tokio::time::sleep(Duration::from_millis(delay.min(delay))).await;
            }
        }
        
        self.check_rate_limit("tokens")?;
        
        let url = "https://tokens.jup.ag/tokens?tags=birdeye-trending";
        
        let api_key = self.get_api_key();
        
        let mut retries = 0;
        loop {
            match self.client.get(url)
                .header("x-api-key", &api_key)
                .send()
                .await {
                Ok(response) => {
                    if response.status().is_success() {
                        let response_text = response.text().await?;
                        info!("API Response: {}", &response_text[..min(200, response_text.len())]);
                        
                        match serde_json::from_str::<TokenResponse>(&response_text) {
                            Ok(token_response) => {
                                return Ok(token_response.tokens);
                            },
                            Err(e) => {
                                error!("Failed to parse token response: {}", e);
                                match serde_json::from_str::<Vec<Token>>(&response_text) {
                                    Ok(tokens) => {
                                        return Ok(tokens);
                                    },
                                    Err(e2) => {
                                        error!("Failed to parse as Vec<Token>: {}", e2);
                                        if retries >= self.max_retries {
                                            return Err(anyhow!("Failed to parse API response"));
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        let status = response.status();
                        let error_text = response.text().await.unwrap_or_default();
                        error!("API error: {} - {}", status, error_text);
                        
                        // Handle 429 rate limit errors specifically
                        if status.as_u16() == 429 {
                            error!("Rate limit (429) error received from Jupiter API");
                            
                            // Trigger exponential backoff via rate limiter
                            if let Some(rate_limiter) = &self.rate_limiter {
                                rate_limiter.handle_rate_limit_error(thread_id);
                                
                                // Wait for a short time before checking backoff status again
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                continue;
                            } else {
                                // If no rate limiter, use a simple backoff
                                let backoff = 5 * 2u64.pow(retries as u32);
                                error!("No rate limiter configured, backing off for {}s", backoff);
                                tokio::time::sleep(Duration::from_secs(backoff)).await;
                            }
                        }
                        
                        if retries >= self.max_retries {
                            return Err(anyhow!("API error after {} retries: {}", retries, status));
                        }
                    }
                },
                Err(e) => {
                    error!("Request error: {}", e);
                    if retries >= self.max_retries {
                        return Err(anyhow!("Request failed after {} retries: {}", retries, e));
                    }
                }
            }
            
            retries += 1;
            
            // Exponential backoff for retries
            let backoff_ms = 500 * 2u64.pow(retries as u32).min(10000);
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
    }
    
    pub fn get_usdc_token() -> Token {
        Token {
            address: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            symbol: "USDC".to_string(),
            name: "USD Coin".to_string(),
            decimals: 6,
            logo_uri: "https://raw.githubusercontent.com/solana-labs/token-list/main/assets/mainnet/EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v/logo.png".to_string(),
            tags: vec!["stablecoin".to_string()],
            daily_volume: None,
        }
    }
    
    pub fn get_wsol_token() -> Token {
        Token {
            address: "So11111111111111111111111111111111111111112".to_string(),
            symbol: "SOL".to_string(),
            name: "Wrapped SOL".to_string(),
            decimals: 9,
            logo_uri: "https://raw.githubusercontent.com/solana-labs/token-list/main/assets/mainnet/So11111111111111111111111111111111111111112/logo.png".to_string(),
            tags: vec!["wrapped-solana".to_string()],
            daily_volume: None,
        }
    }

    // Add a method to prewarm connections for faster initial requests
    pub async fn prewarm_connections(&self) -> Result<()> {
        info!("Prewarming connections to Jupiter API...");
        
        // Make a lightweight request to warm up the connection pool
        let urls = [
            "https://api.jup.ag/swap/v1/quote",
            "https://api.jup.ag/swap/v1/swap-instructions",
            "https://tokens.jup.ag/tokens",
        ];
        
        let api_key = self.get_api_key();
        
        for url in urls.iter() {
            match self.client.get(*url)
                .header("x-api-key", &api_key)
                .send()
                .await {
                Ok(_) => {
                    info!("Successfully prewarmed connection to {}", url);
                },
                Err(e) => {
                    warn!("Failed to prewarm connection to {}: {}", url, e);
                }
            }
        }
        
        Ok(())
    }
    
    // Add a cached version of get_quote that checks the cache first
    pub async fn get_quote_cached(&self, input_mint: &str, output_mint: &str, amount: u64, slippage_bps: u64) -> Result<Quote> {
        let cache_key = (input_mint.to_string(), output_mint.to_string(), amount, slippage_bps);
        
        // Check cache first
        {
            let mut cache = self.quote_cache.lock().unwrap();
            if let Some(quote) = cache.get(&cache_key) {
                info!("Cache hit for quote: {} -> {}", input_mint, output_mint);
                return Ok(quote.clone());
            }
        }
        
        // If not in cache, fetch from API
        let quote = self.get_quote(input_mint, output_mint, amount, slippage_bps).await?;
        
        // Store in cache
        {
            let mut cache = self.quote_cache.lock().unwrap();
            cache.put(cache_key, quote.clone());
        }
        
        Ok(quote)
    }
    
    // Precompute the swap instructions request body to avoid repeated serialization
    fn precompute_swap_instructions_body(&self, quote: &Quote, user_public_key: &str) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "quoteResponse": {
                "inputMint": quote.input_mint,
                "inAmount": quote.in_amount,
                "outputMint": quote.output_mint,
                "outAmount": quote.out_amount,
                "otherAmountThreshold": quote.other_amount_threshold,
                "swapMode": quote.swap_mode,
                "slippageBps": quote.slippage_bps,
                "priceImpactPct": quote.price_impact_pct,
                "routePlan": quote.route_plan.iter().map(|rp| {
                    serde_json::json!({
                        "swapInfo": {
                            "ammKey": rp.swap_info.amm_key,
                            "label": rp.swap_info.label,
                            "inputMint": rp.swap_info.input_mint,
                            "outputMint": rp.swap_info.output_mint,
                            "inAmount": rp.swap_info.in_amount,
                            "outAmount": rp.swap_info.out_amount,
                            "feeAmount": rp.swap_info.fee_amount,
                            "feeMint": rp.swap_info.fee_mint
                        },
                        "percent": rp.percent
                    })
                }).collect::<Vec<_>>(),
                "contextSlot": quote.context_slot
            },
            "userPublicKey": user_public_key,
            "wrapAndUnwrapSol": true,
            "useSharedAccounts": false,
            "dynamicComputeUnitLimit": true
        });
        
        Ok(body)
    }
    
    // Add a method to get swap instructions with precomputed request body
    pub async fn get_swap_instructions_with_body(&self, body: serde_json::Value) -> Result<SwapInstructions> {
        let thread_id = *self.current_thread_id.lock().unwrap();
        
        // Check if we're in a backoff period
        if let Some(rate_limiter) = &self.rate_limiter {
            if rate_limiter.is_in_backoff(thread_id) {
                let delay = rate_limiter.get_backoff_delay();
                warn!("Thread {} in backoff period, waiting {}ms before fetching swap instructions", thread_id, delay);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
        
        self.check_rate_limit("swap-instructions")?;
        
        let url = "https://api.jup.ag/swap/v1/swap-instructions";
        
        let api_key = self.get_api_key();
        
        let mut retries = 0;
        loop {
            // Build the request with API key
            let response = self.client
                .post(url)
                .header("x-api-key", &api_key)
                .json(&body)
                .send()
                .await;
            
            match response {
                Ok(resp) => {
                    if resp.status().is_success() {
                        match resp.json::<SwapInstructions>().await {
                            Ok(instructions) => {
                                return Ok(instructions);
                            },
                            Err(e) => {
                                error!("Failed to parse swap instructions response: {}", e);
                                if retries >= self.max_retries {
                                    return Err(anyhow!("Failed to parse API response after {} retries", retries));
                                }
                            }
                        }
                    } else {
                        let status = resp.status();
                        let error_text = resp.text().await.unwrap_or_default();
                        error!("API error: {} - {}", status, error_text);
                        
                        // Handle 429 rate limit errors specifically
                        if status.as_u16() == 429 {
                            error!("Rate limit (429) error received from Jupiter API (unexpected with paid 50 req/sec API)");
                            
                            // Use minimal backoff strategy for actual API rate limits
                            let backoff_ms = 500 * retries as u64 + 100; // Starts at 600ms, then 1100ms, 1600ms etc
                            error!("API returned 429 error, backing off for {}ms before retry", backoff_ms);
                            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                            
                            retries += 1;
                            continue;
                        }
                        
                        if retries >= self.max_retries {
                            return Err(anyhow!("API error after {} retries: {}", retries, status));
                        }
                    }
                },
                Err(e) => {
                    error!("Request error: {}", e);
                    if retries >= self.max_retries {
                        return Err(anyhow!("Request failed after {} retries: {}", retries, e));
                    }
                }
            }
            
            retries += 1;
            
            // Exponential backoff for retries
            let backoff_ms = 500 * 2u64.pow(retries as u32).min(10000);
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
    }
    
    // Add a method to get swap instructions
    pub async fn get_swap_instructions(&self, quote: &Quote, user_public_key: &str) -> Result<SwapInstructions> {
        let cache_key = (
            quote.input_mint.clone(),
            quote.output_mint.clone(),
            quote.in_amount.clone(),
            quote.out_amount.clone(),
            quote.slippage_bps
        );
        
        // Check cache first
        {
            let mut cache = self.instructions_cache.lock().unwrap();
            if let Some(instructions) = cache.get(&cache_key) {
                info!("Cache hit for swap instructions: {} -> {}", quote.input_mint, quote.output_mint);
                return Ok(instructions.clone());
            }
        }
        
        // Precompute request body for better performance
        let body = self.precompute_swap_instructions_body(quote, user_public_key)?;
        
        // Get instructions using precomputed body
        let instructions = self.get_swap_instructions_with_body(body).await?;
        
        // Store in cache
        {
            let mut cache = self.instructions_cache.lock().unwrap();
            cache.put(cache_key, instructions.clone());
            
            // Mark cache as modified but don't save immediately
            CACHE_MODIFIED.store(true, Ordering::SeqCst);
            
            // Check if it's time to save the cache
            drop(cache); // Release the lock before potentially saving
            self.check_and_save_cache();
        }
        
        Ok(instructions)
    }
    
    // Add a method to get multiple quotes in a single batch request
    pub async fn get_quotes_batch(&self, requests: &[QuoteRequest]) -> Result<Vec<Quote>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        
        let thread_id = *self.current_thread_id.lock().unwrap();
        
        // Check if we're in a backoff period
        if let Some(rate_limiter) = &self.rate_limiter {
            if rate_limiter.is_in_backoff(thread_id) {
                let delay = rate_limiter.get_backoff_delay();
                warn!("Thread {} in backoff period, waiting {}ms before fetching batch quotes", thread_id, delay);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
        
        self.check_rate_limit("quotes-batch")?;
        
        let url = "https://api.jup.ag/swap/v1/quotes";
        
        let api_key = self.get_api_key();
        
        let mut retries = 0;
        loop {
            // Build the request with API key
            let response = self.client
                .post(url)
                .header("x-api-key", &api_key)
                .json(&requests)
                .send()
                .await;
            
            match response {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let response_text = resp.text().await?;
                        match serde_json::from_str::<Vec<Quote>>(&response_text) {
                            Ok(quotes) => {
                                // Cache the results
                                {
                                    let mut cache = self.quote_cache.lock().unwrap();
                                    for (i, quote) in quotes.iter().enumerate() {
                                        if i < requests.len() {
                                            let request = &requests[i];
                                            let amount = request.amount.parse::<u64>().unwrap_or(0);
                                            let cache_key = (
                                                request.input_mint.clone(),
                                                request.output_mint.clone(),
                                                amount,
                                                request.slippage_bps
                                            );
                                            cache.put(cache_key, quote.clone());
                                        }
                                    }
                                }
                                
                                return Ok(quotes);
                            },
                            Err(e) => {
                                error!("Failed to parse batch quotes response: {}", e);
                                error!("Response text: {}", &response_text);
                                if retries >= self.max_retries {
                                    return Err(anyhow!("Failed to parse API response after {} retries", retries));
                                }
                            }
                        }
                    } else {
                        let status = resp.status();
                        let error_text = resp.text().await.unwrap_or_default();
                        error!("API error: {} - {}", status, error_text);
                        
                        // Handle 429 rate limit errors specifically
                        if status.as_u16() == 429 {
                            error!("Rate limit (429) error received from Jupiter API (unexpected with paid 50 req/sec API)");
                            
                            // Use minimal backoff strategy for actual API rate limits
                            let backoff_ms = 500 * retries as u64 + 100; // Starts at 600ms, then 1100ms, 1600ms etc
                            error!("API returned 429 error, backing off for {}ms before retry", backoff_ms);
                            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                            
                            retries += 1;
                            continue;
                        }
                        
                        if retries >= self.max_retries {
                            return Err(anyhow!("API error after {} retries: {}", retries, status));
                        }
                    }
                },
                Err(e) => {
                    error!("Request error: {}", e);
                    if retries >= self.max_retries {
                        return Err(anyhow!("Request failed after {} retries: {}", retries, e));
                    }
                }
            }
            
            retries += 1;
            
            // Exponential backoff for retries
            let backoff_ms = 500 * 2u64.pow(retries as u32).min(10000);
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
    }

    // Add a method to set the cache save interval
    pub fn set_cache_save_interval(&self, seconds: u64) {
        CACHE_SAVE_INTERVAL_SECS.store(seconds, Ordering::SeqCst);
        info!("Instruction cache save interval set to {} seconds", seconds);
    }

    // After get_swap_instructions method, add a new batch method
    pub async fn get_swap_instructions_batch(
        &self, 
        quotes: &[Quote], 
        user_public_key: &str
    ) -> Result<Vec<SwapInstructions>> {
        // If no quotes, return empty result
        if quotes.is_empty() {
            return Ok(Vec::new());
        }
        
        // Create a results vector
        let mut results = Vec::with_capacity(quotes.len());
        
        // If only one quote, just use the normal method
        if quotes.len() == 1 {
            let instructions = self.get_swap_instructions(&quotes[0], user_public_key).await?;
            results.push(instructions);
            return Ok(results);
        }
        
        // For multiple quotes, use parallel execution
        let thread_id = *self.current_thread_id.lock().unwrap();
        
        // Check if we're in a backoff period
        if let Some(rate_limiter) = &self.rate_limiter {
            if rate_limiter.is_in_backoff(thread_id) {
                let delay = rate_limiter.get_backoff_delay();
                warn!("Thread {} in backoff period, waiting {}ms before fetching batch swap instructions", 
                      thread_id, delay);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
        
        // Check cache for all quotes first
        let mut cache_hits = Vec::new();
        let mut cache_misses = Vec::new();
        
        for (i, quote) in quotes.iter().enumerate() {
            let cache_key = (
                quote.input_mint.clone(),
                quote.output_mint.clone(),
                quote.in_amount.clone(),
                quote.out_amount.clone(),
                quote.slippage_bps
            );
            
            // Check if in cache
            let mut has_cache_hit = false;
            {
                let mut cache = self.instructions_cache.lock().unwrap();
                if let Some(instructions) = cache.get(&cache_key) {
                    has_cache_hit = true;
                    cache_hits.push((i, instructions.clone()));
                }
            }
            
            if !has_cache_hit {
                cache_misses.push((i, quote));
            }
        }
        
        // If we have any cache misses, make API calls for them
        if !cache_misses.is_empty() {
            // Process all cache misses in parallel 
            let mut futures = Vec::new();
            
            for (_, quote) in &cache_misses {
                // Create a future for each instruction
                futures.push(self.get_swap_instructions(quote, user_public_key));
            }
            
            // Execute all futures in parallel
            let instruction_results = futures::future::join_all(futures).await;
            
            // Process results and update cache
            for ((i, quote), result) in cache_misses.iter().zip(instruction_results) {
                match result {
                    Ok(instructions) => {
                        // Update cache
                        let cache_key = (
                            quote.input_mint.clone(),
                            quote.output_mint.clone(),
                            quote.in_amount.clone(),
                            quote.out_amount.clone(),
                            quote.slippage_bps
                        );
                        
                        {
                            let mut cache = self.instructions_cache.lock().unwrap();
                            cache.put(cache_key, instructions.clone());
                            CACHE_MODIFIED.store(true, Ordering::SeqCst);
                        }
                        
                        // Store result with index
                        cache_hits.push((*i, instructions));
                    },
                    Err(e) => {
                        return Err(anyhow!("Failed to get swap instructions for quote {}: {}", i, e));
                    }
                }
            }
        }
        
        // Now sort the results by their original index
        cache_hits.sort_by_key(|(i, _)| *i);
        
        // Extract just the instructions
        results = cache_hits.into_iter().map(|(_, instructions)| instructions).collect();
        
        // Check if we need to save the cache
        self.check_and_save_cache();
        
        Ok(results)
    }
}

// Implement Drop trait to save cache when the client is dropped
impl Drop for JupiterClient {
    fn drop(&mut self) {
        if CACHE_MODIFIED.load(Ordering::SeqCst) {
            info!("Saving modified instructions cache before JupiterClient is dropped...");
            self.save_instructions_cache();
        } else {
            debug!("No changes to instruction cache, skipping save on drop");
        }
    }
}

impl Clone for JupiterClient {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            max_retries: self.max_retries,
            rate_limiter: self.rate_limiter.clone(),
            solana_client: self.solana_client.clone(),
            current_thread_id: self.current_thread_id.clone(),
            quote_cache: self.quote_cache.clone(),
            instructions_cache: self.instructions_cache.clone(),
            cache_file_path: self.cache_file_path.clone(),
        }
    }
}

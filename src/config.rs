use dotenv::dotenv;
use serde::{Deserialize, Serialize};
use std::env;
use log;
use crate::utils::{get_env_var, get_env_var_bool, get_env_var_number};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // Core Configuration
    pub private_key: String,
    pub default_rpc: String,
    pub alt_rpc_list: Vec<String>,
    
    // Trading Configuration
    pub wrap_unwrap_sol: bool,
    pub trade_size_sol: f64,
    pub trade_size_strategy: String,
    pub min_profit_threshold: f64,
    pub slippage_bps: u64,
    
    // Rate Limiting
    pub min_interval_ms: u64,
    pub token_rotation_interval_minutes: u64,
    pub jupiter_api_level: u64,
    pub machine_amount: u64,
    pub thread_amount: u64,
    pub api_interval_ms: u64,
    
    // Adaptive Settings
    pub adaptive_slippage: bool,
    pub is_use_swap_cpi: bool,
    
    // Add new field for current machine index
    pub current_machine_index: u64,
    
    // Token Rotation Mode
    // When true: Uses TokenRotator for managed rotation with history tracking
    // When false: Rotates sequentially through trending tokens on each scan
    pub is_use_token_rotation: bool,
    
    // Flash Loan Configuration
    pub use_flash_loan: bool,
    
    // Jito Configuration
    pub use_jito_bundle: bool,
    pub jito_tip_lamports: u64,
    pub jito_rpc_url: String,
    pub jito_block_engine_url: String,
    pub combine_jito_instructions: bool,
    
    // Cache Configuration
    pub cache_file_path: String,

    // Add new config options for quote manufacturing
    pub use_manufactured_quotes: bool,
    pub validate_large_quotes: bool,
    pub validation_threshold_sol: f64,
    pub price_impact_model: String,
    pub price_impact_factor: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            private_key: String::new(),
            default_rpc: "https://api.mainnet-beta.solana.com".to_string(),
            alt_rpc_list: Vec::new(),
            wrap_unwrap_sol: true,
            trade_size_sol: 0.1,
            trade_size_strategy: "fixed".to_string(),
            min_profit_threshold: 0.5,
            slippage_bps: 100,
            min_interval_ms: 3000,
            token_rotation_interval_minutes: 5,
            jupiter_api_level: 0,
            machine_amount: 1,
            thread_amount: 1,
            api_interval_ms: 1000,
            adaptive_slippage: false,
            is_use_swap_cpi: false,
            current_machine_index: 1,
            is_use_token_rotation: true,
            use_flash_loan: false,
            use_jito_bundle: false,
            jito_tip_lamports: 1000_000, // 0.01 SOL
            jito_rpc_url: "https://jito-mainnet.rpc.jito.wtf".to_string(),
            jito_block_engine_url: "http://block-engine.jito.wtf:8899".to_string(),
            combine_jito_instructions: false,
            cache_file_path: "instructions_cache.json".to_string(),
            use_manufactured_quotes: true,
            validate_large_quotes: true,
            validation_threshold_sol: 5.0,
            price_impact_model: "sqrt".to_string(),
            price_impact_factor: 1.0,
        }
    }
}

impl Config {
    pub fn update_from_env(&mut self) {
        // Override with environment variables if they exist
        self.private_key = get_env_var("PRIVATE_KEY", &self.private_key);
        
        self.default_rpc = get_env_var("DEFAULT_RPC", &self.default_rpc);
        
        let alt_rpc_list = get_env_var("ALT_RPC_LIST", "");
        if !alt_rpc_list.is_empty() {
            self.alt_rpc_list = alt_rpc_list
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        
        self.wrap_unwrap_sol = get_env_var_bool("WRAP_UNWRAP_SOL", self.wrap_unwrap_sol);
        
        self.trade_size_sol = get_env_var_number("TRADE_SIZE_SOL", self.trade_size_sol);
        
        self.trade_size_strategy = get_env_var("TRADE_SIZE_STRATEGY", &self.trade_size_strategy);
        
        self.min_profit_threshold = get_env_var_number("MIN_PROFIT_THRESHOLD", self.min_profit_threshold);
        
        // Special handling for slippage_bps which is calculated from MAX_SLIPPAGE_PERCENT
        let slippage_percent: f64 = get_env_var_number("MAX_SLIPPAGE_PERCENT", self.slippage_bps as f64 / 100.0);
        self.slippage_bps = (slippage_percent * 100.0) as u64;
        
        self.min_interval_ms = get_env_var_number("MIN_INTERVAL_MS", self.min_interval_ms);
        
        self.token_rotation_interval_minutes = get_env_var_number(
            "TOKEN_ROTATION_INTERVAL_MINUTES", 
            self.token_rotation_interval_minutes
        );
        
        self.jupiter_api_level = get_env_var_number("JUPITER_API_LEVEL", self.jupiter_api_level);
        
        self.machine_amount = get_env_var_number("MACHINE_AMOUNT", self.machine_amount);
        
        self.thread_amount = get_env_var_number("THREAD_AMOUNT", self.thread_amount);
        
        self.current_machine_index = get_env_var_number("CURRENT_MACHINE_INDEX", self.current_machine_index);
        
        self.is_use_token_rotation = get_env_var_bool("IS_USE_TOKEN_ROTATION", self.is_use_token_rotation);
        
        // Calculate appropriate API interval based on service level
        self.calculate_api_interval();
        
        self.adaptive_slippage = get_env_var_bool("ADAPTIVE_SLIPPAGE", self.adaptive_slippage);
        
        self.is_use_swap_cpi = get_env_var_bool("IS_USE_SWAP_CPI", self.is_use_swap_cpi);
        
        // Flash Loan configuration
        self.use_flash_loan = get_env_var_bool("USE_FLASH_LOAN", self.use_flash_loan);
        
        // Jito configuration
        self.use_jito_bundle = get_env_var_bool("USE_JITO_BUNDLE", self.use_jito_bundle);
        
        self.jito_tip_lamports = get_env_var_number("JITO_TIP_LAMPORTS", self.jito_tip_lamports);
        
        self.jito_rpc_url = get_env_var("JITO_RPC_URL", &self.jito_rpc_url);
        
        self.jito_block_engine_url = get_env_var("JITO_BLOCK_ENGINE_URL", &self.jito_block_engine_url);
        
        // New field for combining Jito instructions
        self.combine_jito_instructions = get_env_var_bool("COMBINE_JITO_INSTRUCTIONS", self.combine_jito_instructions);
        
        // Cache configuration
        self.cache_file_path = get_env_var("CACHE_FILE_PATH", &self.cache_file_path);
        
        // Load the new config options for quote manufacturing
        let use_manufactured_quotes = get_env_var_bool("USE_MANUFACTURED_QUOTES", true);
        let validate_large_quotes = get_env_var_bool("VALIDATE_LARGE_QUOTES", true);
        let validation_threshold_sol = get_env_var("VALIDATION_THRESHOLD_SOL", "5.0")
            .parse::<f64>().unwrap_or(5.0);
        let price_impact_model = get_env_var("PRICE_IMPACT_MODEL", "sqrt");
        let price_impact_factor = get_env_var("PRICE_IMPACT_FACTOR", "1.0")
            .parse::<f64>().unwrap_or(1.0);
        
        // Assign these values to our config struct
        self.use_manufactured_quotes = use_manufactured_quotes;
        self.validate_large_quotes = validate_large_quotes;
        self.validation_threshold_sol = validation_threshold_sol;
        self.price_impact_model = price_impact_model;
        self.price_impact_factor = price_impact_factor;
        
        // Log important settings
        log::info!("Config updated from environment variables:");
        log::info!("  RPC URL: {}", self.default_rpc);
        log::info!("  Trade Size: {} SOL", self.trade_size_sol);
        log::info!("  Min Profit Threshold: {}%", self.min_profit_threshold);
        log::info!("  Token Rotation: {}", self.is_use_token_rotation);
        log::info!("  Flash Loan: {}", self.use_flash_loan);
        log::info!("  Jito Bundle: {}", self.use_jito_bundle);
        log::info!("  Combine Jito Instructions: {}", self.combine_jito_instructions);
        log::info!("  Quote Manufacturing: {}", self.use_manufactured_quotes);
        log::info!("  Quote Manufacturing Model: {}", self.price_impact_model);
    }
    
    pub fn calculate_api_interval(&mut self) {
        // Since we're using a paid API with 50 requests/second (3000 RPM) allowance,
        // we'll use minimal intervals for maximum speed
        
        // If API_INTERVAL_MS is explicitly set, respect that value
        let api_interval_ms = get_env_var_number("API_INTERVAL_MS", 0u64);
        if api_interval_ms > 0 {
            self.api_interval_ms = api_interval_ms;
            log::info!("Using API_INTERVAL_MS from environment variable: {}", api_interval_ms);
            return;
        }
        
        // Set minimal interval for maximum speed with paid API
        self.api_interval_ms = 0;
        self.min_interval_ms = 0;
        
        log::info!("Using optimized settings for paid API (50 req/sec): No rate limiting applied");
    }
}

pub fn load_config() -> Config {
    // Create default config
    let mut config = Config::default();
    
    // Update from environment variables
    config.update_from_env();
    
    config
} 
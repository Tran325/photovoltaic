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

pub fn load_config() -> Config {
    // Create default config
    let mut config = Config::default();
    
    // Update from environment variables
    config.update_from_env();
    
    config
} 
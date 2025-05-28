use anyhow::Result;
use std::fs;
use std::path::Path;
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::sync::RwLock;
use std::env;
use log::{info, warn, error};

// Add this for environment variable loading
lazy_static! {
    pub static ref ENV_VARS: RwLock<HashMap<String, String>> = RwLock::new(HashMap::new());
}

pub fn create_temp_dir() -> Result<()> {
    let temp_dir = Path::new("./temp");
    if !temp_dir.exists() {
        info!("Creating temp directory");
        fs::create_dir_all(temp_dir)?;
    }
    Ok(())
}

// Load environment variables from .env file
pub fn load_env_file() -> Result<()> {
    // First load with dotenv
    match dotenv::dotenv() {
        Ok(path) => info!("Loaded .env file from {:?}", path),
        Err(e) => warn!("Could not load .env file: {:?}", e),
    }
    
    // Read the .env file directly as backup
    if let Ok(content) = std::fs::read_to_string(".env") {
        let mut env_vars = ENV_VARS.write().unwrap();
        
        for line in content.lines() {
            // Skip empty lines and comments
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            
            // Parse key-value pairs
            if let Some(idx) = line.find('=') {
                let key = line[..idx].trim().to_string();
                let value = line[idx + 1..].trim().to_string();
                
                // Set environment variable if not already set
                if env::var(&key).is_err() {
                    env::set_var(&key, &value);
                    info!("Set environment variable from .env file: {}", key);
                }
                
                // Store in our HashMap
                env_vars.insert(key, value);
            }
        }
        
        info!("Loaded {} environment variables from .env file", env_vars.len());
    } else {
        error!("Failed to read .env file directly");
    }
    
    Ok(())
}

// Get environment variable with fallback
pub fn get_env_var(key: &str, default: &str) -> String {
    // Try to get from environment first
    if let Ok(value) = env::var(key) {
        return value;
    }
    
    // Then try our cached values
    if let Ok(vars) = ENV_VARS.read() {
        if let Some(value) = vars.get(key) {
            return value.clone();
        }
    }
    
    // Fallback to default
    default.to_string()
}

// Get environment variable as bool
pub fn get_env_var_bool(key: &str, default: bool) -> bool {
    let value = get_env_var(key, &default.to_string());
    value.to_lowercase() == "true"
}

// Get environment variable as number
pub fn get_env_var_number<T>(key: &str, default: T) -> T 
where 
    T: std::str::FromStr + std::fmt::Display,
    <T as std::str::FromStr>::Err: std::fmt::Debug
{
    let value = get_env_var(key, &default.to_string());
    value.parse::<T>().unwrap_or(default)
}



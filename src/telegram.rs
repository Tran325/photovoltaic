use anyhow::Result;
use log::{info, warn, error};
use reqwest::Client;
use serde_json::json;
use std::env;
use std::time::Duration;
use crate::utils::get_env_var;

/// Configuration for Telegram notifications
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
    pub enabled: bool,
}

impl TelegramConfig {
    /// Create a new TelegramConfig from environment variables
    pub fn from_env() -> Self {
        let bot_token = get_env_var("TELEGRAM_BOT_TOKEN", "");
        let chat_id = get_env_var("TELEGRAM_CHAT_ID", "");
        
        // Consider notifications enabled only if both token and chat_id are set
        let enabled = !bot_token.is_empty() && !chat_id.is_empty();
        
        if enabled {
            info!("Telegram notifications are enabled");
        } else if bot_token.is_empty() && !chat_id.is_empty() {
            warn!("Telegram bot token is missing, notifications are disabled");
        } else if !bot_token.is_empty() && chat_id.is_empty() {
            warn!("Telegram chat ID is missing, notifications are disabled");
        } else {
            info!("Telegram notifications are disabled");
        }
        
        Self {
            bot_token,
            chat_id,
            enabled,
        }
    }
}

/// Telegram notification utility
pub struct TelegramNotifier {
    config: TelegramConfig,
    client: Client,
}

impl TelegramNotifier {
    /// Create a new TelegramNotifier with configuration from environment variables
    pub fn new() -> Self {
        let config = TelegramConfig::from_env();
        
        // Create HTTP client with reasonable timeout
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| {
                warn!("Failed to create HTTP client with timeout, using default");
                Client::new()
            });
        
        Self { config, client }
    }
    
    /// Send a message to the configured Telegram chat
    pub async fn send_message(&self, message: &str) -> Result<()> {
        if !self.config.enabled {
            info!("Telegram notifications are disabled, skipping message");
            return Ok(());
        }
        
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.config.bot_token
        );
        
        let response = self.client
            .post(&url)
            .json(&json!({
                "chat_id": self.config.chat_id,
                "text": message,
                "parse_mode": "HTML",
                "disable_web_page_preview": true
            }))
            .send()
            .await?;
        
        if !response.status().is_success() {
            let error_text = response.text().await?;
            error!("Failed to send Telegram message: {}", error_text);
            return Err(anyhow::anyhow!("Telegram API error: {}", error_text));
        }
        
        info!("Telegram message sent successfully");
        Ok(())
    }
    
    /// Send a transaction notification
    pub async fn send_transaction_notification(
        &self,
        signature: &str,
        token_a: &str,
        token_b: &str,
        profit_percentage: f64,
        profit_sol: f64,
    ) -> Result<()> {
        let use_jito = env::var("USE_JITO_BUNDLE").unwrap_or_default() == "true";
        let message;
        if use_jito {
            message = format!(
                "🚀 <b>Jupiter Bot Transaction</b>\n\n\
            🔄 <b>Trade:</b> {} → {} → {}\n\
            💰 <b>Profit:</b> {:.4}% ({:.6} SOL)\n\
            🔗 <b>Explorer:</b> <a href='https://explorer.jito.wtf/bundle/{}'>View bundle</a>",
            token_a, token_b, token_a,
            profit_percentage, profit_sol,
            signature
        );
    }
        else {
            message = format!(
                "🚀 <b>Jupiter Bot Transaction</b>\n\n\
                🔄 <b>Trade:</b> {} → {} → {}\n\
                💰 <b>Profit:</b> {:.4}% ({:.6} SOL)\n\
            🔗 <b>Explorer:</b> <a href='https://solscan.io/tx/{}'>View Transaction</a>",
            token_a, token_b, token_a,
            profit_percentage, profit_sol,
            signature
        );
    }
        
        self.send_message(&message).await
    }
    
    /// Send an error notification
    pub async fn send_error_notification(&self, error_message: &str) -> Result<()> {
        let message = format!(
            "❌ <b>Jupiter Bot Error</b>\n\n\
            <pre>{}</pre>",
            error_message
        );
        
        self.send_message(&message).await
    }
} 
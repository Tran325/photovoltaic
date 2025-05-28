use anyhow::{Result, anyhow};
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_request::TokenAccountsFilter;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::{Transaction, VersionedTransaction},
    account::Account,
};
use solana_sdk::program_pack::Pack;
use bs58;
use crate::jupiter_api::JupiterClient;
use std::sync::Arc;
use std::str::FromStr;
use serde::{Deserialize, Serialize};
use base64;
use log::warn;

// Add a TokenAccount struct for parsing token account data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenAccount {
    pub address: String,
    pub mint: Option<String>,
    pub owner: String,
    pub amount: String,
}

// Define a struct for token balance response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBalance {
    pub ui_amount: f64,
    pub amount: String,
    pub decimals: u8,
}

pub struct SolanaClient {
    rpc_client: Arc<RpcClient>,
    wallet: Keypair,
    pub jupiter_client: Option<Arc<JupiterClient>>,
}

// Implement Clone for SolanaClient
impl Clone for SolanaClient {
    fn clone(&self) -> Self {
        Self {
            rpc_client: self.rpc_client.clone(),
            wallet: Keypair::from_bytes(&self.wallet.to_bytes()).expect("Failed to clone keypair"),
            jupiter_client: self.jupiter_client.clone(),
        }
    }
}

impl SolanaClient {
    // New method to get token account balance
    pub async fn get_token_account_balance(&self, token_account: &Pubkey) -> Result<TokenBalance> {
        // Fetch the token account data
        let account = self.get_account(token_account).await?;
        
        // Unpack the token account data
        let token_account_data = spl_token::state::Account::unpack(&account.data)?;
        
        // Get the mint information to determine decimals
        let mint_pubkey = token_account_data.mint;
        let mint_account = self.get_account(&mint_pubkey).await?;
        let mint_data = spl_token::state::Mint::unpack(&mint_account.data)?;
        
        // Create TokenBalance struct
        let ui_amount = token_account_data.amount as f64 / 10f64.powi(mint_data.decimals as i32);
        let token_balance = TokenBalance {
            ui_amount,
            amount: token_account_data.amount.to_string(),
            decimals: mint_data.decimals,
        };
        
        Ok(token_balance)
    }
} 
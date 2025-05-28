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
    pub fn new(rpc_url: &str, private_key: &str) -> Result<Self> {
        // Create RPC client
        let rpc_client = RpcClient::new_with_commitment(
            rpc_url.to_string(),
            CommitmentConfig::confirmed(),
        );
        
        // Create wallet from private key
        let wallet = match bs58::decode(private_key).into_vec() {
            Ok(bytes) => {
                if bytes.len() != 64 {
                    return Err(anyhow!("Invalid private key length"));
                }
                let secret = bytes.as_slice().try_into()
                    .map_err(|_| anyhow!("Failed to convert private key bytes"))?;
                Keypair::from_bytes(secret)?
            },
            Err(_) => return Err(anyhow!("Invalid private key format")),
        };
        
        Ok(Self {
            rpc_client: Arc::new(rpc_client),
            wallet,
            jupiter_client: None,
        })
    }

    pub async fn get_account(&self, address: &Pubkey) -> Result<Account> {
        Ok(self.rpc_client.get_account(address)?)
    }
    
    pub fn wallet_pubkey(&self) -> Pubkey {
        self.wallet.pubkey()
    }
 
    pub fn get_keypair(&self) -> &Keypair {
        &self.wallet
    }
    
    pub async fn get_latest_blockhash(&self) -> Result<solana_sdk::hash::Hash> {
        let blockhash = self.rpc_client.get_latest_blockhash()?;
        Ok(blockhash)
    }
    
    pub async fn send_transaction(&self, transaction: &Transaction) -> Result<String> {
        let signature = self.rpc_client.send_transaction(transaction)?;
        Ok(signature.to_string())
    }
    
    pub async fn send_versioned_transaction(&self, transaction: &VersionedTransaction) -> Result<String> {
        let signature = self.rpc_client.send_transaction(transaction)?;
        Ok(signature.to_string())
    }

    pub async fn send_and_confirm_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<String> {
        let signature = self.rpc_client.send_and_confirm_transaction_with_spinner(transaction)?;
        Ok(signature.to_string())
    }
    
    pub async fn get_balance(&self, address: &Pubkey) -> Result<u64> {
        let balance = self.rpc_client.get_balance(address)?;
        Ok(balance)
    }
    
    pub async fn get_slot(&self) -> Result<u64> {
        let slot = self.rpc_client.get_slot()?;
        Ok(slot)
    }
    pub async fn get_slot_with_commitment(&self, commitment: CommitmentConfig) -> Result<u64> {
        Ok(self.rpc_client.get_slot_with_commitment(commitment)?)
    }

    pub fn with_jupiter_client(mut self, jupiter_client: Arc<JupiterClient>) -> Self {
        self.jupiter_client = Some(jupiter_client);
        self
    }
    
    // New method to get all token accounts for a wallet
    pub async fn get_token_accounts(&self, owner: &Pubkey) -> Result<Vec<TokenAccount>> {
        // Use the existing RPC client instance to get all token accounts
        let accounts_result = self.rpc_client.get_token_accounts_by_owner(
            owner,
            TokenAccountsFilter::ProgramId(spl_token::id()),
        )?;
        
        let mut token_accounts = Vec::new();
        
        for account_info in accounts_result {
            // Get the real account from the blockchain to parse
            match self.rpc_client.get_account(&Pubkey::from_str(&account_info.pubkey)?) {
                Ok(account) => {
                    // Now we can try to unpack the account data
                    if let Ok(token_account) = spl_token::state::Account::unpack(&account.data) {
                        token_accounts.push(TokenAccount {
                            address: account_info.pubkey.to_string(),
                            mint: Some(token_account.mint.to_string()),
                            owner: token_account.owner.to_string(),
                            amount: token_account.amount.to_string(),
                        });
                    }
                },
                Err(e) => {
                    // Just log and continue if we fail to get a specific account
                    warn!("Failed to get account data for {}: {}", account_info.pubkey, e);
                    continue;
                }
            }
        }
        
        Ok(token_accounts)
    }

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
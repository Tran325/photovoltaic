use crate::solana::SolanaClient;
use anyhow::{Result, anyhow};
use log::{info, warn};
use reqwest;
use serde_json::{json, Value};
use solana_sdk::{
    transaction::{Transaction, VersionedTransaction},
    pubkey::Pubkey,
    hash::Hash,
    system_instruction,
};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

// JITO tip accounts for faster payments - no random selection
pub const JITO_TIP_ACCOUNTS: [&str; 8] = [
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT"
];

// Get Jito URL from environment variables
pub fn get_jito_url() -> String {
    std::env::var("JITO_BLOCK_ENGINE_URL").unwrap_or_else(|_| {
        warn!("JITO_BLOCK_ENGINE_URL not found in environment, using default");
        "https://mainnet.block-engine.jito.wtf/api/v1".to_string()
    })
}

// Query the Jito bundle API
async fn query_jito_api(url: &str, method: &str, params: Value) -> Result<Value> {
    let client = reqwest::Client::new();
    let response = client.post(url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": [params]
        }))
        .send()
        .await?;
    
    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(anyhow!("Failed to query Jito engine: {}", error_text));
    }
    
    let result: Value = response.json().await?;
    Ok(result)
}

// Get tip account for Jito bundles
pub fn get_jito_tip_account() -> Result<Pubkey> {
    // Randomly select a tip account for better distribution
    let tip_account = JITO_TIP_ACCOUNTS[rand::random::<usize>() % JITO_TIP_ACCOUNTS.len()];
    let pubkey = Pubkey::from_str(tip_account)?;
    Ok(pubkey)
}

// Create a tip transaction to Jito validators
pub fn create_jito_tip_transaction(
    solana_client: &SolanaClient,
    recent_blockhash: Hash,
    tip_amount: u64, // Get tip amount from config
) -> Result<Transaction> {
    let tip_account = get_jito_tip_account()?;
    
    // Create tip instruction
    let tip_ix = system_instruction::transfer(
        &solana_client.wallet_pubkey(), 
        &tip_account, 
        tip_amount
    );
    
    // Create and sign the transaction
    let transaction = Transaction::new_signed_with_payer(
        &[tip_ix],
        Some(&solana_client.wallet_pubkey()),
        &[solana_client.get_keypair()],
        recent_blockhash,
    );
    
    Ok(transaction)
}

// Send a bundle of transactions to Jito block engine
pub async fn send_bundle_with_jito_tip(
    solana_client: &Arc<SolanaClient>,
    ata_creator_transaction: Option<VersionedTransaction>,
    main_transaction: &VersionedTransaction,
    recent_blockhash: Hash,
    tip_lamports: Option<u64>,
) -> Result<String> {
    info!("Creating and sending Jito bundle with tip...");
    
    // Get tip amount from config or environment
    let tip_amount = if let Some(tip) = tip_lamports {
        tip
    } else {
        std::env::var("JITO_TIP_LAMPORTS")
            .map(|s| s.parse::<u64>().unwrap_or(1_000_000))
            .unwrap_or(1_000_000) // Default to 0.001 SOL if not set
    };
    
    info!("Using Jito tip amount: {} lamports ({} SOL)", 
        tip_amount, tip_amount as f64 / 1_000_000_000.0);
    
    // Create tip transaction
    let tip_tx = create_jito_tip_transaction(solana_client, recent_blockhash, tip_amount)?;
    
    // Serialize transactions to base58
    let mut bundle_txs = Vec::new();

    // Add ATA creator transaction if it exists
    if let Some(ata_tx) = &ata_creator_transaction {
        let ata_creator_tx_serialized = bs58::encode(bincode::serialize(ata_tx)?).into_string();
        bundle_txs.push(ata_creator_tx_serialized);
    }

    // Add main transaction and tip transaction
    let main_tx_serialized = bs58::encode(bincode::serialize(main_transaction)?).into_string();
    let tip_tx_serialized = bs58::encode(bincode::serialize(&tip_tx)?).into_string();
    bundle_txs.push(main_tx_serialized);
    bundle_txs.push(tip_tx_serialized);

    // Send bundle to Jito block engine
    let jito_url = format!("{}/bundles", get_jito_url());
    info!("Sending bundle to Jito at URL: {}", jito_url);
    let bundle_txs_json = json!(bundle_txs);
    info!("Bundle txs: {:?}", bundle_txs_json);

    let response = query_jito_api(
        &jito_url,
        "sendBundle",
        bundle_txs_json
    ).await?;
    
    // Parse bundle UUID
    let bundle_uuid = response["result"].as_str()
        .ok_or_else(|| anyhow!("Failed to get bundle UUID from response"))?;
    
    info!("✅ Bundle sent with UUID: {}", bundle_uuid);
    
    // Wait for bundle confirmation
    // let bundle_result = wait_for_bundle_confirmation(solana_client, bundle_uuid).await?;
    // FOR bot performance, we don't wait for bundle confirmation
    let bundle_result = bundle_uuid.to_string();
    
    Ok(bundle_result)
}

// Wait for bundle confirmation
async fn wait_for_bundle_confirmation(
    solana_client: &Arc<SolanaClient>,
    bundle_uuid: &str,
) -> Result<String> {
    let max_retries = 50;
    let time_between_retries = Duration::from_millis(5000);
    
    let inflight_url = format!("{}/getInflightBundleStatuses", get_jito_url());
    let bundle_url = format!("{}/getBundleStatuses", get_jito_url());
    
    for retry in 0..max_retries {
        // Check inflight bundle status
        let inflight_response = query_jito_api(
            &inflight_url,
            "getInflightBundleStatuses",
            json!([bundle_uuid])
        ).await?;
        
        // Parse the response to check status
        let inflight_status = inflight_response["result"]["value"].as_array()
            .and_then(|arr| arr.get(0))
            .and_then(|obj| obj["status"].as_str())
            .unwrap_or("Unknown");
        
        info!("🔄 JITO bundle status (retry {}/{}): {}", retry + 1, max_retries, inflight_status);
        
        if inflight_status == "Failed" {
            return Err(anyhow!("❌ JITO bundle failed"));
        }
        
        if inflight_status == "Landed" {
            // Get transaction signatures from bundle
            let bundle_response = query_jito_api(
                &bundle_url,
                "getBundleStatuses",
                json!([bundle_uuid])
            ).await?;
            
            let transactions = bundle_response["result"]["value"].as_array()
                .and_then(|arr| arr.get(0))
                .and_then(|obj| obj["transactions"].as_array())
                .ok_or_else(|| anyhow!("Failed to get transactions from bundle"))?;
            
            // Extract the first transaction signature (the main arbitrage transaction)
            if let Some(main_tx_sig) = transactions.get(0).and_then(|v| v.as_str()) {
                info!("✅ JITO bundle landed with signature: {}", main_tx_sig);
                return Ok(main_tx_sig.to_string());
            }
            
            return Err(anyhow!("Bundle landed but no transactions found"));
        }
        
        // Wait before checking again
        tokio::time::sleep(time_between_retries).await;
    }
    
    Err(anyhow!("Timed out waiting for bundle confirmation"))
}


use crate::solana::SolanaClient;
use anyhow::Result;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    transaction::Transaction,
    commitment_config::CommitmentConfig,
    sysvar,
};
use std::str::FromStr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use log::info;
use std::env;
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::sync::Mutex;

// Constants for Kamino Lending
const KLEND_PROGRAM_ID: &str = "KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD";
const FLASH_BORROW_DISCRIMINATOR: [u8; 8] = [135, 231, 52, 167, 7, 52, 212, 193];
const FLASH_REPAY_DISCRIMINATOR: [u8; 8] = [185, 117, 0, 203, 96, 245, 180, 186];

// Global FlashLoanContext cache to avoid recreating for each transaction
lazy_static! {
    static ref FLASH_LOAN_CONTEXT_CACHE: Mutex<HashMap<String, Arc<FlashLoanContext>>> = Mutex::new(HashMap::new());
}

/// FlashLoanContext holds all the necessary account information needed for flash loans
/// 
/// A flash loan is a type of uncollateralized loan where a user can borrow assets
/// and must repay them in the same transaction. This enables arbitrage and other
/// complex operations without requiring upfront capital.
///
/// This implementation uses Kamino Lending's flash loan functionality.
/// 
/// To use a flash loan:
/// 1. Create a flash borrow instruction to borrow funds
/// 2. Perform operations with the borrowed funds (e.g., arbitrage swaps)
/// 3. Create a flash repay instruction to return the borrowed funds (plus fees)
/// 4. Execute all instructions in a single transaction
pub struct FlashLoanContext {
    pub solana_client: Arc<SolanaClient>,
    pub klend_program_id: Pubkey,
    pub lending_market_authority: Pubkey,
    pub lending_market: Pubkey,
    pub reserve: Pubkey,
    pub reserve_liquidity_mint: Pubkey,
    pub reserve_source_liquidity: Pubkey,
    pub user_token_account: Pubkey,
    pub reserve_liquidity_fee_receiver: Pubkey,
    pub referrer_token_state: Pubkey,
    pub referrer_account: Pubkey,
}

/// Arguments for the flash borrow instruction
pub struct FlashBorrowReserveLiquidityArgs {
    /// Amount to borrow in lamports
    pub amount: u64,
}

/// Arguments for the flash repay instruction
pub struct FlashRepayReserveLiquidityArgs {
    /// Amount to repay in lamports (should match the borrowed amount)
    pub amount: u64,
    /// Index of the borrow instruction in the transaction
    pub borrow_instruction_index: u8,
}

impl FlashLoanContext {
    /// Creates a new FlashLoanContext with the necessary account information
    ///
    /// This function initializes a FlashLoanContext with the provided parameters
    /// and ensures that the user token account exists (creating it if necessary).
    ///
    /// # Arguments
    /// * `solana_client` - A reference to the SolanaClient
    /// * `lending_market_authority` - The pubkey of the lending market authority
    /// * `lending_market` - The pubkey of the lending market
    /// * `reserve` - The pubkey of the reserve account
    /// * `reserve_liquidity_mint` - The pubkey of the reserve liquidity mint (e.g., SOL's mint)
    /// * `reserve_source_liquidity` - The pubkey of the reserve source liquidity
    /// * `reserve_liquidity_fee_receiver` - The pubkey of the fee receiver
    /// * `referrer_token_state` - The pubkey of the referrer token state
    /// * `referrer_account` - The pubkey of the referrer account
    ///
    /// # Returns
    /// A Result containing the FlashLoanContext or an error
    pub async fn new(
        solana_client: Arc<SolanaClient>,
        lending_market_authority: &str,
        lending_market: &str,
        reserve: &str,
        reserve_liquidity_mint: &str,
        reserve_source_liquidity: &str,
        reserve_liquidity_fee_receiver: &str,
        referrer_token_state: &str,
        referrer_account: &str,
    ) -> Result<Self> {
        info!("Initializing flash loan context with parameters:");
        info!("  Lending Market Authority: {}", lending_market_authority);
        info!("  Lending Market: {}", lending_market);
        info!("  Reserve: {}", reserve);
        info!("  Reserve Liquidity Mint: {}", reserve_liquidity_mint);
        
        // Validate environment variables
        if lending_market_authority.is_empty() || lending_market.is_empty() || reserve.is_empty() 
            || reserve_liquidity_mint.is_empty() || reserve_source_liquidity.is_empty() {
            return Err(anyhow::anyhow!("Missing required flash loan parameters. Please check your .env file for LENDING_MARKET_AUTHORITY, LENDING_MARKET_ADDRESS, RESERVE_ADDRESS, RESERVE_LIQUIDITY_MINT, and RESERVE_SOURCE_LIQUIDITY"));
        }
        
        let wallet_pubkey = solana_client.wallet_pubkey();
        info!("Using wallet pubkey: {}", wallet_pubkey);
        
        let reserve_liquidity_mint_pubkey = match Pubkey::from_str(reserve_liquidity_mint) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("Invalid reserve liquidity mint pubkey: {}", e)),
        };

        // Ensure user token account exists
        let user_token_account = spl_associated_token_account::get_associated_token_address(
            &wallet_pubkey,
            &reserve_liquidity_mint_pubkey,
        );
        info!("User token account: {}", user_token_account);

        // Check if account needs to be created
        if solana_client.get_account(&user_token_account).await.is_err() {
            info!("Creating user token account for flash loan...");
            let create_ata_ix = spl_associated_token_account::instruction::create_associated_token_account(
                &wallet_pubkey,
                &wallet_pubkey,
                &reserve_liquidity_mint_pubkey,
                &spl_token::id(),
            );

            let recent_blockhash = match solana_client.get_latest_blockhash().await {
                Ok(blockhash) => blockhash,
                Err(e) => return Err(anyhow::anyhow!("Failed to get recent blockhash: {}", e)),
            };
            
            let transaction = solana_sdk::transaction::Transaction::new_signed_with_payer(
                &[create_ata_ix],
                Some(&wallet_pubkey),
                &[solana_client.get_keypair()],
                recent_blockhash,
            );

            match solana_client.send_transaction(&transaction).await {
                Ok(signature) => info!("Created user token account with signature: {}", signature),
                Err(e) => return Err(anyhow::anyhow!("Failed to create token account: {}", e)),
            }
        } else {
            info!("User token account already exists");
        }

        let klend_program_id = match Pubkey::from_str(KLEND_PROGRAM_ID) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("Invalid KLend program ID: {}", e)),
        };
        
        let lending_market_authority_pubkey = match Pubkey::from_str(lending_market_authority) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("Invalid lending market authority: {}", e)),
        };
        
        let lending_market_pubkey = match Pubkey::from_str(lending_market) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("Invalid lending market address: {}", e)),
        };
        
        let reserve_pubkey = match Pubkey::from_str(reserve) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("Invalid reserve address: {}", e)),
        };
        
        let reserve_source_liquidity_pubkey = match Pubkey::from_str(reserve_source_liquidity) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("Invalid reserve source liquidity: {}", e)),
        };
        
        let reserve_liquidity_fee_receiver_pubkey = match Pubkey::from_str(reserve_liquidity_fee_receiver) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("Invalid reserve liquidity fee receiver: {}", e)),
        };
        
        let referrer_token_state_pubkey = match Pubkey::from_str(referrer_token_state) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("Invalid referrer token state: {}", e)),
        };
        
        let referrer_account_pubkey = match Pubkey::from_str(referrer_account) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("Invalid referrer account: {}", e)),
        };

        info!("Flash loan context initialized successfully");
        
        Ok(Self {
            solana_client,
            klend_program_id,
            lending_market_authority: lending_market_authority_pubkey,
            lending_market: lending_market_pubkey,
            reserve: reserve_pubkey,
            reserve_liquidity_mint: reserve_liquidity_mint_pubkey,
            reserve_source_liquidity: reserve_source_liquidity_pubkey,
            user_token_account,
            reserve_liquidity_fee_receiver: reserve_liquidity_fee_receiver_pubkey,
            referrer_token_state: referrer_token_state_pubkey,
            referrer_account: referrer_account_pubkey,
        })
    }

    /// Gets an existing or creates a new FlashLoanContext and caches it
    ///
    /// # Arguments
    /// * `solana_client` - A reference to the SolanaClient
    /// * `lending_market_authority` - The pubkey of the lending market authority
    /// * `lending_market` - The pubkey of the lending market
    /// * `reserve` - The pubkey of the reserve account
    /// * `reserve_liquidity_mint` - The pubkey of the reserve liquidity mint
    /// * `reserve_source_liquidity` - The pubkey of the reserve source liquidity
    /// * `reserve_liquidity_fee_receiver` - The pubkey of the fee receiver
    /// * `referrer_token_state` - The pubkey of the referrer token state
    /// * `referrer_account` - The pubkey of the referrer account
    ///
    /// # Returns
    /// A Result containing the FlashLoanContext or an error
    pub async fn get_or_create(
        solana_client: Arc<SolanaClient>,
        lending_market_authority: &str,
        lending_market: &str,
        reserve: &str,
        reserve_liquidity_mint: &str,
        reserve_source_liquidity: &str,
        reserve_liquidity_fee_receiver: &str,
        referrer_token_state: &str,
        referrer_account: &str,
    ) -> Result<Arc<FlashLoanContext>> {
        // Create a cache key from the reserve (primary identifier)
        let cache_key = reserve.to_string();
        
        // Try to get from cache first
        {
            let cache = FLASH_LOAN_CONTEXT_CACHE.lock().unwrap();
            if let Some(context) = cache.get(&cache_key) {
                info!("Using cached FlashLoanContext for reserve: {}", reserve);
                return Ok(context.clone());
            }
        }
        
        // Not in cache, create a new context
        info!("Creating new FlashLoanContext for reserve: {}", reserve);
        let context = Arc::new(Self::new(
            solana_client,
            lending_market_authority,
            lending_market,
            reserve,
            reserve_liquidity_mint,
            reserve_source_liquidity,
            reserve_liquidity_fee_receiver,
            referrer_token_state,
            referrer_account,
        ).await?);
        
        // Store in cache
        {
            let mut cache = FLASH_LOAN_CONTEXT_CACHE.lock().unwrap();
            cache.insert(cache_key, context.clone());
        }
        
        Ok(context)
    }
    
    /// Invalidates the cache entry for a specific reserve or the entire cache
    ///
    /// Call this method when there are issues with a cached FlashLoanContext
    /// or when the underlying accounts might have changed.
    ///
    /// # Arguments
    /// * `reserve` - Optional reserve address. If provided, only that entry is removed.
    ///               If None, the entire cache is cleared.
    pub fn invalidate_cache(reserve: Option<&str>) {
        let mut cache = FLASH_LOAN_CONTEXT_CACHE.lock().unwrap();
        match reserve {
            Some(reserve_addr) => {
                if cache.remove(reserve_addr).is_some() {
                    info!("Invalidated FlashLoanContext cache for reserve: {}", reserve_addr);
                }
            },
            None => {
                let count = cache.len();
                cache.clear();
                info!("Cleared entire FlashLoanContext cache ({} entries)", count);
            }
        }
    }
}

/// Creates a flash borrow instruction for borrowing tokens
///
/// This function creates an instruction to borrow tokens from the Kamino Lending
/// protocol as part of a flash loan. The borrowed amount must be repaid (plus fees)
/// in the same transaction.
///
/// # Arguments
/// * `ctx` - The FlashLoanContext containing account information
/// * `args` - The FlashBorrowReserveLiquidityArgs containing the amount to borrow
///
/// # Returns
/// A Result containing the Instruction or an error
pub fn create_flash_borrow_instruction(
    ctx: &FlashLoanContext,
    args: &FlashBorrowReserveLiquidityArgs,
) -> Result<Instruction> {
    info!("Creating flash borrow instruction");
    info!("Wallet pubkey that will be signer: {}", ctx.solana_client.wallet_pubkey());
    
    // Create instruction data
    let identifier = FLASH_BORROW_DISCRIMINATOR;
    
    // Create a buffer for the arguments
    let mut buffer = vec![0; 8]; // 8 bytes for u64
    
    // Encode the liquidityAmount (u64) in little-endian format
    buffer[0..8].copy_from_slice(&args.amount.to_le_bytes());
    
    // Create the final data buffer
    let mut data = Vec::with_capacity(16); // 8 bytes identifier + 8 bytes amount
    data.extend_from_slice(&identifier);
    data.extend_from_slice(&buffer);
    
    // Create account metas
    let accounts = vec![
        AccountMeta::new(ctx.solana_client.wallet_pubkey(), true), // User Wallet
        AccountMeta::new_readonly(ctx.lending_market_authority, false), // Lending Market Authority
        AccountMeta::new_readonly(ctx.lending_market, false), // Lending Market
        AccountMeta::new(ctx.reserve, false), // Reserve
        AccountMeta::new_readonly(ctx.reserve_liquidity_mint, false), // Reserve Liquidity Mint
        AccountMeta::new(ctx.reserve_source_liquidity, false), // Reserve Source Liquidity
        AccountMeta::new(ctx.user_token_account, false), // User Token Account
        AccountMeta::new(ctx.reserve_liquidity_fee_receiver, false), // Reserve Liquidity Fee Receiver
        AccountMeta::new(ctx.referrer_token_state, false), // Referrer Token State
        AccountMeta::new(ctx.referrer_account, false), // Referrer Account
        AccountMeta::new_readonly(solana_sdk::sysvar::instructions::id(), false), // Sysvar Info
        AccountMeta::new_readonly(spl_token::id(), false), // Token Program
    ];
    
    info!("Flash borrow instruction accounts that require signing:");
    for account in accounts.iter().filter(|a| a.is_signer) {
        info!("  - {} (writable: {})", account.pubkey, account.is_writable);
    }

    Ok(Instruction {
        program_id: ctx.klend_program_id,
        accounts,
        data,
    })
}

/// Creates a flash repay instruction for repaying borrowed tokens
///
/// This function creates an instruction to repay tokens previously borrowed
/// from the Kamino Lending protocol as part of a flash loan. This instruction
/// must be executed in the same transaction as the borrow instruction.
///
/// # Arguments
/// * `ctx` - The FlashLoanContext containing account information
/// * `args` - The FlashRepayReserveLiquidityArgs containing the amount to repay
///            and the index of the borrow instruction
///
/// # Returns
/// A Result containing the Instruction or an error
pub fn create_flash_repay_instruction(
    ctx: &FlashLoanContext,
    args: &FlashRepayReserveLiquidityArgs,
) -> Result<Instruction> {
    info!("Creating flash repay instruction");
    info!("Wallet pubkey that will be signer: {}", ctx.solana_client.wallet_pubkey());
    
    // Create instruction data
    let identifier = FLASH_REPAY_DISCRIMINATOR;
    
    // Create a buffer for the arguments
    let mut buffer = vec![0; 9]; // 8 bytes for u64 + 1 byte for u8
    
    // Encode the liquidityAmount (u64) in little-endian format
    buffer[0..8].copy_from_slice(&args.amount.to_le_bytes());
    
    // Encode the borrowInstructionIndex (u8)
    buffer[8] = args.borrow_instruction_index;
    
    // Create the final data buffer
    let mut data = Vec::with_capacity(17); // 8 bytes identifier + 9 bytes args
    data.extend_from_slice(&identifier);
    data.extend_from_slice(&buffer);
    
    // Create account metas
    let accounts = vec![
        AccountMeta::new(ctx.solana_client.wallet_pubkey(), true), // User Wallet
        AccountMeta::new_readonly(ctx.lending_market_authority, false), // Lending Market Authority
        AccountMeta::new_readonly(ctx.lending_market, false), // Lending Market
        AccountMeta::new(ctx.reserve, false), // Reserve
        AccountMeta::new_readonly(ctx.reserve_liquidity_mint, false), // Reserve Liquidity Mint
        AccountMeta::new(ctx.reserve_source_liquidity, false), // Reserve Source Liquidity
        AccountMeta::new(ctx.user_token_account, false), // User Token Account
        AccountMeta::new(ctx.reserve_liquidity_fee_receiver, false), // Reserve Liquidity Fee Receiver
        AccountMeta::new(ctx.referrer_token_state, false), // Referrer Token State
        AccountMeta::new(ctx.referrer_account, false), // Referrer Account
        AccountMeta::new_readonly(solana_sdk::sysvar::instructions::id(), false), // Sysvar Info
        AccountMeta::new_readonly(spl_token::id(), false), // Token Program
    ];
    
    info!("Flash repay instruction accounts that require signing:");
    for account in accounts.iter().filter(|a| a.is_signer) {
        info!("  - {} (writable: {})", account.pubkey, account.is_writable);
    }

    Ok(Instruction {
        program_id: ctx.klend_program_id,
        accounts,
        data,
    })
}

/// Creates or retrieves an Address Lookup Table for flash loan accounts
///
/// If FLASH_LOAN_ADDRESS_LOOKUP_TABLE is not defined in the .env file,
/// this function will create a new Address Lookup Table containing all
/// the required accounts for flash loan operations.
///
/// # Arguments
/// * `ctx` - The FlashLoanContext containing account information
///
/// # Returns
/// A Result containing the address of the lookup table as a string
pub async fn create_or_get_flash_loan_lookup_table(solana_client: &SolanaClient) -> Result<String> {
    // Check if we have an existing lookup table address in environment
    let lookup_table_addr_str = std::env::var("FLASH_LOAN_ADDRESS_LOOKUP_TABLE").unwrap_or_default();
    
    if !lookup_table_addr_str.is_empty() {
        info!("Using existing flash loan address lookup table: {}", lookup_table_addr_str);
        return Ok(lookup_table_addr_str);
    }
    
    // Clone the reference to create an owned SolanaClient for the Arc
    let flash_loan_ctx = FlashLoanContext::new(
        Arc::new(solana_client.clone()),
        &env::var("LENDING_MARKET_AUTHORITY").unwrap_or_default(),
        &env::var("LENDING_MARKET_ADDRESS").unwrap_or_default(),
        &env::var("RESERVE_ADDRESS").unwrap_or_default(),
        &env::var("RESERVE_LIQUIDITY_MINT").unwrap_or_default(),
        &env::var("RESERVE_SOURCE_LIQUIDITY").unwrap_or_default(),
        &env::var("RESERVE_LIQUIDITY_FEE_RECEIVER").unwrap_or_default(),
        &env::var("REFERER_TOKEN_STATE").unwrap_or_default(),
        &env::var("REFERER_ACCOUNT").unwrap_or_default(),
    ).await?;
        

    let wallet_pubkey = solana_client.wallet_pubkey();
    let recent_slot = solana_client.get_slot_with_commitment(
        CommitmentConfig::finalized()
    ).await?;

    // Create the lookup table
    let (create_ix, lookup_table_address) = 
        solana_address_lookup_table_program::instruction::create_lookup_table(
            wallet_pubkey,
            wallet_pubkey,
            recent_slot,
        );

    // Create transaction to create the lookup table
    let recent_blockhash = solana_client.get_latest_blockhash().await?;
    let create_table_tx = Transaction::new_signed_with_payer(
        &[create_ix],
        Some(&wallet_pubkey),
        &[solana_client.get_keypair()],
        recent_blockhash,
    );

    // Send and confirm the transaction
    info!("Creating address lookup table...");
    let signature = solana_client.send_and_confirm_transaction(&create_table_tx).await?;
    info!("Created address lookup table: {} with signature: {}", lookup_table_address, signature);

    // Wait a moment for the table to be confirmed
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

     // Extend the lookup table with flash loan accounts
      let addresses_to_add = vec![
        flash_loan_ctx.klend_program_id,
        flash_loan_ctx.lending_market_authority,
        flash_loan_ctx.lending_market,
        flash_loan_ctx.reserve,
        flash_loan_ctx.reserve_liquidity_mint,
        flash_loan_ctx.reserve_source_liquidity,
        flash_loan_ctx.user_token_account,
        flash_loan_ctx.reserve_liquidity_fee_receiver,
        flash_loan_ctx.referrer_token_state,
        flash_loan_ctx.referrer_account,
        solana_sdk::sysvar::instructions::id(),
        spl_token::id(),
    ];
    
  // Create extend instruction
  let extend_ix = solana_address_lookup_table_program::instruction::extend_lookup_table(
    lookup_table_address,
    wallet_pubkey,
    Some(wallet_pubkey),
    addresses_to_add,
);

// Create transaction to extend the lookup table
let recent_blockhash = solana_client.get_latest_blockhash().await?;
let extend_table_tx = Transaction::new_signed_with_payer(
    &[extend_ix],
    Some(&wallet_pubkey),
    &[solana_client.get_keypair()],
    recent_blockhash,
);

    // Send and confirm the transaction
    info!("Extending address lookup table...");
    let signature = solana_client.send_and_confirm_transaction(&extend_table_tx).await?;
    info!("Extended address lookup table with signature: {}", signature);

    // Wait a moment for the extension to be confirmed
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    Ok(lookup_table_address.to_string())
}
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


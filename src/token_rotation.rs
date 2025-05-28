use crate::jupiter_api::{JupiterClient, Token};
use anyhow::{Result, anyhow};
use log::{info, error};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct TokenRotator {
    pub jupiter_client: Arc<JupiterClient>,
    pub token_list: Arc<Mutex<Vec<Token>>>,
    pub last_rotation: Arc<Mutex<Instant>>,
    pub rotation_interval: Duration,
    pub thread_token_indices: Arc<Mutex<Vec<usize>>>,
    pub machine_index: u64,
    pub machine_count: u64,
    // Add tracking for which tokens have been used by each thread
    pub thread_token_history: Arc<Mutex<Vec<Vec<usize>>>>,
}

impl TokenRotator {
    pub fn new(jupiter_client: Arc<JupiterClient>, rotation_interval_minutes: u64, machine_index: u64, machine_count: u64) -> Self {
        info!("Initializing token rotator with {} minute interval (Machine {}/{})", 
            rotation_interval_minutes, machine_index, machine_count);
        
        Self {
            jupiter_client,
            token_list: Arc::new(Mutex::new(Vec::new())),
            last_rotation: Arc::new(Mutex::new(Instant::now())),
            rotation_interval: Duration::from_secs(rotation_interval_minutes * 60),
            thread_token_indices: Arc::new(Mutex::new(Vec::new())),
            machine_index,
            machine_count,
            thread_token_history: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    
    pub fn rotate_token_for_thread(&self, thread_id: usize) -> Result<Token> {
        let token_list = self.token_list.lock().unwrap();
        
        if token_list.is_empty() {
            return Err(anyhow!("Token list is empty"));
        }
        
        let mut thread_indices = self.thread_token_indices.lock().unwrap();
        let mut thread_history = self.thread_token_history.lock().unwrap();
        
        if thread_id >= thread_indices.len() {
            return Err(anyhow!("Invalid thread ID: {}", thread_id));
        }
        
        // Get current token index for this thread
        let current_index = thread_indices[thread_id];
        let old_token = &token_list[current_index];
        
        // Find the next token that hasn't been used by this thread recently
        // We'll try to go through all tokens before repeating
        let token_count = token_list.len();
        let mut new_index = (current_index + 1) % token_count;
        
        // If we've used all tokens once, start over
        if thread_history[thread_id].len() >= token_count {
            // Reset history but keep the last used token to avoid immediate repetition
            let last_used = thread_history[thread_id].last().cloned();
            thread_history[thread_id].clear();
            if let Some(last) = last_used {
                thread_history[thread_id].push(last);
            }
        }
        
        // Find a token that hasn't been used recently by this thread
        let mut attempts = 0;
        while thread_history[thread_id].contains(&new_index) && attempts < token_count {
            new_index = (new_index + 1) % token_count;
            attempts += 1;
        }
        
        // Update the index for this thread
        thread_indices[thread_id] = new_index;
        
        // Add to history
        thread_history[thread_id].push(new_index);
        
        // Update the last rotation time
        let mut last_rotation = self.last_rotation.lock().unwrap();
        *last_rotation = Instant::now();
        
        let new_token = &token_list[new_index];
        
        info!("Thread {} rotated from {} to {} (token #{} of {})", 
            thread_id,
            old_token.symbol, 
            new_token.symbol, 
            new_index + 1, 
            token_list.len());
        
        Ok(new_token.clone())
    }
}
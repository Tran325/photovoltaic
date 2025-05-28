use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use log::{info, warn, debug};
use std::collections::{HashMap, HashSet};

pub struct RateLimiter {
    pub request_count: Arc<Mutex<u32>>,
    pub last_reset_time: Arc<Mutex<Instant>>,
    pub is_rate_limited: Arc<Mutex<bool>>,
    pub current_delay: Arc<Mutex<u64>>,
    pub max_requests_per_minute: u32,
    pub min_interval_ms: u64,
    pub thread_call_counts: Arc<Mutex<HashMap<usize, u32>>>,
    pub last_log_time: Arc<Mutex<Instant>>,
    pub endpoint_counts: Arc<Mutex<HashMap<String, u32>>>,
    pub thread_rate_limited: Arc<Mutex<HashSet<usize>>>,
    pub backoff_threads: Arc<Mutex<HashMap<usize, Instant>>>,
    pub backoff_duration: Arc<Mutex<u64>>,
    pub api_level: Arc<Mutex<u64>>,
}

impl Clone for RateLimiter {
    fn clone(&self) -> Self {
        Self {
            request_count: self.request_count.clone(),
            last_reset_time: self.last_reset_time.clone(),
            is_rate_limited: self.is_rate_limited.clone(),
            current_delay: self.current_delay.clone(),
            max_requests_per_minute: self.max_requests_per_minute,
            min_interval_ms: self.min_interval_ms,
            thread_call_counts: self.thread_call_counts.clone(),
            last_log_time: self.last_log_time.clone(),
            endpoint_counts: self.endpoint_counts.clone(),
            thread_rate_limited: self.thread_rate_limited.clone(),
            backoff_threads: self.backoff_threads.clone(),
            backoff_duration: self.backoff_duration.clone(),
            api_level: self.api_level.clone(),
        }
    }
} 
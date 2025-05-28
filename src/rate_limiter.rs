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

impl RateLimiter {
    pub fn new(min_interval_ms: u64, max_requests_per_minute: u32) -> Self {
        info!("Initializing optimized rate limiter for paid API (50 req/sec) without throttling");
            
        Self {
            request_count: Arc::new(Mutex::new(0)),
            last_reset_time: Arc::new(Mutex::new(Instant::now())),
            is_rate_limited: Arc::new(Mutex::new(false)),
            current_delay: Arc::new(Mutex::new(0)), // Set default delay to 0
            max_requests_per_minute: 3000, // 50 req/sec = 3000 req/min
            min_interval_ms: 0, // No minimum interval
            thread_call_counts: Arc::new(Mutex::new(HashMap::new())),
            last_log_time: Arc::new(Mutex::new(Instant::now())),
            endpoint_counts: Arc::new(Mutex::new(HashMap::new())),
            thread_rate_limited: Arc::new(Mutex::new(HashSet::new())),
            backoff_threads: Arc::new(Mutex::new(HashMap::new())),
            backoff_duration: Arc::new(Mutex::new(100)), // Minimal backoff for actual API errors
            api_level: Arc::new(Mutex::new(4)), // Set to highest level
        }
    }
    
    pub fn is_in_backoff(&self, thread_id: usize) -> bool {
        // Always return false - no backoff needed for paid API
        false
    }
    
    pub fn get_backoff_delay(&self) -> u64 {
        // Return minimal delay - no need to back off for paid API
        0
    }
    
    pub fn handle_rate_limit_error(&self, thread_id: usize) {
        // Only log the error but don't do actual rate limiting
        warn!("Thread {} received rate limit error - this is unexpected with paid API", thread_id);
        
        // Apply minimal backoff only for actual API errors (not our own rate limiting)
        let mut backoff_threads = self.backoff_threads.lock().unwrap();
        backoff_threads.insert(thread_id, Instant::now());
    }
    
    pub fn set_api_level(&self, level: u64) {
        let mut api_level = self.api_level.lock().unwrap();
        *api_level = level;
        info!("Rate limiter API level set to {} (no throttling enabled)", level);
    }
    
    pub fn increment_request_count_for_thread(&self, thread_id: usize, endpoint: Option<&str>) -> bool {
        let mut count = self.request_count.lock().unwrap();
        let mut last_reset = self.last_reset_time.lock().unwrap();
        let mut thread_counts = self.thread_call_counts.lock().unwrap();
        let mut last_log = self.last_log_time.lock().unwrap();
        let mut endpoint_counts = self.endpoint_counts.lock().unwrap();
        
        // Still track counters for monitoring
        if let Some(ep) = endpoint {
            *endpoint_counts.entry(ep.to_string()).or_insert(0) += 1;
        }
        
        *count += 1;
        *thread_counts.entry(thread_id).or_insert(0) += 1;
        
        // Reset counters and log stats every minute
        if last_reset.elapsed() >= Duration::from_secs(60) {
            info!("=== API Call Statistics for the Last Minute ===");
            info!("Total API calls: {} (no throttling applied, paid API)", *count);
            
            info!("Thread distribution:");
            for (tid, calls) in thread_counts.iter() {
                let percentage = (*calls as f64 / *count as f64) * 100.0;
                info!("  Thread {}: {} calls ({:.1}%)", tid, calls, percentage);
            }
            
            if !endpoint_counts.is_empty() {
                info!("Endpoint distribution:");
                for (ep, ep_count) in endpoint_counts.iter() {
                    let percentage = (*ep_count as f64 / *count as f64) * 100.0;
                    info!("  {}: {} calls ({:.1}%)", ep, ep_count, percentage);
                }
            }
            
            *count = 1;
            *last_reset = Instant::now();
            thread_counts.clear();
            endpoint_counts.clear();
        }
        
        // Log stats every 10 seconds without resetting
        if last_log.elapsed() >= Duration::from_secs(10) {
            debug!("Current API usage: {} calls (no throttling, paid API)", *count);
            
            if thread_counts.len() > 1 {
                debug!("Thread distribution:");
                for (tid, calls) in thread_counts.iter() {
                    let percentage = (*calls as f64 / *count as f64) * 100.0;
                    debug!("  Thread {}: {} calls ({:.1}%)", tid, calls, percentage);
                }
            }
            
            *last_log = Instant::now();
        }
        
        // Always return true - never limit requests
        true
    }
    
    pub fn get_current_delay_for_main_loop(&self) -> u64 {
        // Return 0 for immediate execution
        0
    }
    
    pub fn get_current_delay(&self) -> u64 {
        // Return 0 for immediate execution
        0
    }
    
    pub fn is_limited(&self) -> bool {
        // Never limited for paid API
        false
    }
    
    pub fn reset(&self) {
        let mut count = self.request_count.lock().unwrap();
        let mut last_reset = self.last_reset_time.lock().unwrap();
        let mut is_limited = self.is_rate_limited.lock().unwrap();
        let mut current_delay = self.current_delay.lock().unwrap();
        let mut thread_counts = self.thread_call_counts.lock().unwrap();
        let mut endpoint_counts = self.endpoint_counts.lock().unwrap();
        let mut thread_limited = self.thread_rate_limited.lock().unwrap();
        
        // Reset all counters but don't apply any limits
        *count = 0;
        *last_reset = Instant::now();
        *is_limited = false;
        *current_delay = 0;
        thread_counts.clear();
        endpoint_counts.clear();
        thread_limited.clear();
    }
    
    pub fn get_thread_stats(&self) -> Vec<(usize, u32)> {
        let thread_counts = self.thread_call_counts.lock().unwrap();
        thread_counts.iter()
            .map(|(&tid, &count)| (tid, count))
            .collect()
    }
    
    pub fn set_thread_id(&self, thread_id: usize) {
        debug!("Setting thread ID {} for rate limiter (API compatibility only)", thread_id);
    }
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
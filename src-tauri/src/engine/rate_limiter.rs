use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug)]
struct TokenBucketState {
    tokens: f64,
    last_refill: Instant,
}

#[derive(Clone)]
pub struct TokenBucketLimiter {
    rate_bytes_per_sec: Arc<AtomicU64>,
    state: Arc<Mutex<TokenBucketState>>,
}

impl TokenBucketLimiter {
    pub fn new(rate_bytes_per_sec: u64) -> Self {
        Self {
            rate_bytes_per_sec: Arc::new(AtomicU64::new(rate_bytes_per_sec)),
            state: Arc::new(Mutex::new(TokenBucketState {
                tokens: rate_bytes_per_sec as f64,
                last_refill: Instant::now(),
            })),
        }
    }

    pub fn set_rate(&self, new_rate: u64) {
        self.rate_bytes_per_sec.store(new_rate, Ordering::Relaxed);
    }

    pub fn get_rate(&self) -> u64 {
        self.rate_bytes_per_sec.load(Ordering::Relaxed)
    }

    /// Acquires tokens for `bytes` amount of data.
    /// If rate is 0 (unlimited), returns immediately.
    /// Otherwise sleeps for the required interval if insufficient tokens.
    pub async fn acquire(&self, bytes: usize) {
        let rate = self.rate_bytes_per_sec.load(Ordering::Relaxed);
        if rate == 0 || bytes == 0 {
            return;
        }

        let rate_f = rate as f64;
        let burst_capacity = (rate_f * 2.0).max(128.0 * 1024.0); // Allow up to 2 sec burst or 128KB minimum

        loop {
            let sleep_duration = {
                let mut guard = self.state.lock().await;
                let now = Instant::now();
                let elapsed = now.duration_since(guard.last_refill).as_secs_f64();
                guard.last_refill = now;

                // Replenish tokens based on elapsed time
                guard.tokens = (guard.tokens + elapsed * rate_f).min(burst_capacity);

                let needed = bytes as f64;
                if guard.tokens >= needed {
                    guard.tokens -= needed;
                    return;
                }

                // Insufficient tokens: calculate wait duration
                let deficit = needed - guard.tokens;
                Duration::from_secs_f64(deficit / rate_f)
            };

            // Sleep without holding lock
            tokio::time::sleep(sleep_duration).await;
        }
    }
}


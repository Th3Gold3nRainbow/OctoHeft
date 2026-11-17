use std::time::{Duration, Instant};

/// Fixed heartbeat interval at which download progress and smoothed speed are sampled and emitted (2 Hz).
pub const TELEMETRY_TICK_INTERVAL: Duration = Duration::from_millis(500);

/// EWMA damping factor (alpha in [0.25, 0.30]) balancing responsiveness with high-throughput stability.
pub const DEFAULT_EWMA_ALPHA: f64 = 0.28;

/// Number of consecutive zero-byte heartbeat ticks before decaying/resetting displayed speed to 0
/// (2 ticks × 500 ms = 1.0 second grace window).
pub const ZERO_SPEED_GRACE_TICKS: u32 = 2;

/// Minimum elapsed time guard to prevent division spikes on sub-interval calls.
const MIN_DT_SECS: f64 = 0.05;

/// Stateful Exponentially Weighted Moving Average (EWMA) speed tracker with zero-speed hysteresis.
#[derive(Debug, Clone)]
pub struct EwmaSpeedTracker {
    alpha: f64,
    grace_ticks: u32,
    smoothed_speed: f64,
    last_bytes: u64,
    last_sample: Option<Instant>,
    consecutive_zero_ticks: u32,
}

impl EwmaSpeedTracker {
    pub fn new(initial_bytes: u64) -> Self {
        Self::with_config(initial_bytes, DEFAULT_EWMA_ALPHA, ZERO_SPEED_GRACE_TICKS)
    }

    pub fn with_config(initial_bytes: u64, alpha: f64, grace_ticks: u32) -> Self {
        Self {
            alpha: alpha.clamp(0.05, 1.0),
            grace_ticks: grace_ticks.max(1),
            smoothed_speed: 0.0,
            last_bytes: initial_bytes,
            last_sample: None,
            consecutive_zero_ticks: 0,
        }
    }

    /// Anchors the sampling clock right when a transfer begins without emitting a startup sample.
    pub fn seed_clock(&mut self, initial_bytes: u64, now: Instant) {
        self.last_bytes = initial_bytes;
        self.last_sample = Some(now);
        self.consecutive_zero_ticks = 0;
    }

    /// Samples cumulative downloaded bytes at `now` and returns smoothed bytes/sec.
    pub fn sample_at(&mut self, current_bytes: u64, now: Instant) -> u64 {
        let prev_time = match self.last_sample {
            Some(t) => t,
            None => {
                self.last_bytes = current_bytes;
                self.last_sample = Some(now);
                return self.current_speed();
            }
        };

        let dt = now.saturating_duration_since(prev_time).as_secs_f64();
        if dt < MIN_DT_SECS {
            return self.current_speed();
        }

        let delta_bytes = current_bytes.saturating_sub(self.last_bytes);
        self.last_bytes = current_bytes;
        self.last_sample = Some(now);

        if delta_bytes == 0 {
            self.consecutive_zero_ticks = self.consecutive_zero_ticks.saturating_add(1);
            if self.consecutive_zero_ticks >= self.grace_ticks {
                self.smoothed_speed = 0.0;
            }
            return self.current_speed();
        }

        self.consecutive_zero_ticks = 0;
        let instant_speed = (delta_bytes as f64) / dt;

        if self.smoothed_speed <= 0.0 {
            // Seed directly from first active measurement so startup ramps immediately.
            self.smoothed_speed = instant_speed;
        } else {
            self.smoothed_speed =
                (self.alpha * instant_speed) + ((1.0 - self.alpha) * self.smoothed_speed);
        }

        self.current_speed()
    }

    /// Returns the current smoothed throughput in bytes/sec.
    pub fn current_speed(&self) -> u64 {
        self.smoothed_speed.round().max(0.0) as u64
    }

    /// Resets smoothed speed cleanly to 0 (used on pause, error, stop, or completion).
    pub fn reset(&mut self, current_bytes: u64) {
        self.smoothed_speed = 0.0;
        self.consecutive_zero_ticks = 0;
        self.last_bytes = current_bytes;
        self.last_sample = None;
    }

    /// Computes stabilized ETA in seconds from current smoothed speed.
    pub fn compute_eta(&self, total_size: u64, downloaded_bytes: u64) -> Option<u64> {
        let speed = self.current_speed();
        if speed == 0 || total_size == 0 {
            None
        } else {
            let remaining = total_size.saturating_sub(downloaded_bytes);
            Some(remaining / speed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn test_ewma_startup_ramp_and_burst_damping() {
        let t0 = Instant::now();
        let mut tracker = EwmaSpeedTracker::new(0);
        tracker.seed_clock(0, t0);

        // Tick 1 (+500ms): 125 MB in 0.5s = 250 MB/s -> seeded directly from 0
        let t1 = t0 + Duration::from_millis(500);
        let s1 = tracker.sample_at(125 * MB, t1);
        assert_eq!(s1, 250 * MB);

        // Tick 2 (+500ms): temporary slow TCP window (10 MB in 0.5s = 20 MB/s raw)
        // EWMA = 0.28 * 20 + 0.72 * 250 = 185.6 MB/s (does NOT collapse to 20 MB/s)
        let t2 = t1 + Duration::from_millis(500);
        let s2 = tracker.sample_at(135 * MB, t2);
        let expected_s2 = ((0.28 * (20 * MB) as f64) + (0.72 * (250 * MB) as f64)).round() as u64;
        assert_eq!(s2, expected_s2);
        assert!(s2 > 180 * MB && s2 < 190 * MB);

        // Tick 3 (+500ms): burst back to 250 MB/s -> climbs smoothly
        let t3 = t2 + Duration::from_millis(500);
        let s3 = tracker.sample_at(260 * MB, t3);
        assert!(s3 > s2 && s3 < 250 * MB);
    }

    #[test]
    fn test_zero_speed_hysteresis_holds_for_one_second_grace_window() {
        let t0 = Instant::now();
        let mut tracker = EwmaSpeedTracker::new(0);
        tracker.seed_clock(0, t0);

        // Establish 200 MB/s steady state
        let t1 = t0 + Duration::from_millis(500);
        let s1 = tracker.sample_at(100 * MB, t1);
        assert_eq!(s1, 200 * MB);

        // Tick 2 (+500ms): 0 delta bytes (transient TCP ACK stall) -> MUST hold 200 MB/s
        let t2 = t1 + Duration::from_millis(500);
        let s2 = tracker.sample_at(100 * MB, t2);
        assert_eq!(s2, 200 * MB, "Single zero-byte 500ms tick must NOT drop speed to 0");

        // Case A: Transfer resumes on Tick 3 (+500ms) with 100 MB -> zero counter resets and smooths seamlessly
        let mut recovered = tracker.clone();
        let t3 = t2 + Duration::from_millis(500);
        let s3_rec = recovered.sample_at(200 * MB, t3);
        assert_eq!(s3_rec, 200 * MB);

        // Case B: Still 0 delta bytes on Tick 3 (+1000ms total stall) -> grace window exhausted, drops cleanly to 0
        let s3_drop = tracker.sample_at(100 * MB, t3);
        assert_eq!(s3_drop, 0, "Two consecutive zero-byte 500ms ticks (1s) must reset speed to 0");
    }

    #[test]
    fn test_reset_and_eta_calculation() {
        let t0 = Instant::now();
        let mut tracker = EwmaSpeedTracker::new(0);
        tracker.seed_clock(0, t0);

        let t1 = t0 + Duration::from_millis(500);
        let speed = tracker.sample_at(50 * MB, t1);
        assert_eq!(speed, 100 * MB);
        assert_eq!(tracker.compute_eta(550 * MB, 50 * MB), Some(5));

        tracker.reset(50 * MB);
        assert_eq!(tracker.current_speed(), 0);
        assert_eq!(tracker.compute_eta(550 * MB, 50 * MB), None);
    }
}


use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Fixed heartbeat interval at which download progress and smoothed speed are sampled and emitted (5 Hz).
pub const TELEMETRY_TICK_INTERVAL: Duration = Duration::from_millis(200);

/// EWMA damping factor (alpha in [0.25, 0.30]) balancing responsiveness with high-throughput stability.
pub const DEFAULT_EWMA_ALPHA: f64 = 0.28;

/// Number of consecutive zero-byte heartbeat ticks before decaying/resetting displayed speed to 0
/// (5 ticks × 200 ms = 1.0 second grace window).
pub const ZERO_SPEED_GRACE_TICKS: u32 = 5;

/// Rolling sliding window duration for instant throughput calculation (1.0s window).
pub const ROLLING_WINDOW_DURATION: Duration = Duration::from_millis(1000);

/// Minimum elapsed time guard to prevent division spikes on sub-interval calls.
const MIN_DT_SECS: f64 = 0.05;

/// Stateful Exponentially Weighted Moving Average (EWMA) speed tracker with a rolling sliding window
/// and zero-speed hysteresis.
#[derive(Debug, Clone)]
pub struct EwmaSpeedTracker {
    alpha: f64,
    grace_ticks: u32,
    window_duration: Duration,
    smoothed_speed: f64,
    last_bytes: u64,
    last_sample: Option<Instant>,
    consecutive_zero_ticks: u32,
    samples: VecDeque<(Instant, u64)>,
}

impl EwmaSpeedTracker {
    pub fn new(initial_bytes: u64) -> Self {
        Self::with_window(
            initial_bytes,
            DEFAULT_EWMA_ALPHA,
            ZERO_SPEED_GRACE_TICKS,
            ROLLING_WINDOW_DURATION,
        )
    }

    pub fn with_config(initial_bytes: u64, alpha: f64, grace_ticks: u32) -> Self {
        Self::with_window(initial_bytes, alpha, grace_ticks, ROLLING_WINDOW_DURATION)
    }

    pub fn with_window(
        initial_bytes: u64,
        alpha: f64,
        grace_ticks: u32,
        window_duration: Duration,
    ) -> Self {
        Self {
            alpha: alpha.clamp(0.05, 1.0),
            grace_ticks: grace_ticks.max(1),
            window_duration,
            smoothed_speed: 0.0,
            last_bytes: initial_bytes,
            last_sample: None,
            consecutive_zero_ticks: 0,
            samples: VecDeque::new(),
        }
    }

    /// Anchors the sampling clock right when a transfer begins without emitting a startup sample.
    pub fn seed_clock(&mut self, initial_bytes: u64, now: Instant) {
        self.last_bytes = initial_bytes;
        self.last_sample = Some(now);
        self.consecutive_zero_ticks = 0;
        self.samples.clear();
        self.samples.push_back((now, initial_bytes));
    }

    /// Samples cumulative downloaded bytes at `now` and returns smoothed bytes/sec.
    pub fn sample_at(&mut self, current_bytes: u64, now: Instant) -> u64 {
        let prev_time = match self.last_sample {
            Some(t) => t,
            None => {
                self.last_bytes = current_bytes;
                self.last_sample = Some(now);
                self.samples.clear();
                self.samples.push_back((now, current_bytes));
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
                self.samples.clear();
            }
            return self.current_speed();
        }

        self.consecutive_zero_ticks = 0;

        // If telemetry was halted for a significant gap (e.g., long pause), restart window
        if dt > self.window_duration.as_secs_f64() * 2.0 {
            self.samples.clear();
            self.samples.push_back((prev_time, current_bytes.saturating_sub(delta_bytes)));
        }

        self.samples.push_back((now, current_bytes));

        // Retain samples within window_duration, keeping the sample that spans across the window boundary
        while self.samples.len() > 2 {
            if let Some(&(second_time, _)) = self.samples.get(1) {
                if now.saturating_duration_since(second_time) >= self.window_duration {
                    self.samples.pop_front();
                    continue;
                }
            }
            break;
        }

        let (front_time, front_bytes) = self
            .samples
            .front()
            .copied()
            .unwrap_or((prev_time, current_bytes.saturating_sub(delta_bytes)));
        let span_dt = now.saturating_duration_since(front_time).as_secs_f64();
        let span_bytes = current_bytes.saturating_sub(front_bytes);

        let instant_speed = if span_dt >= MIN_DT_SECS {
            (span_bytes as f64) / span_dt
        } else {
            (delta_bytes as f64) / dt
        };

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
        self.samples.clear();
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

        // Tick 1 (+200ms): 50 MB in 0.2s = 250 MB/s -> seeded directly from 0
        let t1 = t0 + Duration::from_millis(200);
        let s1 = tracker.sample_at(50 * MB, t1);
        assert_eq!(s1, 250 * MB);

        // Tick 2 (+200ms): temporary slow TCP window (2 MB in 0.2s = 10 MB/s raw)
        // Rolling window: (52 MB in 0.4s = 130 MB/s instant_speed)
        // EWMA smooths: 0.28 * 130 + 0.72 * 250 = 216.4 MB/s (does NOT collapse to 10 MB/s)
        let t2 = t1 + Duration::from_millis(200);
        let s2 = tracker.sample_at(52 * MB, t2);
        assert!(s2 > 200 * MB && s2 < 250 * MB);

        // Tick 3 (+200ms): burst back
        let t3 = t2 + Duration::from_millis(200);
        let s3 = tracker.sample_at(102 * MB, t3);
        assert!(s3 > s2);
    }

    #[test]
    fn test_zero_speed_hysteresis_holds_for_one_second_grace_window() {
        let t0 = Instant::now();
        let mut tracker = EwmaSpeedTracker::new(0);
        tracker.seed_clock(0, t0);

        // Establish 200 MB/s steady state (40 MB in 200ms = 200 MB/s)
        let t1 = t0 + Duration::from_millis(200);
        let s1 = tracker.sample_at(40 * MB, t1);
        assert_eq!(s1, 200 * MB);

        // Ticks 2..5 (+200ms each, 4 ticks total = 800ms of stall): 0 delta bytes -> MUST hold 200 MB/s
        let mut cur_t = t1;
        for _ in 1..=4 {
            cur_t += Duration::from_millis(200);
            let s = tracker.sample_at(40 * MB, cur_t);
            assert_eq!(s, 200 * MB, "Sub-second zero-byte ticks must NOT drop speed to 0");
        }

        // Case A: Transfer resumes on Tick 6 (+200ms) with 40 MB -> zero counter resets and smooths seamlessly
        let mut recovered = tracker.clone();
        let t6 = cur_t + Duration::from_millis(200);
        let s6_rec = recovered.sample_at(80 * MB, t6);
        assert!(s6_rec > 0, "Recovered tracker must report non-zero speed");

        // Case B: Still 0 delta bytes on Tick 6 (5 consecutive zero ticks = 1.0s stall) -> grace window exhausted, drops cleanly to 0
        let s6_drop = tracker.sample_at(40 * MB, t6);
        assert_eq!(s6_drop, 0, "Five consecutive zero-byte 200ms ticks (1s) must reset speed to 0");
    }

    #[test]
    fn test_reset_and_eta_calculation() {
        let t0 = Instant::now();
        let mut tracker = EwmaSpeedTracker::new(0);
        tracker.seed_clock(0, t0);

        let t1 = t0 + Duration::from_millis(200);
        let speed = tracker.sample_at(20 * MB, t1);
        assert_eq!(speed, 100 * MB);
        assert_eq!(tracker.compute_eta(520 * MB, 20 * MB), Some(5));

        tracker.reset(20 * MB);
        assert_eq!(tracker.current_speed(), 0);
        assert_eq!(tracker.compute_eta(520 * MB, 20 * MB), None);
    }
}

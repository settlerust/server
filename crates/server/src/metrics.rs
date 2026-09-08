//! Bounded measurements of command latency and timer delay, without participant data.
use std::sync::atomic::{AtomicU64, Ordering};
const BOUNDS: [u64; 14] = [
    1,
    2,
    5,
    10,
    20,
    50,
    100,
    150,
    250,
    500,
    1000,
    2500,
    5000,
    u64::MAX,
];
pub struct Metrics {
    commands: [AtomicU64; 14],
    tick_lag: AtomicU64,
}
impl Default for Metrics {
    fn default() -> Self {
        Self {
            commands: std::array::from_fn(|_| AtomicU64::new(0)),
            tick_lag: AtomicU64::new(0),
        }
    }
}
impl Metrics {
    pub fn command(&self, elapsed_ms: u64) {
        let bucket = BOUNDS
            .iter()
            .position(|&bound| elapsed_ms <= bound)
            .unwrap_or(13);
        self.commands[bucket].fetch_add(1, Ordering::Relaxed);
    }
    pub fn tick_lag(&self, elapsed_ms: u64) {
        self.tick_lag.fetch_max(elapsed_ms, Ordering::Relaxed);
    }
    pub fn take(&self) -> (u64, u64, u64) {
        let counts = self
            .commands
            .each_ref()
            .map(|v| v.swap(0, Ordering::Relaxed));
        let total: u64 = counts.iter().sum();
        let mut through = 0;
        let mut p95 = 0;
        if total > 0 {
            for (index, count) in counts.iter().enumerate() {
                through += count;
                if through >= (total * 95).div_ceil(100) {
                    p95 = BOUNDS[index].min(10000);
                    break;
                }
            }
        }
        (total, p95, self.tick_lag.swap(0, Ordering::Relaxed))
    }
}

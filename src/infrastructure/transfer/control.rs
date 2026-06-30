//! Adaptive control loop for the parallel transfer core (Deliverable 2).
//!
//! Implements **AIMD** (additive-increase / multiplicative-decrease) over the number
//! of chunks allowed in flight, exactly like TCP congestion control:
//!   * when throughput keeps improving and there's no resource pressure, we *add* one
//!     to the in-flight window (probe for more bandwidth), and
//!   * when throughput drops or RAM pressure appears, we *halve* the window (back off
//!     fast to relieve congestion / memory).
//!
//! The window is hard-bounded by the plan's `max_in_flight` and by a memory budget, so
//! peak buffer memory is always `window * chunk_size <= memory_budget`. A
//! `DynamicSemaphore` realises the window for the worker pool.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tracing::debug;

/// A counting semaphore whose capacity can change at runtime (the AIMD window).
pub struct DynamicSemaphore {
    state: Mutex<SemState>,
    cv: Condvar,
}

struct SemState {
    capacity: u32,
    in_use: u32,
}

impl DynamicSemaphore {
    pub fn new(capacity: u32) -> Self {
        Self {
            state: Mutex::new(SemState { capacity: capacity.max(1), in_use: 0 }),
            cv: Condvar::new(),
        }
    }

    /// Block until a permit is free, then take it.
    pub fn acquire(&self) {
        let mut s = self.state.lock().unwrap();
        while s.in_use >= s.capacity {
            s = self.cv.wait(s).unwrap();
        }
        s.in_use += 1;
    }

    pub fn release(&self) {
        let mut s = self.state.lock().unwrap();
        s.in_use = s.in_use.saturating_sub(1);
        drop(s);
        self.cv.notify_one();
    }

    /// Resize the window. Growing wakes any waiters.
    pub fn set_capacity(&self, capacity: u32) {
        let mut s = self.state.lock().unwrap();
        let grew = capacity > s.capacity;
        s.capacity = capacity.max(1);
        drop(s);
        if grew {
            self.cv.notify_all();
        }
    }

    pub fn capacity(&self) -> u32 {
        self.state.lock().unwrap().capacity
    }
}

/// Live counters updated by workers; read by the controller.
#[derive(Default)]
pub struct TransferStats {
    pub bytes_done: AtomicU64,
    pub chunks_done: AtomicU64,
    /// Wire bytes actually moved (post-compression) — for ratio reporting.
    pub wire_bytes: AtomicU64,
}

impl TransferStats {
    pub fn record_chunk(&self, plaintext_len: u64, wire_len: u64) {
        self.bytes_done.fetch_add(plaintext_len, Ordering::Relaxed);
        self.wire_bytes.fetch_add(wire_len, Ordering::Relaxed);
        self.chunks_done.fetch_add(1, Ordering::Relaxed);
    }
}

/// Parameters governing the AIMD loop.
pub struct ControllerConfig {
    pub min_window: u32,
    pub max_window: u32,
    pub chunk_size: u64,
    pub memory_budget: u64,
    /// How often the loop re-evaluates throughput.
    pub tick: Duration,
    /// Back off if available RAM drops below this many bytes.
    pub ram_floor_bytes: u64,
}

/// Runs the AIMD loop in a background thread until `stop` is set. Returns a handle
/// whose `history` (window sizes over time) is available for tests/observability.
pub struct AdaptiveController {
    pub sem: Arc<DynamicSemaphore>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<Vec<u32>>>,
}

impl AdaptiveController {
    pub fn start(stats: Arc<TransferStats>, cfg: ControllerConfig) -> Self {
        // Start at a conservative window (2) and probe upward — classic slow-ish start.
        let start_window = cfg.min_window.clamp(1, cfg.max_window);
        let sem = Arc::new(DynamicSemaphore::new(start_window));
        let stop = Arc::new(AtomicBool::new(false));

        let sem_c = sem.clone();
        let stop_c = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut history = vec![start_window];
            let mut last_bytes = 0u64;
            let mut last_rate = 0.0f64;
            let mut last_t = Instant::now();
            // Memory hard-cap on the window regardless of AIMD decisions.
            let mem_cap = ((cfg.memory_budget / cfg.chunk_size.max(1)) as u32)
                .clamp(cfg.min_window, cfg.max_window);

            while !stop_c.load(Ordering::Relaxed) {
                std::thread::sleep(cfg.tick);
                let now = Instant::now();
                let dt = now.duration_since(last_t).as_secs_f64().max(1e-6);
                let bytes = stats.bytes_done.load(Ordering::Relaxed);
                let rate = (bytes.saturating_sub(last_bytes)) as f64 / dt; // bytes/s

                let cur = sem_c.capacity();
                let pressure = available_ram() < cfg.ram_floor_bytes;

                let next = if pressure {
                    // Multiplicative decrease — relieve memory fast.
                    (cur / 2).max(cfg.min_window)
                } else if rate > last_rate * 1.02 {
                    // Still improving → additive increase (probe for more).
                    (cur + 1).min(mem_cap)
                } else if rate < last_rate * 0.95 {
                    // Throughput regressed → multiplicative decrease (congestion).
                    (cur / 2).max(cfg.min_window)
                } else {
                    cur // plateau: hold
                };

                if next != cur {
                    sem_c.set_capacity(next);
                    debug!(
                        window = next,
                        prev = cur,
                        rate_mbps = rate / 1e6,
                        pressure,
                        "AIMD window adjusted"
                    );
                }
                history.push(next);
                last_bytes = bytes;
                last_rate = rate;
                last_t = now;
            }
            history
        });

        Self { sem, stop, handle: Some(handle) }
    }

    /// Signal the loop to stop and collect the window history.
    pub fn stop(mut self) -> Vec<u32> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().map(|h| h.join().unwrap_or_default()).unwrap_or_default()
    }
}

/// Best-effort current available RAM (bytes). Cheap enough to poll on each tick.
fn available_ram() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.available_memory()
}

/// A thread-safe token bucket rate limiter for bandwidth throttling (JULES-10).
pub struct TokenBucket {
    max_rate: u64, // bytes per second
    capacity: u64,
    state: Mutex<BucketState>,
}

struct BucketState {
    tokens: f64,
    last_replenish: Instant,
}

impl TokenBucket {
    pub fn new(max_rate: u64) -> Self {
        Self {
            max_rate,
            capacity: max_rate.max(1024 * 1024), // at least 1 MiB burst capacity
            state: Mutex::new(BucketState {
                tokens: max_rate as f64,
                last_replenish: Instant::now(),
            }),
        }
    }

    pub fn consume(&self, tokens_needed: u64) {
        if self.max_rate == 0 {
            return;
        }
        loop {
            let mut state = self.state.lock().unwrap();
            let now = Instant::now();
            let elapsed = now.duration_since(state.last_replenish).as_secs_f64();
            state.last_replenish = now;

            let new_tokens = state.tokens + elapsed * self.max_rate as f64;
            state.tokens = new_tokens.min(self.capacity as f64);

            if state.tokens >= tokens_needed as f64 {
                state.tokens -= tokens_needed as f64;
                return;
            }

            // Calculate sleep time
            let missing = tokens_needed as f64 - state.tokens;
            let sleep_dur = Duration::from_secs_f64(missing / self.max_rate as f64);

            // Release lock before sleeping to let other threads progress/replenish
            drop(state);
            std::thread::sleep(sleep_dur);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_semaphore_bounds_concurrency() {
        let sem = Arc::new(DynamicSemaphore::new(2));
        let live = Arc::new(AtomicU64::new(0));
        let peak = Arc::new(AtomicU64::new(0));
        std::thread::scope(|s| {
            for _ in 0..8 {
                let sem = sem.clone();
                let live = live.clone();
                let peak = peak.clone();
                s.spawn(move || {
                    sem.acquire();
                    let n = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    live.fetch_sub(1, Ordering::SeqCst);
                    sem.release();
                });
            }
        });
        // Never more than the capacity (2) concurrent holders.
        assert!(peak.load(Ordering::SeqCst) <= 2);
    }

    #[test]
    fn semaphore_resize_grows_window() {
        let sem = DynamicSemaphore::new(1);
        assert_eq!(sem.capacity(), 1);
        sem.set_capacity(5);
        assert_eq!(sem.capacity(), 5);
    }

    #[test]
    fn aimd_grows_window_under_increasing_throughput() {
        let stats = Arc::new(TransferStats::default());
        let ctrl = AdaptiveController::start(
            stats.clone(),
            ControllerConfig {
                min_window: 2,
                max_window: 16,
                chunk_size: 1024,
                memory_budget: 16 * 1024,
                tick: Duration::from_millis(20),
                ram_floor_bytes: 0, // no pressure in the test
            },
        );
        // Simulate ever-increasing throughput.
        for _ in 0..6 {
            stats.bytes_done.fetch_add(1_000_000, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(25));
        }
        let history = ctrl.stop();
        let peak = history.iter().copied().max().unwrap_or(0);
        assert!(peak > 2, "AIMD should have grown the window above the start of 2 (got {peak})");
        assert!(peak <= 16, "window must respect max_window");
    }

    #[test]
    fn token_bucket_throttles_rate() {
        let bucket = TokenBucket::new(10_000); // 10 KB/s
        let start = Instant::now();
        // Consume 20 KB. Since rate is 10 KB/s, this should take at least 1 second.
        bucket.consume(10_000);
        bucket.consume(10_000);
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(900), "should have throttled (elapsed: {:?})", elapsed);
    }
}

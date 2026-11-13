//! DEDICATED rayon thread pool (not the global one).
//!
//! Rationale: other libraries in the process (or the game itself) may use rayon's global
//! pool; with our own we control the thread count and avoid competing with the
//! render/server thread. By default it uses half of the logical cores (minimum 1).

use rayon::{ThreadPool, ThreadPoolBuilder};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

static REQUESTED: AtomicUsize = AtomicUsize::new(0);
static POOL: OnceLock<ThreadPool> = OnceLock::new();

fn default_threads() -> usize {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    (n / 2).max(1)
}

/// Configures the thread count. Only has an effect before first use. Returns true if applied.
pub fn configure(threads: usize) -> bool {
    if POOL.get().is_some() {
        return false;
    }
    REQUESTED.store(threads, Ordering::Relaxed);
    true
}

pub fn pool() -> &'static ThreadPool {
    POOL.get_or_init(|| {
        let req = REQUESTED.load(Ordering::Relaxed);
        let n = if req == 0 { default_threads() } else { req.min(256) };
        ThreadPoolBuilder::new()
            .num_threads(n)
            .thread_name(|i| format!("rustlib-{i}"))
            .build()
            .unwrap_or_else(|_| ThreadPoolBuilder::new().num_threads(1).build().expect("rayon pool"))
    })
}

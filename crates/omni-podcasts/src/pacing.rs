//! Client-side request pacing shared by the Castro and Podcast Index clients:
//! a fixed-window request budget plus a concurrency cap, so every request
//! (retries included) consumes its own token and permit.

use std::time::Duration;

use tokio::sync::{Mutex, Semaphore, SemaphorePermit};
use tokio::time::Instant;

#[derive(Debug)]
struct Window {
    start: Option<Instant>,
    used: u32,
}

/// `takeRatePermit` + `Semaphore.withPermits(1)` from the TS clients.
#[derive(Debug)]
pub struct RequestPacer {
    permits: Semaphore,
    window: Mutex<Window>,
    max_per_interval: u32,
    interval: Duration,
}

impl RequestPacer {
    pub fn new(max_concurrent: usize, max_per_interval: u32, interval: Duration) -> Self {
        Self {
            permits: Semaphore::new(max_concurrent),
            window: Mutex::new(Window {
                start: None,
                used: 0,
            }),
            max_per_interval,
            interval,
        }
    }

    /// Waits for a rate token, then for a concurrency permit held until the
    /// returned guard drops.
    pub async fn acquire(&self) -> Option<SemaphorePermit<'_>> {
        loop {
            let wait = {
                let mut window = self.window.lock().await;
                let now = Instant::now();
                match window.start {
                    Some(start) if now.duration_since(start) < self.interval => {
                        if window.used < self.max_per_interval {
                            window.used += 1;
                            None
                        } else {
                            Some(start + self.interval - now)
                        }
                    }
                    _ => {
                        window.start = Some(now);
                        window.used = 1;
                        None
                    }
                }
            };
            match wait {
                Some(delay) => tokio::time::sleep(delay).await,
                None => break,
            }
        }
        self.permits.acquire().await.ok()
    }
}

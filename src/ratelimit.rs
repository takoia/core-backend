//! Sliding-window rate limiter for unauthenticated or cheap-to-call surfaces
//! (inbound webhooks). In-process: this binary is a single instance per
//! database, and the limit protects the process, not a distributed quota.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct SlidingWindow {
    per_min: u32,
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl SlidingWindow {
    /// `per_min == 0` disables the limiter (every call is allowed).
    pub fn new(per_min: u32) -> Self {
        Self {
            per_min,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Record one attempt for `key` at `now` and say whether it is within the
    /// limit. Attempts are counted whether or not they are allowed, so a flood
    /// stays refused until it stops.
    pub fn allow_at(&self, key: &str, now: Instant) -> bool {
        if self.per_min == 0 {
            return true;
        }
        let window = Duration::from_secs(60);
        let mut hits = match self.hits.lock() {
            Ok(h) => h,
            Err(poisoned) => poisoned.into_inner(),
        };
        let q = hits.entry(key.to_string()).or_default();
        while let Some(front) = q.front() {
            if now.duration_since(*front) >= window {
                q.pop_front();
            } else {
                break;
            }
        }
        let allowed = (q.len() as u32) < self.per_min;
        q.push_back(now);
        // Keep the map from growing with dead keys.
        if q.len() > (self.per_min as usize) * 2 {
            let excess = q.len() - self.per_min as usize;
            q.drain(..excess);
        }
        allowed
    }

    pub fn allow(&self, key: &str) -> bool {
        self.allow_at(key, Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_the_limit_then_refuses_until_the_window_slides() {
        let l = SlidingWindow::new(3);
        let t0 = Instant::now();
        assert!(l.allow_at("ev", t0));
        assert!(l.allow_at("ev", t0 + Duration::from_secs(1)));
        assert!(l.allow_at("ev", t0 + Duration::from_secs(2)));
        assert!(
            !l.allow_at("ev", t0 + Duration::from_secs(3)),
            "4th within a minute"
        );
        assert!(
            l.allow_at("other", t0 + Duration::from_secs(3)),
            "keys are independent"
        );
        // Refused attempts count too: still refused 30s later.
        assert!(!l.allow_at("ev", t0 + Duration::from_secs(30)));
        // Well after the window everything before has expired.
        assert!(l.allow_at("ev", t0 + Duration::from_secs(200)));
    }

    #[test]
    fn zero_disables() {
        let l = SlidingWindow::new(0);
        for _ in 0..1000 {
            assert!(l.allow("ev"));
        }
    }
}

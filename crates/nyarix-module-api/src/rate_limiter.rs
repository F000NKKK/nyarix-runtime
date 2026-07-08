//! I/O rate limiting (see issue #78's "Rate limiting на I/O операций").
//!
//! A plain token bucket: `capacity` tokens available up front,
//! refilling at `refill_per_second`, each [`RateLimiter::try_acquire`]
//! call consuming one. Used by
//! [`crate::context::RuntimeContext::check_file_access`]/
//! [`crate::context::RuntimeContext::check_network_access`] to bound
//! how often a module can make it through the I/O whitelist checks,
//! independent of whether the destination itself is allowed.

use std::sync::Mutex;
use std::time::Instant;

#[derive(Debug)]
struct State {
    tokens: f64,
    last_refill: Instant,
}

/// A token-bucket rate limiter, safe to share across calls via `&self`
/// (interior mutability, no external locking needed).
#[derive(Debug)]
pub struct RateLimiter {
    capacity: f64,
    refill_per_second: f64,
    state: Mutex<State>,
}

impl RateLimiter {
    /// Build a limiter starting with a full bucket of `capacity`
    /// tokens, refilling at `refill_per_second` tokens/second.
    #[must_use]
    pub fn new(capacity: u32, refill_per_second: u32) -> Self {
        Self {
            capacity: f64::from(capacity),
            refill_per_second: f64::from(refill_per_second),
            state: Mutex::new(State {
                tokens: f64::from(capacity),
                last_refill: Instant::now(),
            }),
        }
    }

    /// Try to consume one token. Returns `true` (and consumes a token)
    /// if one was available, `false` (consuming nothing) otherwise.
    #[must_use]
    pub fn try_acquire(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let now = Instant::now();
        let elapsed = now.duration_since(state.last_refill).as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.refill_per_second).min(self.capacity);
        state.last_refill = now;

        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_a_full_bucket() {
        let limiter = RateLimiter::new(3, 1);
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
    }

    #[test]
    fn refuses_once_the_bucket_is_empty() {
        let limiter = RateLimiter::new(1, 0);
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn refills_over_time() {
        let limiter = RateLimiter::new(1, 1000);
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());

        std::thread::sleep(std::time::Duration::from_millis(20));

        assert!(limiter.try_acquire());
    }

    #[test]
    fn never_exceeds_capacity_even_after_a_long_idle_period() {
        let limiter = RateLimiter::new(2, 1000);
        std::thread::sleep(std::time::Duration::from_millis(50));

        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());
    }
}

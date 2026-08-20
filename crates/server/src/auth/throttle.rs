//! Per-source sign-in throttling.
//!
//! Account lockout (`auth::local`) counts failures per email. That stops
//! someone grinding one account, and does nothing whatever about the shape
//! credential stuffing actually takes — one source trying one likely password
//! against a thousand accounts, never reaching five failures on any of them.
//! It also hands anyone who knows a colleague's address a way to keep that
//! account locked. Throttling the *origin* addresses both: it is the control
//! that scales with the attacker rather than with the victim.
//!
//! In-process and in-memory, deliberately. The server is a single process,
//! and the alternative — a table — would put a write on the unauthenticated
//! path, which is the one place an attacker controls the request rate.
//! A restart clears the counters; account lockout, which is persisted,
//! remains the backstop that does not.
//!
//! Nothing here authorizes anything: the address comes from a header a client
//! can forge (see `auth::client_addr`), so forging it can only ever cost the
//! forger their own throttle bucket, never open a door. A request the
//! deployment cannot attribute an address to is not throttled at all.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Failures tolerated inside `window` before a source starts being told to
/// wait. Well above the five that lock a single account, so an operator
/// fat-fingering their own password a few times is never affected.
const FAILURES_BEFORE_DELAY: u32 = 10;
const WINDOW: Duration = Duration::from_secs(15 * 60);
/// First penalty, doubling per further failure, capped by `MAX_DELAY`.
const BASE_DELAY: Duration = Duration::from_secs(30);
const MAX_DELAY: Duration = Duration::from_secs(15 * 60);
/// Buckets idle for longer than this are dropped on the next sweep, so the
/// map cannot grow without bound on a long-lived process.
const IDLE_EVICT: Duration = Duration::from_secs(60 * 60);

#[derive(Debug)]
struct Bucket {
    failures: u32,
    last: Instant,
    blocked_until: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct Throttle {
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl Throttle {
    pub fn new() -> Self {
        Throttle::default()
    }

    /// How long this source must wait, if it currently must. `None` means the
    /// attempt may proceed — including for a source with no address at all,
    /// which cannot be told apart from any other.
    pub fn retry_after(&self, source: Option<&str>, now: Instant) -> Option<Duration> {
        let source = source?;
        let buckets = self.buckets.lock().ok()?;
        let bucket = buckets.get(source)?;
        let until = bucket.blocked_until?;
        (until > now).then(|| until - now)
    }

    /// One failed sign-in from this source. Failures older than `WINDOW`
    /// count for nothing, so a slow trickle never accumulates into a block.
    pub fn record_failure(&self, source: Option<&str>, now: Instant) {
        let Some(source) = source else { return };
        let Ok(mut buckets) = self.buckets.lock() else { return };
        buckets.retain(|_, b| now.duration_since(b.last) < IDLE_EVICT);
        let bucket = buckets.entry(source.to_string()).or_insert(Bucket {
            failures: 0,
            last: now,
            blocked_until: None,
        });
        if now.duration_since(bucket.last) > WINDOW {
            bucket.failures = 0;
            bucket.blocked_until = None;
        }
        bucket.failures += 1;
        bucket.last = now;
        if bucket.failures > FAILURES_BEFORE_DELAY {
            let steps = bucket.failures - FAILURES_BEFORE_DELAY - 1;
            let delay = BASE_DELAY
                .checked_mul(1u32.checked_shl(steps.min(16)).unwrap_or(u32::MAX))
                .unwrap_or(MAX_DELAY)
                .min(MAX_DELAY);
            bucket.blocked_until = Some(now + delay);
        }
    }

    /// A successful sign-in clears the source: the traffic was legitimate
    /// after all, and leaving the count standing would penalise a shared
    /// office address for one person's typo streak.
    pub fn reset(&self, source: Option<&str>) {
        let Some(source) = source else { return };
        if let Ok(mut buckets) = self.buckets.lock() {
            buckets.remove(source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail_n(t: &Throttle, source: &str, n: u32, now: Instant) {
        for _ in 0..n {
            t.record_failure(Some(source), now);
        }
    }

    #[test]
    fn the_first_failures_cost_nothing() {
        let t = Throttle::new();
        let now = Instant::now();
        fail_n(&t, "a", FAILURES_BEFORE_DELAY, now);
        assert_eq!(t.retry_after(Some("a"), now), None);
    }

    #[test]
    fn the_delay_doubles_and_stops_at_the_cap() {
        let t = Throttle::new();
        let now = Instant::now();
        fail_n(&t, "a", FAILURES_BEFORE_DELAY + 1, now);
        assert_eq!(t.retry_after(Some("a"), now), Some(BASE_DELAY));

        t.record_failure(Some("a"), now);
        assert_eq!(t.retry_after(Some("a"), now), Some(BASE_DELAY * 2));

        fail_n(&t, "a", 20, now);
        assert_eq!(t.retry_after(Some("a"), now), Some(MAX_DELAY));
    }

    #[test]
    fn sources_are_counted_separately() {
        let t = Throttle::new();
        let now = Instant::now();
        fail_n(&t, "a", FAILURES_BEFORE_DELAY + 5, now);
        assert!(t.retry_after(Some("a"), now).is_some());
        assert_eq!(t.retry_after(Some("b"), now), None);
    }

    #[test]
    fn a_block_expires_on_its_own() {
        let t = Throttle::new();
        let now = Instant::now();
        fail_n(&t, "a", FAILURES_BEFORE_DELAY + 1, now);
        assert!(t.retry_after(Some("a"), now).is_some());
        assert_eq!(t.retry_after(Some("a"), now + BASE_DELAY), None);
    }

    #[test]
    fn failures_older_than_the_window_do_not_accumulate() {
        let t = Throttle::new();
        let now = Instant::now();
        fail_n(&t, "a", FAILURES_BEFORE_DELAY, now);
        let later = now + WINDOW + Duration::from_secs(1);
        t.record_failure(Some("a"), later);
        assert_eq!(t.retry_after(Some("a"), later), None, "the old streak should have lapsed");
    }

    #[test]
    fn a_successful_sign_in_clears_the_source() {
        let t = Throttle::new();
        let now = Instant::now();
        fail_n(&t, "a", FAILURES_BEFORE_DELAY + 3, now);
        assert!(t.retry_after(Some("a"), now).is_some());
        t.reset(Some("a"));
        assert_eq!(t.retry_after(Some("a"), now), None);
    }

    #[test]
    fn a_request_with_no_address_is_never_throttled() {
        let t = Throttle::new();
        let now = Instant::now();
        fail_n(&t, "a", FAILURES_BEFORE_DELAY + 3, now);
        t.record_failure(None, now);
        assert_eq!(t.retry_after(None, now), None);
    }
}

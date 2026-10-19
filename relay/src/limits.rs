//! Hourly quotas, in memory (a relay that restarts starts counting again: fine for its purpose, abuse damping). Each
//! key is a hash, forgotten within the hour: [`Quota::sweep`] runs every minute (main.rs) and drops what is older than
//! the window less a minute, so no hash is kept an hour or more.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(3600);
pub const SWEEP_EVERY: Duration = Duration::from_secs(60);
const SWEEP_ABOVE: usize = 100_000;

pub struct Quota {
    per_hour: u32,
    seen: Mutex<HashMap<[u8; 16], (Instant, u32)>>,
}

impl Quota {
    pub fn new(per_hour: u32) -> Self {
        Self { per_hour, seen: Mutex::new(HashMap::new()) }
    }

    /// Forgets the keys counted more than an hour ago, less the time until the next sweep.
    pub fn sweep(&self) {
        self.sweep_at(Instant::now());
    }

    fn sweep_at(&self, now: Instant) {
        self.seen.lock().expect("quota lock").retain(|_, (start, _)| now.duration_since(*start) < WINDOW - SWEEP_EVERY);
    }

    /// Counts one more for `key` (hashed: no token or address kept as such); false past the hour's allowance.
    pub fn allow(&self, key: &[u8]) -> bool {
        self.allow_at(key, Instant::now())
    }

    fn allow_at(&self, key: &[u8], now: Instant) -> bool {
        let digest = Sha256::digest(key);
        let mut id = [0u8; 16];
        id.copy_from_slice(&digest[..16]);
        let mut seen = self.seen.lock().expect("quota lock");
        if seen.len() > SWEEP_ABOVE {
            seen.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
        }
        let entry = seen.entry(id).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        if entry.1 >= self.per_hour {
            return false;
        }
        entry.1 += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_key_has_its_own_hourly_allowance() {
        let quota = Quota::new(2);
        let start = Instant::now();
        assert!(quota.allow_at(b"a", start) && quota.allow_at(b"a", start));
        assert!(!quota.allow_at(b"a", start));
        assert!(quota.allow_at(b"b", start));
        assert!(quota.allow_at(b"a", start + WINDOW));
    }

    #[test]
    fn nothing_is_kept_an_hour() {
        let quota = Quota::new(2);
        let start = Instant::now();
        quota.allow_at(b"a", start);
        quota.sweep_at(start + WINDOW - SWEEP_EVERY - Duration::from_secs(1));
        assert_eq!(quota.seen.lock().unwrap().len(), 1);
        quota.sweep_at(start + WINDOW - SWEEP_EVERY);
        assert!(quota.seen.lock().unwrap().is_empty());
    }
}

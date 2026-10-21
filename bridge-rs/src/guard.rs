//! Wrong keys, counted by address: an address that presents `limit` wrong keys (the bridge's, an agent's through
//! the door, a pairing proof) within `window` seconds is refused for `block` seconds, and each refusal is logged.
//! Mostly for a bridge on the Internet (a server the app created, `cloud = true`), where anyone can knock; on the
//! tailnet or a LAN it costs nothing. The app's own requests never come close: one wrong key is a key that changed.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;

pub use crate::py::Clock;

#[derive(Default)]
struct State {
    failures: HashMap<String, Vec<f64>>,
    blocked: HashMap<String, f64>,
}

pub struct Guard {
    pub limit: usize,
    pub window: f64,
    pub block: f64,
    clock: Clock,
    state: Mutex<State>,
}

impl Default for Guard {
    fn default() -> Self {
        Guard::new(20, 600.0, 900.0, crate::py::monotonic_clock())
    }
}

impl Guard {
    pub fn new(limit: usize, window: f64, block: f64, clock: Clock) -> Self {
        Guard { limit, window, block, clock, state: Mutex::new(State::default()) }
    }

    /// Not this machine itself (its own services, the main listener's proxy).
    fn counted(address: Option<&str>) -> bool {
        match address {
            None | Some("") => false,
            Some(a) => match a.parse::<IpAddr>() {
                Ok(ip) => !ip.is_loopback(),
                Err(_) => true,
            },
        }
    }

    pub fn blocked(&self, address: Option<&str>) -> bool {
        let key = address.unwrap_or("");
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(until) = state.blocked.get(key).copied() else { return false };
        if (self.clock)() >= until {
            state.blocked.remove(key);
            return false;
        }
        true
    }

    pub fn fail(&self, address: Option<&str>, what: &str) {
        if !Self::counted(address) {
            return;
        }
        let address = address.unwrap_or_default().to_string();
        let now = (self.clock)();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut recent: Vec<f64> = state
            .failures
            .get(&address)
            .map(|v| v.iter().copied().filter(|t| now - t < self.window).collect())
            .unwrap_or_default();
        recent.push(now);
        let count = recent.len();
        state.failures.insert(address.clone(), recent);
        tracing::warn!(address = %address, what = what, count = count, "wrong key");
        if count >= self.limit {
            state.blocked.insert(address.clone(), now + self.block);
            state.failures.remove(&address);
            tracing::warn!(address = %address, minutes = (self.block / 60.0).round() as i64, "address blocked");
        }
        if state.failures.len() > 10_000 {
            // never grows without end: the oldest addresses are forgotten
            let mut by_last: Vec<(String, f64)> =
                state.failures.iter().map(|(a, times)| (a.clone(), times.last().copied().unwrap_or(0.0))).collect();
            by_last.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            for (old, _) in by_last.into_iter().take(5_000) {
                state.failures.remove(&old);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[test]
    fn blocks_after_limit_then_forgives() {
        let now = Arc::new(AtomicU64::new(1000));
        let n = now.clone();
        let guard = Guard::new(3, 600.0, 900.0, Arc::new(move || n.load(Ordering::SeqCst) as f64));
        let addr = Some("203.0.113.9");
        guard.fail(addr, "bridge key");
        guard.fail(addr, "bridge key");
        assert!(!guard.blocked(addr));
        guard.fail(addr, "bridge key");
        assert!(guard.blocked(addr));
        now.store(1000 + 901, Ordering::SeqCst);
        assert!(!guard.blocked(addr));
        // this machine is never counted
        for _ in 0..5 {
            guard.fail(Some("127.0.0.1"), "bridge key");
            guard.fail(Some("::1"), "bridge key");
        }
        assert!(!guard.blocked(Some("127.0.0.1")));
        // old failures fall out of the window
        guard.fail(addr, "x");
        guard.fail(addr, "x");
        now.store(1000 + 901 + 601, Ordering::SeqCst);
        guard.fail(addr, "x");
        assert!(!guard.blocked(addr));
    }
}

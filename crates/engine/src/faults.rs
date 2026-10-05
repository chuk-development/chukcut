//! Fault injection: named points where a test can make the engine panic.
//!
//! Containment is only worth something if it is tested, and a real bug is not
//! available on demand. So the places a panic must be contained at — the
//! player's render step, an export, a queued export, a bake, a project
//! preparation — call [`hit`] with their name, and a test [`arm`]s that name
//! to make the next call panic as a bug there would.
//!
//! Disarmed, a hit is one relaxed atomic load; nothing is ever armed outside
//! a test. Each arm is consumed by the panic it causes, so a contained panic
//! is followed by normal operation, which is what the tests then check.

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

static ANY: AtomicBool = AtomicBool::new(false);
static ARMED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Make the next [`hit`] of `point` panic. Arming twice panics twice.
pub fn arm(point: &str) {
    ARMED.lock().push(point.to_string());
    ANY.store(true, Ordering::Release);
}

/// Disarm `point`; what is still armed of it is forgotten.
pub fn disarm(point: &str) {
    let mut armed = ARMED.lock();
    armed.retain(|p| p != point);
    ANY.store(!armed.is_empty(), Ordering::Release);
}

/// Whether `point` is armed and not yet hit.
pub fn is_armed(point: &str) -> bool {
    ANY.load(Ordering::Acquire) && ARMED.lock().iter().any(|p| p == point)
}

/// Panic here if a test armed `point`.
pub fn hit(point: &str) {
    hit_keyed(point, "");
}

/// Panic here if a test armed `point`, or `point:key`. The key scopes a
/// fault to one job (its id, its file) so tests running in parallel in one
/// process cannot set off each other's.
pub fn hit_keyed(point: &str, key: &str) {
    if !ANY.load(Ordering::Acquire) {
        return;
    }
    let fire = {
        let mut armed = ARMED.lock();
        let found = armed.iter().position(|p| {
            p == point
                || (!key.is_empty()
                    && p.len() == point.len() + 1 + key.len()
                    && p.starts_with(point)
                    && p[point.len()..].starts_with(':')
                    && p.ends_with(key))
        });
        if let Some(index) = found {
            armed.remove(index);
        }
        ANY.store(!armed.is_empty(), Ordering::Release);
        found.is_some()
    };
    if fire {
        panic!("injected fault at {point}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_armed_point_panics_once_and_others_do_not() {
        arm("faults.test.once");
        hit("faults.test.other");
        let first = std::panic::catch_unwind(|| hit("faults.test.once"));
        assert!(first.is_err());
        // Consumed: the next hit is a normal call.
        hit("faults.test.once");
        assert!(!is_armed("faults.test.once"));
    }

    #[test]
    fn a_keyed_point_fires_only_for_its_key() {
        arm("faults.test.keyed:job-1");
        hit_keyed("faults.test.keyed", "job-2");
        assert!(is_armed("faults.test.keyed:job-1"));
        assert!(std::panic::catch_unwind(|| hit_keyed("faults.test.keyed", "job-1")).is_err());
    }
}

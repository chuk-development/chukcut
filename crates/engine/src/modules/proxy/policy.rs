//! The user's proxy policy, from Settings → Proxies and cache.
//!
//! [`super::decision::decide`] answers "would this file play badly here?". The
//! policy answers "does the user want proxies at all?", and the two are kept
//! apart so the measured rule stays a pure function of the file. This module
//! is the one place they meet.
//!
//! - **Off**: no file is queued and the preview never decodes a proxy, even
//!   one already in the cache. Proxies on disk are left alone; "Clear" in the
//!   same settings section removes them.
//! - **Automatic**: exactly what [`decide`] says.
//! - **Always**: every video that a proxy would make meaningfully smaller. The
//!   shrink clause survives the override on purpose: a "proxy" of a 720p file
//!   is a 720p file, a slower copy of the original, and building it would cost
//!   a transcode for a picture that decodes no faster.
//!
//! The policy lives on the [`super::ProxyQueue`], not in a global, so a test
//! queue is unaffected by what the shared one is set to.

pub use crate::modules::workspace::settings::ProxyPolicy;

use super::decision::{decide, Decision, SourceProfile};

/// [`decide`], overruled by the user's policy.
pub fn decide_with_policy(
    policy: ProxyPolicy,
    profile: &SourceProfile,
    measured_ms: Option<f64>,
) -> Decision {
    let mut decision = decide(profile, measured_ms);
    match policy {
        ProxyPolicy::Auto => {}
        ProxyPolicy::Off => {
            decision.build = false;
            decision.reason = OFF_REASON.into();
        }
        ProxyPolicy::Always => {
            if !decision.build && profile.worth_shrinking() {
                decision.build = true;
                decision.reason = format!(
                    "proxy media is set to Always, so {}×{} gets a {}×{} proxy",
                    profile.width, profile.height, decision.target_width, decision.target_height
                );
            }
        }
    }
    decision
}

/// Whether the preview may decode a proxy in place of the original.
pub fn preview_uses_proxies(policy: ProxyPolicy) -> bool {
    policy != ProxyPolicy::Off
}

pub(super) const OFF_REASON: &str = "proxy media is turned off in Settings";

pub(super) fn to_bits(policy: ProxyPolicy) -> u8 {
    match policy {
        ProxyPolicy::Off => 0,
        ProxyPolicy::Auto => 1,
        ProxyPolicy::Always => 2,
    }
}

pub(super) fn from_bits(bits: u8) -> ProxyPolicy {
    match bits {
        0 => ProxyPolicy::Off,
        2 => ProxyPolicy::Always,
        _ => ProxyPolicy::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hevc_4k() -> SourceProfile {
        SourceProfile::new(3840, 2160, 30.0, "hevc")
    }
    fn h264_1080() -> SourceProfile {
        SourceProfile::new(1920, 1080, 30.0, "h264")
    }
    fn h264_720() -> SourceProfile {
        SourceProfile::new(1280, 720, 30.0, "h264")
    }

    /// The table the Settings row promises: Off never, Automatic only the
    /// heavy file, Always everything a proxy would shrink.
    #[test]
    fn the_policy_decides_what_gets_enqueued() {
        let cases = [
            (ProxyPolicy::Off, hevc_4k(), false),
            (ProxyPolicy::Off, h264_1080(), false),
            (ProxyPolicy::Off, h264_720(), false),
            (ProxyPolicy::Auto, hevc_4k(), true),
            (ProxyPolicy::Auto, h264_1080(), false),
            (ProxyPolicy::Auto, h264_720(), false),
            (ProxyPolicy::Always, hevc_4k(), true),
            (ProxyPolicy::Always, h264_1080(), true),
            // Already proxy-sized: a proxy would be a slower copy.
            (ProxyPolicy::Always, h264_720(), false),
        ];
        for (policy, profile, expected) in cases {
            let decision = decide_with_policy(policy, &profile, None);
            assert_eq!(
                decision.build, expected,
                "{policy:?} {}x{} {}: {}",
                profile.width, profile.height, profile.codec, decision.reason
            );
            assert!(!decision.reason.is_empty());
        }
    }

    /// Off wins over a measurement that says the file is unplayable.
    #[test]
    fn off_overrules_a_measured_decode_cost() {
        let decision = decide_with_policy(ProxyPolicy::Off, &hevc_4k(), Some(500.0));
        assert!(!decision.build);
        assert_eq!(decision.reason, OFF_REASON);
    }

    #[test]
    fn only_off_keeps_the_preview_on_originals() {
        assert!(!preview_uses_proxies(ProxyPolicy::Off));
        assert!(preview_uses_proxies(ProxyPolicy::Auto));
        assert!(preview_uses_proxies(ProxyPolicy::Always));
    }

    #[test]
    fn the_policy_survives_its_atomic_encoding() {
        for policy in [ProxyPolicy::Off, ProxyPolicy::Auto, ProxyPolicy::Always] {
            assert_eq!(from_bits(to_bits(policy)), policy);
        }
    }
}

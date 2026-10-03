//! Loudness: EBU R128 measurement, per clip and for the mix, and the
//! loudness target an export can be brought to.
//!
//! Platforms normalise what they play — YouTube, TikTok and Instagram turn
//! loud uploads down and leave quiet ones quiet — so a mix delivered at their
//! reference level is the loudest it will ever sound there without being
//! turned down. That reference is −14 LUFS for social video, −16 for podcasts,
//! −23 for EBU broadcast. The export option measures the finished mix offline
//! (no real-time scan, the Resolve complaint in
//! `docs/research/resolve-plugins.md` §6.6), applies the gain, and limits true
//! peaks to −1 dBTP so the platform's own encoder does not clip.
//!
//! Per-clip "Normalize" is a gain stored in the clip's voice cleanup block;
//! see `modules::voice`.

pub mod commands;
pub mod measure;
pub mod normalize;

pub use measure::{measure, measure_file, Loudness, Meter};
pub use normalize::{limit_true_peak, normalize_in_place, NormalizeReport};

/// Where true peaks are held after normalising, in dBTP. −1 is what EBU R128
/// and every platform's delivery spec ask for.
pub const TRUE_PEAK_CEILING: f64 = -1.0;

/// The targets the export dialog and the inspector offer, as (LUFS, label).
pub const TARGETS: &[(f32, &str)] = &[
    (-14.0, "−14 LUFS · social"),
    (-16.0, "−16 LUFS · podcast"),
    (-23.0, "−23 LUFS · broadcast"),
];

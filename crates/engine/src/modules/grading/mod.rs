//! Colour tools that read the picture: auto adjust, colour match, and grade
//! presets.
//!
//! The grade itself — its controls, their order, the shader — lives in
//! `project::grade`, `inspector` and `render::grade` (decision 0007 and its
//! addendum). This module only *chooses values* for those controls:
//!
//! - [`auto`] — exposure, white balance and contrast from the clip's own
//!   statistics.
//! - [`matching`] — one clip's colours matched to another's, in L*a*b*.
//! - [`presets`] — a clip's whole grade saved under a name and applied to
//!   others.
//! - [`pixels`] — samples, the grade simulated on the CPU, statistics.
//! - [`sources`] — reading a clip's frames for the above.
//! - [`commands`] — the shell-facing API, `grading_<verb>`.
//!
//! The results are ordinary grades: editable, undoable, drawn by the same
//! shader as anything the user set by hand. `docs/research/ml-features.md`
//! §3.14 explains why no model is involved.

pub mod auto;
pub mod commands;
pub mod matching;
pub mod pixels;
pub mod presets;
pub mod sources;

//! Transitions: the effect between two adjacent clips.
//!
//! A transition is one [`TransitionMaterial`](crate::modules::project::TransitionMaterial)
//! in the material pool, referenced from the `extras` list of the **incoming**
//! segment. The reasoning behind that shape is on the type itself in
//! `project/document.rs`, because that is where the format is defined and where
//! anyone reading the on-disk JSON will look for it. In one line: a transition
//! that lives on a segment survives every structural edit for free, and a
//! transition that lives in a side table has to be repaired by every one of
//! them.
//!
//! ## The geometry of it, in time
//!
//! ```text
//!            outgoing clip                 incoming clip
//!   ┌──────────────────────────┬────────────────────────────┐
//!   │                          │                            │
//!   └──────────────────────────┼────────────────────────────┘
//!                              cut
//!                    ├──── window ────┤
//!                  cut-d/2          cut+d/2
//!            progress 0 ────────────► 1
//! ```
//!
//! Neither clip moves. For the first half the outgoing clip is where it always
//! was and the incoming one is read *before* its in point; for the second half
//! the incoming clip is where it always was and the outgoing one is read *past*
//! its out point. Both halves therefore need media outside the trimmed range —
//! "handles" — and where there is none the borrowed frame freezes on the
//! boundary rather than the transition being refused.
//!
//! Exactly one of the two segments contains any given instant of the window,
//! because the cut is the boundary between them. That is what makes the
//! compositor's hook a one-liner: the segment it was already about to draw
//! either is part of a live transition or is not.
//!
//! ## The pieces
//!
//! - [`resolve`] — pure arithmetic. Window, progress, and the two source
//!   instants. No GPU, no document mutation, and the place every boundary test
//!   points at.
//! - [`edit`] — the bodies of the three `EditCommand` variants, and the
//!   builders that construct them. Every mutation still goes through
//!   `EditCommand`; this module supplies what those variants do, so
//!   `timeline/ops.rs` stays a table of contents.
//! - [`validate`] — the checks `Project::validate()` delegates here, chiefly
//!   the orphan: a transition whose neighbours no longer touch.
//! - [`render`] — the wgpu pipelines and the WGSL. One fragment entry point per
//!   kind, two texture bindings and a progress uniform.
//! - [`catalog`] — what the frontend needs to offer the set to a user.
//!
//! ## What it deliberately does not own
//!
//! Compositing the two layers that get blended. `render::TransitionPipeline`
//! takes two textures and hands back one; producing those textures is the
//! compositor's job, because it is the thing that knows how a segment becomes a
//! quad.

pub mod catalog;
pub mod commands;
pub mod edit;
pub mod render;
pub mod resolve;
pub mod validate;

pub use catalog::{catalog, TransitionDescriptor};
pub use render::{TransitionParams, TransitionPipeline};
pub use resolve::{
    extended_source_time, instant_for, instants_at, linear_progress_at, max_duration, span_at,
    spans, window_for, TransitionInstant, TransitionLayer, TransitionSpan,
};

//! Motion tracking: follow an object through a video clip, and make text,
//! stickers and images follow it.
//!
//! The design is `docs/research/ml-features.md` §2, step T1:
//!
//! - **Analysis** ([`tracker`], [`klt`], [`job`]): pyramidal Lucas–Kanade on
//!   corners inside the box, a RANSAC similarity fit for position, scale and
//!   rotation, and a template search that re-finds the object when the flow
//!   loses it. Pure Rust, CPU only, no model to download. Runs as a background
//!   job over the clip's source range at a reduced frame size.
//! - **Document** ([`model`]): the result is a [`TrackingMaterial`] in the
//!   pool — samples in *source* time and source-frame fractions — and an
//!   overlay follows it through a [`FollowMaterial`] its `extras` name.
//! - **Rendering** ([`follow`]): the compositor maps the overlay's timeline
//!   time through the tracked clip to source time, reads the pose, and puts it
//!   on the canvas through the clip's crop and transform. Trimming, slipping,
//!   retiming, moving and scaling the video keep the overlay on the object,
//!   and the preview and the export share the one function.
//! - **Edits** ([`edit`]): undoable [`TrackingCommand`]s on the one undo
//!   stack — create, re-track, remove, attach, detach, bake to keyframes.
//! - **Commands** ([`commands`]): what the UI, a CLI and MCP call.

pub mod colour;
pub mod commands;
pub mod edit;
pub mod follow;
pub mod job;
pub mod klt;
pub mod model;
pub mod tracker;

pub use edit::TrackingCommand;
pub use model::{
    FollowMaterial, FollowMode, Pose, TrackSample, TrackSettings, TrackingMaterial, FLAG_ANCHOR,
    FLAG_LOST, LOW_CONFIDENCE,
};

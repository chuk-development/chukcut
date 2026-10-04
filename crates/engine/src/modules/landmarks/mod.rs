//! Face landmarks: MediaPipe's 478-point face mesh, per frame of a clip,
//! for the features that need to know where a face is and how it is shaped.
//!
//! - **The track** ([`track`]): every analysed source frame's faces, in a
//!   cache file per media file and model version — derived data, like the
//!   mattes (decision 0019), never the document.
//! - **Analysis** ([`analyse`]): a background job (`analysis::jobs`, kind
//!   `Landmarks`) walking the clip's frames through the ML worker
//!   (`ml::landmarks`), which follows each face from frame to frame and
//!   searches with YuNet only when it loses one.
//! - **Shape** ([`shape`]): the named points the retouch effect reads, and a
//!   face's pose (anchor point, size, tilt) for a follow link.
//! - **Commands** ([`commands`]): analyse, read, follow a face, and keep
//!   the landmarks the retouch effect (`fx::retouch`) needs — queued in the
//!   background after an edit, made before an export.
//!
//! Following a face is not a new kind of link: the face's pose is written
//! as an ordinary motion track (`tracking::model`), so every follow feature
//! (modes, smoothing, detach, bake to keyframes) applies unchanged.

pub mod analyse;
pub mod commands;
pub mod shape;
pub mod track;

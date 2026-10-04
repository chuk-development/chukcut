//! Body landmarks: COCO's 17 keypoints per person (RTMPose, with a YOLOX
//! person detector), per frame of a clip, for text and stickers that follow
//! a body part.
//!
//! - **The track** ([`track`]): every analysed source frame's people, each
//!   with an id kept from frame to frame, in a cache file per media file and
//!   model version — derived data, like the face track (decision 0019).
//! - **Analysis** ([`analyse`]): a background job (`analysis::jobs`, kind
//!   `Body`) walking the clip's frames through the ML worker (`ml::body`),
//!   which follows each person from frame to frame and runs the detector
//!   when it loses everyone and once a second for newcomers.
//! - **Shape** ([`shape`]): a body part's position, size and angle.
//! - **Commands** ([`commands`]): analyse, read, follow a body part.
//!
//! Following a body part is not a new kind of link: the part's pose is
//! written as an ordinary motion track (`tracking::model`), as following a
//! face is (`landmarks`), so modes, smoothing, detach and bake to keyframes
//! apply unchanged. Auto reframe uses the person detector alone
//! (`ml::body::PeopleDetector`) on frames without a face.
//!
//! Decision 0032.

pub mod analyse;
pub mod commands;
pub mod shape;
pub mod track;

//! Body landmarks (RTMPose, 17 COCO keypoints per person) and people (the
//! YOLOX person detector it reads crops from) through the ML worker, for
//! text and stickers that follow a body part (`modules::body`) and for auto
//! reframe when no face is visible.
//!
//! The worker finds people with the detector on the first frame and then
//! follows them the way it follows faces: each frame's keypoints give the
//! region the next frame is read in (`BodyPose::region`), and the detector
//! runs again when every person is lost or when the caller asks it to look
//! for newcomers (`search`).

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody};

pub use chukcut_ml_worker::protocol::{BodyPose, Person};

use super::{worker, MlError};

/// The registry id of the body pose model.
pub const MODEL: &str = "rtmpose-m";
/// The registry id of the person detector (the pose model's companion).
pub const DETECTOR: &str = "yolox-tiny-human";
/// People per frame a clip is analysed for: each costs a pose run on every
/// frame.
pub const MAX_PEOPLE: u32 = 3;
/// A detection at least this sure is a person. YOLOX's scores for a clearly
/// visible person are 0.8 and up; 0.5 is `rtmlib`'s cut-off.
pub const PERSON_THRESHOLD: f32 = 0.5;
/// The frame height people are detected at for auto reframe: the detector
/// letterboxes into 416², so more gains nothing.
pub const DETECTION_HEIGHT: u32 = 416;

/// The version of [`MODEL`] this build runs, which the cache key records.
pub fn model_version() -> &'static str {
    super::matte::version_of(MODEL).expect("the registry lists the body pose model")
}

/// Make the pose model (and the detector it brings) ready; returns the
/// provider it runs on.
pub fn prepare(
    progress: &dyn Fn(&str, Option<f32>),
    cancel: &AtomicBool,
) -> Result<String, MlError> {
    super::prepare(MODEL, "the body landmark model", progress, cancel)
}

/// The people in one RGBA8 frame, with their keypoints in its pixels.
/// `hints` are the last frame's `region`s; `search` also runs the detector
/// for people the hints do not cover.
pub fn landmarks(
    rgba: &[u8],
    width: usize,
    height: usize,
    hints: &[[f32; 4]],
    search: bool,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<BodyPose>, MlError> {
    match worker::request(
        RequestBody::BodyLandmarks {
            model: MODEL.into(),
            width: width as u32,
            height: height as u32,
            hints: hints.to_vec(),
            max_people: MAX_PEOPLE,
            search,
        },
        rgba,
        &|_, _| {},
        cancel,
        Duration::from_secs(60),
    )? {
        Outcome::BodyLandmarks { people, .. } => Ok(people),
        other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
    }
}

/// A ready person detector, for auto reframe. Cheap to keep; the session
/// lives in the worker.
#[derive(Debug, Clone)]
pub struct PeopleDetector {
    /// Where the model runs, e.g. `CUDA` or `CPU`.
    pub provider: String,
}

impl PeopleDetector {
    /// Make person detection ready, downloading what is missing.
    pub fn prepare(
        progress: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<PeopleDetector, MlError> {
        let provider = super::prepare(DETECTOR, "the person detector", progress, cancel)?;
        Ok(PeopleDetector { provider })
    }

    /// The people in one RGBA8 frame, in its pixels, largest first.
    pub fn detect(&self, rgba: &[u8], width: usize, height: usize) -> Result<Vec<Person>, MlError> {
        match worker::request(
            RequestBody::DetectPeople {
                model: DETECTOR.into(),
                width: width as u32,
                height: height as u32,
                score_threshold: PERSON_THRESHOLD,
            },
            rgba,
            &|_, _| {},
            None,
            Duration::from_secs(30),
        )? {
            Outcome::People { people, .. } => Ok(people),
            other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
        }
    }
}

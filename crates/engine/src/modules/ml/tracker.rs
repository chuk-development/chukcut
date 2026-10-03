//! VitTrack single-object tracking through the ML worker (tracker "T2" in
//! `docs/research/ml-features.md` §2.3).
//!
//! A learned tracker: robust to fast motion and blur, where the KLT tracker
//! (T1, `modules/tracking/tracker.rs`) loses the object. It gives a box and a
//! confidence per frame, no rotation.
//!
//! The track's template lives in the worker. If the worker dies mid-track,
//! the next update starts a new track on the current frame from the last
//! box: one frame of lower accuracy instead of a failed job.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody};

use super::{worker, MlError};

/// The registry id of the tracking model.
pub const MODEL: &str = "vittrack";

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// One running track in the worker.
#[derive(Debug)]
pub struct VitTracker {
    session: u64,
    /// The last box, in pixels of the frames passed in: x, y, w, h.
    last: [f32; 4],
}

impl VitTracker {
    /// Make VitTrack ready (see [`super::prepare`]); returns the provider.
    pub fn prepare(
        progress: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<String, MlError> {
        super::prepare(MODEL, "the VitTrack tracker", progress, cancel)
    }

    /// Start tracking the object in `bbox` (pixels: x, y, w, h) of this
    /// RGBA frame.
    pub fn start(
        rgba: &[u8],
        width: usize,
        height: usize,
        bbox: [f32; 4],
    ) -> Result<Self, MlError> {
        let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        start_session(session, rgba, width, height, bbox)?;
        Ok(VitTracker {
            session,
            last: bbox,
        })
    }

    /// The object's box in the next frame and the tracker's confidence;
    /// `None` when it is lost in this frame (the box then stays put). With
    /// `redetect`, a frame where the search around the last box finds
    /// nothing is searched whole, which re-finds an object that came out from
    /// behind something further on or came back into the frame elsewhere.
    pub fn update(
        &mut self,
        rgba: &[u8],
        width: usize,
        height: usize,
        redetect: bool,
    ) -> Result<(Option<[f32; 4]>, f32), MlError> {
        let body = RequestBody::TrackUpdate {
            session: self.session,
            width: width as u32,
            height: height as u32,
            redetect,
        };
        let outcome = match worker::request(body, rgba, &|_, _| {}, None, Duration::from_secs(30)) {
            // A new worker does not know this track: start it again here.
            Err(MlError::Failed(why)) if why.starts_with("no track") => {
                tracing::warn!("VitTrack session lost with the worker; restarting it");
                start_session(self.session, rgba, width, height, self.last)?;
                return Ok((Some(self.last), 0.0));
            }
            other => other?,
        };
        match outcome {
            Outcome::Track { bbox, score, .. } => {
                if let Some(b) = bbox {
                    self.last = b;
                }
                Ok((bbox, score))
            }
            other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
        }
    }
}

fn start_session(
    session: u64,
    rgba: &[u8],
    width: usize,
    height: usize,
    bbox: [f32; 4],
) -> Result<(), MlError> {
    let body = RequestBody::TrackStart {
        model: MODEL.into(),
        session,
        width: width as u32,
        height: height as u32,
        bbox,
    };
    worker::request(body, rgba, &|_, _| {}, None, Duration::from_secs(60)).map(|_| ())
}

impl Drop for VitTracker {
    fn drop(&mut self) {
        // Only tell a worker that is running; never start one to forget.
        if let Some(client) = worker::running() {
            let _ = client.call_with(
                RequestBody::TrackEnd {
                    session: self.session,
                },
                &[],
                &|_, _| {},
                None,
                Duration::from_secs(5),
            );
        }
    }
}

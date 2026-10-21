//! What `project_inspect`, `sequence_inspect` and `doc_inspect` answer.
//!
//! FilmCraft splits its document view in two: the project tree (bins,
//! items) and the active sequence (tracks, clips, markers, playhead). The
//! split is taken over: the project is the media pool, the canvas and the
//! list of timelines; the sequence is the open timeline's lanes and clips,
//! with the refs and ids every other command takes. `info` is both in one,
//! as it always was.

use chukcut_engine::modules::sequence::{self as seq, SequenceKind};
use serde_json::{json, Value};

use crate::ops::summary;
use crate::session::Session;
use crate::values::seconds;

/// The project: name, canvas, frame rate, media, timelines, issues, undo.
pub fn project(session: &Session) -> Value {
    let mut out = summary::project(&session.project(), &session.state, &session.path);
    if let Some(map) = out.as_object_mut() {
        map.remove("tracks");
        map.remove("markers");
        map.insert("timelines".into(), timelines(session));
    }
    out
}

/// The open timeline: its lanes and clips, markers and length.
pub fn sequence(session: &Session) -> Value {
    let full = summary::project(&session.project(), &session.state, &session.path);
    let pick = |key: &str| full.get(key).cloned().unwrap_or(Value::Null);
    json!({
        "timeline": pick("timeline"),
        "fps": pick("fps"),
        "canvas": pick("canvas"),
        "duration": pick("duration"),
        "duration_us": pick("duration_us"),
        "tracks": pick("tracks"),
        "markers": pick("markers"),
        "history": pick("history"),
    })
}

/// The project and the open timeline in one answer.
pub fn document(session: &Session) -> Value {
    json!({"project": project(session), "sequence": sequence(session)})
}

fn timelines(session: &Session) -> Value {
    let list = session.with(seq::list);
    Value::Array(
        list.iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "name": s.name,
                    "kind": if s.kind == SequenceKind::Compound { "compound" } else { "timeline" },
                    "duration": seconds(s.duration),
                    "tracks": s.tracks,
                    "clips": s.clips,
                    "open": s.active,
                })
            })
            .collect(),
    )
}

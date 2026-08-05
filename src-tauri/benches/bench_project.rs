//! What the document itself costs: saving, loading, editing, undoing.
//!
//! No GPU, no FFmpeg, no files bigger than a few megabytes — and therefore the
//! only group in this suite whose numbers are stable enough to compare between
//! machines. It is also the group most likely to matter to a user without
//! anybody noticing, because a save that takes a second is felt on every
//! keystroke that triggers an autosave, and nothing in this repository had ever
//! measured one.
//!
//! Five hundred segments is the size the brief asks for and is a realistic
//! upper end for a hand-cut edit: a three-minute video cut every few seconds
//! across four lanes.
//!
//! ## What the undo row found
//!
//! `timeline::history::MAX_DEPTH` is 500. Applying a thousand edits and then
//! undoing "them all" is not possible through `History` — the first five
//! hundred commands have been dropped off the bottom of the stack by then. The
//! benchmark measures a thousand applies and then undoes everything the history
//! still holds, and reports how many that was. This is a real property of the
//! editor, not a limitation of the benchmark: an hour of editing followed by an
//! attempt to undo back to the start stops halfway, silently.

use chukcut_lib::modules::project::{
    new_id, CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
};
use chukcut_lib::modules::timeline::ops::EditCommand;
use chukcut_lib::modules::timeline::History;

use crate::harness::{rounds, time_ms, Measurement};

pub const GROUP: &str = "project";

/// The document under test. Four video lanes, `segments` clips in total, each
/// carrying a non-default transform so the serialized form is the size a real
/// project's is rather than a run of defaults `serde` skips.
fn document(segments: usize) -> Project {
    const LANES: usize = 4;
    const CLIP: Micros = 2_000_000;
    let mut project = Project::new("project bench", CanvasConfig::default(), 30.0);

    let material_id = new_id();
    project
        .materials
        .videos
        .push(chukcut_lib::modules::project::VideoMaterial {
            id: material_id.clone(),
            path: "/nonexistent/bench.mp4".into(),
            width: 1920,
            height: 1080,
            duration: CLIP * segments as Micros,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });

    let mut tracks: Vec<Track> = (0..LANES)
        .map(|n| Track::new(TrackKind::Video, format!("Video {}", n + 1)))
        .collect();

    for index in 0..segments {
        let lane = index % LANES;
        let slot = (index / LANES) as Micros;
        let spread = index as f32 / segments.max(1) as f32;
        tracks[lane].segments.push(Segment {
            id: new_id(),
            material_id: material_id.clone(),
            target_range: TimeRange::new(slot * CLIP, CLIP - 1),
            source_range: TimeRange::new(slot * CLIP, CLIP - 1),
            render_index: lane as i32,
            speed: 1.0,
            volume: 1.0,
            transform: Transform {
                position: [spread - 0.5, 0.5 - spread],
                scale: [0.8, 0.8],
                rotation: spread * 10.0,
                opacity: 0.9,
                flip_h: false,
                flip_v: false,
            },
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
    }
    project.tracks = tracks;
    project
}

pub struct Budget {
    pub segments: usize,
    pub edits: usize,
    pub rounds: usize,
}

pub fn run(budget: &Budget) -> Vec<Measurement> {
    let mut out = Vec::new();
    let project = document(budget.segments);
    let path = std::env::temp_dir().join("chukcut-bench-project.json");

    // Save. `project::commands::write_project` is `to_string_pretty` plus a
    // write, and it is measured as both because on a slow disk the write is the
    // whole cost and on a fast one the serializer is.
    let mut bytes = 0usize;
    let samples = rounds::<String>(budget.rounds, |_| {
        let (json, ms) = time_ms(|| {
            serde_json::to_string_pretty(&project).expect("a project always serializes")
        });
        bytes = json.len();
        std::fs::write(&path, &json).map_err(|e| e.to_string())?;
        Ok(ms)
    });
    match samples {
        Ok(samples) => out.push(
            Measurement::ms(GROUP, format!("save {} segments", budget.segments), samples)
                .with_note(format!("{} KB of JSON, serializer only", bytes / 1024)),
        ),
        Err(error) => out.push(Measurement::skip(
            GROUP,
            format!("save {} segments", budget.segments),
            error,
        )),
    }

    let samples = rounds::<String>(budget.rounds, |_| {
        let json = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let (parsed, ms) = time_ms(|| serde_json::from_str::<Project>(&json));
        let parsed = parsed.map_err(|e| e.to_string())?;
        if parsed
            .tracks
            .iter()
            .map(|t| t.segments.len())
            .sum::<usize>()
            != budget.segments
        {
            return Err("the round trip lost segments".into());
        }
        Ok(ms)
    });
    match samples {
        Ok(samples) => out.push(
            Measurement::ms(GROUP, format!("load {} segments", budget.segments), samples)
                .with_note("deserializer only, file already in page cache"),
        ),
        Err(error) => out.push(Measurement::skip(
            GROUP,
            format!("load {} segments", budget.segments),
            error,
        )),
    }
    let _ = std::fs::remove_file(&path);

    // A full round trip through the disk, which is what an autosave costs.
    let samples = rounds::<String>(budget.rounds, |_| {
        let (result, ms) = time_ms(|| -> Result<(), String> {
            let json = serde_json::to_string_pretty(&project).map_err(|e| e.to_string())?;
            std::fs::write(&path, &json).map_err(|e| e.to_string())?;
            let read = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            serde_json::from_str::<Project>(&read)
                .map(drop)
                .map_err(|e| e.to_string())
        });
        result?;
        Ok(ms)
    });
    match samples {
        Ok(samples) => out.push(
            Measurement::ms(
                GROUP,
                format!("save+load round trip, {} segments", budget.segments),
                samples,
            )
            .with_note("what an autosave costs"),
        ),
        Err(error) => out.push(Measurement::skip(GROUP, "save+load round trip", error)),
    }
    let _ = std::fs::remove_file(&path);

    // Edits. `SetTransform` is chosen because it is the command the UI issues
    // most often by a wide margin — every drag of a clip in the viewer is one —
    // and because it cannot fail on a valid document, so a slow round and a
    // rejected round cannot be confused.
    let mut undone = 0usize;
    let mut depth = 0usize;
    let samples = rounds::<String>(budget.rounds, |_| {
        let mut working = project.clone();
        let ids: Vec<String> = working
            .tracks
            .iter()
            .flat_map(|t| t.segments.iter().map(|s| s.id.clone()))
            .collect();
        let mut history = History::new();

        let (result, ms) = time_ms(|| -> Result<(), String> {
            for n in 0..budget.edits {
                let id = &ids[n % ids.len()];
                let before = working
                    .segment(id)
                    .map(|(_, s)| s.transform)
                    .ok_or("segment vanished mid-run")?;
                let mut after = before;
                after.rotation = (n % 360) as f32;
                after.opacity = 0.5 + (n % 50) as f32 / 100.0;
                history.apply(
                    &mut working,
                    EditCommand::SetTransform {
                        segment_id: id.clone(),
                        before,
                        after,
                    },
                )?;
            }
            Ok(())
        });
        result?;
        depth = depth.max(count_undoable(&mut history, &mut working.clone()));
        Ok(ms / budget.edits as f64 * 1000.0)
    });
    match samples {
        Ok(samples) => out.push(
            Measurement::new(
                GROUP,
                format!("apply {} edits", budget.edits),
                "µs/edit",
                crate::harness::Direction::Lower,
                samples,
            )
            .with_note(format!(
                "on a {}-segment document; history retained {depth} of {}",
                budget.segments, budget.edits
            )),
        ),
        Err(error) => out.push(Measurement::skip(
            GROUP,
            format!("apply {} edits", budget.edits),
            error,
        )),
    }

    // Undo everything the history holds. Measured on its own because undo does
    // strictly more work than apply: it inverts the command first, and for the
    // segment commands that means cloning a `Segment`.
    let samples = rounds::<String>(budget.rounds, |_| {
        let mut working = project.clone();
        let ids: Vec<String> = working
            .tracks
            .iter()
            .flat_map(|t| t.segments.iter().map(|s| s.id.clone()))
            .collect();
        let mut history = History::new();
        for n in 0..budget.edits {
            let id = &ids[n % ids.len()];
            let before = working
                .segment(id)
                .map(|(_, s)| s.transform)
                .ok_or("segment vanished mid-run")?;
            let mut after = before;
            after.rotation = (n % 360) as f32;
            history
                .apply(
                    &mut working,
                    EditCommand::SetTransform {
                        segment_id: id.clone(),
                        before,
                        after,
                    },
                )
                .map_err(|e| e.to_string())?;
        }

        let mut count = 0usize;
        let (result, ms) = time_ms(|| -> Result<(), String> {
            while history.can_undo() {
                history.undo(&mut working)?;
                count += 1;
            }
            Ok(())
        });
        result?;
        undone = count;
        Ok(ms / count.max(1) as f64 * 1000.0)
    });
    match samples {
        Ok(samples) => out.push(
            Measurement::new(
                GROUP,
                "undo the whole history",
                "µs/undo",
                crate::harness::Direction::Lower,
                samples,
            )
            .with_note(format!(
                "{undone} undone of {} applied — History::MAX_DEPTH caps the stack at 500, so the \
                 earlier edits cannot be undone at all",
                budget.edits
            )),
        ),
        Err(error) => out.push(Measurement::skip(GROUP, "undo the whole history", error)),
    }

    out
}

/// How many commands the history is still holding, established by undoing them
/// on a throwaway copy of the document.
fn count_undoable(history: &mut History, scratch: &mut Project) -> usize {
    let mut count = 0;
    while history.can_undo() {
        if history.undo(scratch).is_err() {
            break;
        }
        count += 1;
    }
    count
}

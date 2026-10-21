//! OpenTimelineIO JSON (`.otio`).
//!
//! Schemas written: `Timeline.1`, `Stack.1`, `Track.1`, `Clip.2`, `Gap.1`,
//! `Transition.1`, `Marker.2`, `ExternalReference.1`, `LinearTimeWarp.1`,
//! `RationalTime.1`, `TimeRange.1` — the set FilmCraft's OTIO writer uses
//! (storytold/filmcraft, `crates/interchange/src/otio.rs`, Apache-2.0 OR
//! MIT; NOTICE.md), which DaVinci Resolve and the reference adapters read.
//!
//! Times are frames at the timeline rate, `rate` being the exact rate as a
//! float (29.97002997…). A clip's `source_range` lasts as long as the clip
//! does on the timeline; a speed change is a `LinearTimeWarp` on it, as
//! OTIO's own adapters write it. chukcut's transitions are centred on the
//! cut, which is exactly OTIO's `in_offset` = `out_offset` = half.

use serde_json::{json, Value};

use super::timeline::{Clip, ClipKind, Kind, Rate, Timeline};
use super::{file_url, Report};
use crate::modules::project::Micros;

fn rt(t: Micros, rate: Rate) -> Value {
    let exact = rate.frames_exact(t);
    // Whole frames as whole numbers: a reader that compares times exactly
    // (Resolve does) then sees 100.0, not 99.99999999.
    let value = if (exact - exact.round()).abs() < 1e-6 {
        exact.round()
    } else {
        exact
    };
    json!({"OTIO_SCHEMA": "RationalTime.1", "rate": rate.fps, "value": value})
}

fn tr(start: Micros, duration: Micros, rate: Rate) -> Value {
    json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": rt(start, rate), "duration": rt(duration, rate)})
}

fn gap(duration: Micros, rate: Rate) -> Value {
    json!({
        "OTIO_SCHEMA": "Gap.1",
        "name": "",
        "source_range": tr(0, duration, rate),
        "effects": [],
        "markers": [],
        "enabled": true,
        "metadata": {},
    })
}

fn clip(c: &Clip, rate: Rate) -> Value {
    let available = match c.media_duration {
        Some(d) => tr(0, d, rate),
        None => Value::Null,
    };
    let mut effects = Vec::new();
    if c.speed != 1.0 {
        effects.push(json!({
            "OTIO_SCHEMA": "LinearTimeWarp.1",
            "name": "",
            "effect_name": "LinearTimeWarp",
            "time_scalar": c.speed as f64,
            "metadata": {},
        }));
    }
    json!({
        "OTIO_SCHEMA": "Clip.2",
        "name": c.name,
        "source_range": tr(c.source_start, c.record.duration, rate),
        "effects": effects,
        "markers": [],
        "enabled": c.enabled,
        "media_references": {"DEFAULT_MEDIA": {
            "OTIO_SCHEMA": "ExternalReference.1",
            "name": c.name,
            "target_url": file_url(&c.path),
            "available_range": available,
            "available_image_bounds": null,
            "metadata": {"chukcut": {"still": c.kind == ClipKind::Image}},
        }},
        "active_media_reference_key": "DEFAULT_MEDIA",
        "metadata": {"chukcut": {"speed": c.speed}},
    })
}

fn transition(duration: Micros, rate: Rate) -> Value {
    let half = duration / 2;
    json!({
        "OTIO_SCHEMA": "Transition.1",
        "name": "Cross Dissolve",
        "transition_type": "SMPTE_Dissolve",
        "in_offset": rt(half, rate),
        "out_offset": rt(duration - half, rate),
        "metadata": {},
    })
}

/// The timeline as an `.otio` document.
pub fn write(t: &Timeline, report: &mut Report) -> String {
    let rate = t.rate;
    let mut tracks = Vec::new();
    for track in &t.tracks {
        let mut children = Vec::new();
        let mut at: Micros = 0;
        for c in &track.clips {
            if c.record.start > at {
                children.push(gap(c.record.start - at, rate));
            }
            if let Some(d) = c.transition_in {
                // OTIO needs an item on both sides of a transition, and the
                // borrowed half must fit in the neighbours.
                if children.is_empty() {
                    report.warn("a transition at the very start of a track is not written");
                } else {
                    children.push(transition(d, rate));
                }
            }
            children.push(clip(c, rate));
            at = c.record.end();
        }
        tracks.push(json!({
            "OTIO_SCHEMA": "Track.1",
            "name": track.name,
            "kind": match track.kind { Kind::Video => "Video", Kind::Audio => "Audio" },
            "children": children,
            "source_range": null,
            "effects": [],
            "markers": [],
            "enabled": true,
            "metadata": {},
        }));
    }
    let markers: Vec<Value> = t
        .markers
        .iter()
        .map(|m| {
            json!({
                "OTIO_SCHEMA": "Marker.2",
                "name": m.label,
                "comment": "",
                "color": m.color,
                "marked_range": tr(m.time, 0, rate),
                "metadata": {},
            })
        })
        .collect();
    let doc = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": t.name,
        "global_start_time": null,
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "children": tracks,
            "source_range": null,
            "effects": [],
            "markers": markers,
            "enabled": true,
            "metadata": {},
        },
        "metadata": {"chukcut": {"width": t.width, "height": t.height, "fps": t.rate.fps}},
    });
    serde_json::to_string_pretty(&doc).expect("a JSON value always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::interchange::timeline::{collect, tests::project};

    fn doc() -> Value {
        let mut report = Report::default();
        let t = collect(&project(), &mut report);
        serde_json::from_str(&write(&t, &mut report)).unwrap()
    }

    fn frames(v: &Value) -> f64 {
        v["value"].as_f64().unwrap()
    }

    #[test]
    fn the_main_lane_has_both_shots_and_the_dissolve_between_them() {
        let d = doc();
        let v1 = &d["tracks"]["children"][0];
        assert_eq!(v1["kind"], "Video");
        let kids = v1["children"].as_array().unwrap();
        let schemas: Vec<&str> = kids
            .iter()
            .map(|k| k["OTIO_SCHEMA"].as_str().unwrap())
            .collect();
        assert_eq!(schemas, ["Clip.2", "Transition.1", "Clip.2"]);
        // 10 s into the file, 4 s long, at 25 fps.
        assert_eq!(frames(&kids[0]["source_range"]["start_time"]), 250.0);
        assert_eq!(frames(&kids[0]["source_range"]["duration"]), 100.0);
        assert_eq!(frames(&kids[1]["in_offset"]), 12.5);
        assert_eq!(frames(&kids[1]["out_offset"]), 12.5);
        assert_eq!(
            kids[0]["media_references"]["DEFAULT_MEDIA"]["target_url"],
            "file:///m/shot%20a.mp4"
        );
    }

    #[test]
    fn the_overlay_starts_after_a_gap_and_carries_its_speed() {
        let d = doc();
        let v2 = d["tracks"]["children"][1]["children"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(v2[0]["OTIO_SCHEMA"], "Gap.1");
        assert_eq!(frames(&v2[0]["source_range"]["duration"]), 50.0);
        assert_eq!(v2[1]["effects"][0]["OTIO_SCHEMA"], "LinearTimeWarp.1");
        assert_eq!(v2[1]["effects"][0]["time_scalar"], 2.0);
        let a1 = &d["tracks"]["children"][2];
        assert_eq!(a1["kind"], "Audio");
        let marker = &d["tracks"]["markers"][0];
        assert_eq!(marker["name"], "drop");
        assert_eq!(marker["color"], "RED");
        assert_eq!(frames(&marker["marked_range"]["start_time"]), 150.0);
    }
}

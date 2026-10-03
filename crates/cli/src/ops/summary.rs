//! The JSON a script reads back: the project, one clip, one grade.
//!
//! Not the document itself — that is the `.chukcut` file, and `info --full`
//! hands it over whole. This is the view a caller needs to decide its next
//! edit: every clip with a reference it can pass back, where it sits, what it
//! shows, and what is applied to it. Times are seconds as floats, with the
//! exact microseconds alongside where an edit would reuse them.

use chukcut_engine::modules::inspector::edit::{GradeControl, GradeEdit};
use chukcut_engine::modules::project::grade::{WheelKind, HSL_BANDS};
use chukcut_engine::modules::project::{Project, Segment, Track, TrackKind};
use chukcut_engine::state::AppState;
use serde_json::{json, Map, Value};

use crate::values::{hex, seconds};

/// Every single-number grade control, by the name `grade --set` takes.
pub fn grade_controls() -> Vec<(String, GradeControl)> {
    use GradeControl as G;
    let mut out: Vec<(String, GradeControl)> = [
        G::Brightness,
        G::Contrast,
        G::Saturation,
        G::Temperature,
        G::LutIntensity,
        G::Exposure,
        G::Tint,
        G::Highlights,
        G::Shadows,
        G::Whites,
        G::Blacks,
        G::Vibrance,
        G::Sharpen,
        G::Clarity,
        G::VignetteAmount,
        G::VignetteMidpoint,
        G::VignetteFeather,
        G::Grain,
        G::Fade,
    ]
    .into_iter()
    .map(|c| (control_name(c), c))
    .collect();
    for (i, (band, _)) in HSL_BANDS.iter().enumerate() {
        let band = band.to_ascii_lowercase();
        let i = i as u8;
        out.push((format!("hsl_hue:{band}"), G::HslHue(i)));
        out.push((format!("hsl_saturation:{band}"), G::HslSaturation(i)));
        out.push((format!("hsl_luminance:{band}"), G::HslLuminance(i)));
    }
    for kind in WheelKind::ALL {
        let k = serde_json::to_value(kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        out.push((format!("wheel_x:{k}"), G::WheelX(kind)));
        out.push((format!("wheel_y:{k}"), G::WheelY(kind)));
        out.push((format!("wheel_luma:{k}"), G::WheelLuma(kind)));
    }
    out
}

fn control_name(control: GradeControl) -> String {
    serde_json::to_value(control)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The controls of `edit` that are away from rest.
pub fn grade_values(edit: &GradeEdit) -> Map<String, Value> {
    let mut map = Map::new();
    for (name, control) in grade_controls() {
        let value = control.get(edit);
        if (value - control.rest()).abs() > 1e-6 {
            map.insert(name, json!(value));
        }
    }
    map
}

fn kind_of(project: &Project, track: &Track, segment: &Segment) -> &'static str {
    let pool = &project.materials;
    if pool.is_effect_clip(segment) {
        return "effect";
    }
    if let Some(text) = pool.text(&segment.material_id) {
        return if text.caption.is_some() {
            "caption"
        } else {
            "title"
        };
    }
    if pool.video(&segment.material_id).is_some() {
        return "video";
    }
    if pool.image(&segment.material_id).is_some() {
        return "image";
    }
    if pool.audio(&segment.material_id).is_some() {
        return "audio";
    }
    match track.kind {
        TrackKind::Sticker => "sticker",
        _ => "unknown",
    }
}

/// One clip, as `info` lists it.
pub fn clip(project: &Project, track_index: usize, clip_index: usize, segment: &Segment) -> Value {
    let track = &project.tracks[track_index];
    let pool = &project.materials;
    let mut out = Map::new();
    out.insert("ref".into(), json!(format!("{track_index}:{clip_index}")));
    out.insert("id".into(), json!(segment.id));
    out.insert("kind".into(), json!(kind_of(project, track, segment)));
    out.insert("material_id".into(), json!(segment.material_id));
    let source = if let Some(m) = pool.video(&segment.material_id) {
        json!(m.path)
    } else if let Some(m) = pool.image(&segment.material_id) {
        json!(m.path)
    } else if let Some(m) = pool.audio(&segment.material_id) {
        json!(m.path)
    } else if let Some(t) = pool.text(&segment.material_id) {
        json!(t.content)
    } else if let Some(e) = pool.effect(&segment.material_id) {
        json!(e.kind)
    } else {
        Value::Null
    };
    out.insert("source".into(), source);
    let t = segment.target_range;
    out.insert("start".into(), json!(seconds(t.start)));
    out.insert("end".into(), json!(seconds(t.end())));
    out.insert("duration".into(), json!(seconds(t.duration)));
    out.insert("start_us".into(), json!(t.start));
    out.insert("duration_us".into(), json!(t.duration));
    out.insert("in".into(), json!(seconds(segment.source_range.start)));
    out.insert("out".into(), json!(seconds(segment.source_range.end())));
    if (segment.speed - 1.0).abs() > 1e-6 {
        out.insert("speed".into(), json!(segment.speed));
    }
    if (segment.volume - 1.0).abs() > 1e-6 {
        out.insert("volume".into(), json!(segment.volume));
    }
    let tr = segment.transform;
    if tr.position != [0.0, 0.0]
        || tr.scale != [1.0, 1.0]
        || tr.rotation != 0.0
        || tr.opacity != 1.0
        || tr.flip_h
        || tr.flip_v
    {
        out.insert("transform".into(), json!(tr));
    }
    if let Some(crop) = segment.crop {
        out.insert("crop".into(), json!(crop));
    }
    if let Some(text) = pool.text(&segment.material_id) {
        out.insert(
            "text".into(),
            json!({
                "content": text.content,
                "font": text.font_family,
                "size": text.font_size,
                "color": hex(text.color),
                "bold": text.bold,
                "italic": text.italic,
            }),
        );
    }
    let effects: Vec<Value> = pool
        .effects_of(segment)
        .iter()
        .enumerate()
        .map(|(i, e)| {
            json!({
                "index": i,
                "id": e.id,
                "kind": e.kind,
                "enabled": e.enabled,
                "params": e.params,
                "animated": e.keyframes.keys().collect::<Vec<_>>(),
            })
        })
        .collect();
    if !effects.is_empty() {
        out.insert("effects".into(), json!(effects));
    }
    if let Some(adjust) = pool.color_adjust_of(segment) {
        let edit = GradeEdit::of(Some(adjust));
        let mut grade = grade_values(&edit);
        if let Some(lut) = &adjust.lut {
            grade.insert("lut".into(), json!(lut.path));
        }
        out.insert("grade".into(), Value::Object(grade));
    }
    if let Some(transition) = pool.transition_of(segment) {
        out.insert(
            "transition".into(),
            json!({
                "kind": transition.kind,
                "preset": transition.preset,
                "duration": seconds(transition.duration),
            }),
        );
    }
    if let Some(animation) = pool.animation_of(segment) {
        let mut a = serde_json::to_value(animation).unwrap_or(Value::Null);
        if let Some(map) = a.as_object_mut() {
            map.remove("id");
        }
        out.insert("animation".into(), a);
    }
    if !segment.keyframes.is_empty() {
        let keys: Map<String, Value> = segment
            .keyframes
            .iter()
            .map(|track| {
                let name = serde_json::to_value(track.property)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let points: Vec<Value> = track
                    .keyframes
                    .iter()
                    .map(|k| json!({"time": seconds(k.time), "value": k.value, "easing": k.easing}))
                    .collect();
                (name, json!(points))
            })
            .collect();
        out.insert("keyframes".into(), Value::Object(keys));
    }
    if let Some(link) = pool.link_of(segment) {
        out.insert("link_group".into(), json!(link));
    }
    if let Some(follow) = pool.follow_of(segment) {
        out.insert("follows_track".into(), json!(follow.track_id));
    }
    Value::Object(out)
}

/// A clip by id, with its position, or `null`.
pub fn clip_by_id(project: &Project, segment_id: &str) -> Value {
    for (ti, track) in project.tracks.iter().enumerate() {
        let mut segments: Vec<&Segment> = track.segments.iter().collect();
        segments.sort_by_key(|s| s.target_range.start);
        if let Some(ci) = segments.iter().position(|s| s.id == segment_id) {
            return clip(project, ti, ci, segments[ci]);
        }
    }
    Value::Null
}

/// The whole project, as `info` prints it.
pub fn project(project: &Project, state: &AppState, path: &std::path::Path) -> Value {
    let tracks: Vec<Value> = project
        .tracks
        .iter()
        .enumerate()
        .map(|(ti, track)| {
            let mut segments: Vec<&Segment> = track.segments.iter().collect();
            segments.sort_by_key(|s| s.target_range.start);
            let clips: Vec<Value> = segments
                .iter()
                .enumerate()
                .map(|(ci, s)| clip(project, ti, ci, s))
                .collect();
            json!({
                "index": ti,
                "id": track.id,
                "name": track.name,
                "kind": track.kind,
                "muted": track.muted,
                "locked": track.locked,
                "hidden": track.hidden,
                "volume": track.volume,
                "clips": clips,
            })
        })
        .collect();
    let pool = &project.materials;
    let videos: Vec<Value> = pool
        .videos
        .iter()
        .map(|m| {
            json!({
                "id": m.id, "kind": "video", "path": m.path, "width": m.width, "height": m.height,
                "duration": seconds(m.duration), "fps": m.fps, "has_audio": m.has_audio,
            })
        })
        .collect();
    let images: Vec<Value> = pool
        .images
        .iter()
        .map(|m| json!({"id": m.id, "kind": "image", "path": m.path, "width": m.width, "height": m.height}))
        .collect();
    let audios: Vec<Value> = pool
        .audios
        .iter()
        .map(|m| {
            json!({
                "id": m.id, "kind": "audio", "path": m.path, "duration": seconds(m.duration),
                "sample_rate": m.sample_rate, "channels": m.channels,
            })
        })
        .collect();
    let markers: Vec<Value> = project
        .markers
        .iter()
        .map(|m| json!({"id": m.id, "time": seconds(m.time), "label": m.label, "color": m.color}))
        .collect();
    let issues: Vec<Value> = project
        .validate()
        .into_iter()
        .map(|i| json!({"severity": i.severity, "message": i.message, "subject": i.subject_id}))
        .collect();
    let history = state.history.read();
    let duration = project.duration();
    json!({
        "path": path,
        "name": project.name,
        "canvas": {
            "width": project.canvas.width,
            "height": project.canvas.height,
            "background": project.canvas.background,
        },
        "fps": project.fps,
        "duration": seconds(duration),
        "duration_us": duration,
        "tracks": tracks,
        "materials": videos.into_iter().chain(images).chain(audios).collect::<Vec<_>>(),
        "markers": markers,
        "issues": issues,
        "history": {
            "can_undo": history.can_undo(),
            "can_redo": history.can_redo(),
            "undo": history.undo_label(),
            "redo": history.redo_label(),
        },
    })
}

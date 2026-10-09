//! One whole editing session through the command layer, the way the app
//! drives it: new project, imports, cuts, grading, effects, motion, titles,
//! captions, a transition, tracking, silence removal, a loudness target,
//! an export on every usable encoder, save and reopen, and then undo back
//! to the start and redo to the end.
//!
//! Every other integration test checks one module against a hand-built
//! document. This one checks that the modules agree with each other on one
//! document that all of them have touched — which is where a night of
//! parallel merges breaks things. The output is checked with ffprobe and
//! ffmpeg, tools this codebase did not write.
//!
//! Skips (with a reason) when there is no ffmpeg or no GPU.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chukcut_engine::modules::captions::commands as captions;
use chukcut_engine::modules::captions::edit::PlaceOptions;
use chukcut_engine::modules::captions::srt::SubtitleFormat;
use chukcut_engine::modules::captions::style::CaptionStyle;
use chukcut_engine::modules::cloud::commands as cloud;
use chukcut_engine::modules::cloud::provenance::{self, Licence, Origin, OriginKind};
use chukcut_engine::modules::export::commands as export;
use chukcut_engine::modules::export::job::{ExportOverrides, ExportProgress, ExportRequest};
use chukcut_engine::modules::export::presets::{AudioCodec, VideoCodec};
use chukcut_engine::modules::fx::commands as fx;
use chukcut_engine::modules::inspector::commands as inspector;
use chukcut_engine::modules::inspector::edit::GradeControl;
use chukcut_engine::modules::motion::commands as motion;
use chukcut_engine::modules::project::animation::{AnimationSlot, TextSlot};
use chukcut_engine::modules::project::commands as project;
use chukcut_engine::modules::project::document::{
    new_id, LutRef, Micros, Project, Segment, TimeRange, TrackKind, Transform,
};
use chukcut_engine::modules::project::grade::CurveChannel;
use chukcut_engine::modules::silence::commands as silence;
use chukcut_engine::modules::silence::SilenceParams;
use chukcut_engine::modules::text::commands as text;
use chukcut_engine::modules::timeline::commands as timeline;
use chukcut_engine::modules::timeline::ops::EditCommand;
use chukcut_engine::modules::tracking::commands as tracking;
use chukcut_engine::modules::tracking::FollowMode;
use chukcut_engine::modules::transitions::commands as transitions;
use chukcut_engine::shell::Channel;
use chukcut_engine::state::AppState;

const S: Micros = 1_000_000;
const W: u32 = 1280;
const H: u32 = 720;
const FPS: f64 = 30.0;

fn has(tool: &str) -> bool {
    Command::new(tool)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn fixture_dir() -> PathBuf {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/full-workflow-v1");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir
}

fn scratch() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("full_workflow");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

fn ffmpeg(args: &[&str], out: &Path) {
    if out.exists() {
        return;
    }
    let tmp = out.with_extension(format!(
        "tmp.{}",
        out.extension().unwrap().to_string_lossy()
    ));
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error"])
        .args(args)
        .arg(&tmp)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg failed for {}", out.display());
    std::fs::rename(&tmp, out).unwrap();
}

/// Clip A: 6 s of a dark, low-saturation picture with grain, and a tone that
/// speaks for 2 s and pauses for 1 s, over and over — the pauses are what the
/// silence removal finds. Low saturation keeps the magenta title box the only
/// magenta thing on screen.
fn clip_a(dir: &Path) -> PathBuf {
    let out = dir.join("a_talk.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x283038:size=1280x720:rate=30:duration=6,noise=alls=12:allf=t",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc='0.4*sin(2*PI*220*t)*lt(mod(t\\,3)\\,2)':s=48000:d=6",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
        &out,
    );
    out
}

/// Clip B: the tracking test's red disc on a known path, 4 s, with a tone.
fn clip_b(dir: &Path) -> PathBuf {
    let out = dir.join("b_disc.mp4");
    let disc = "color=c=red:size=80x80:rate=30:duration=4,format=rgba,\
                geq=r='230':g='30':b='40':a='if(lte(hypot(X-39.5,Y-39.5),38),255,0)'";
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x303030:size=1280x720:rate=30:duration=4,noise=alls=30:allf=t",
            "-f",
            "lavfi",
            "-i",
            disc,
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=330:sample_rate=48000:duration=4",
            "-filter_complex",
            "[0][1]overlay=x='100+280*t':y='320+150*sin(1.7*t)':eval=frame[v]",
            "-map",
            "[v]",
            "-map",
            "2:a",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-t",
            "4",
        ],
        &out,
    );
    out
}

/// A still, in a directory of its own with a stock licence record beside
/// it, so the export owes a credits file.
fn still(dir: &Path) -> PathBuf {
    let stock = dir.join("stock");
    std::fs::create_dir_all(&stock).unwrap();
    let out = stock.join("still.png");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x406080:size=640x360,format=rgb24",
            "-frames:v",
            "1",
        ],
        &out,
    );
    let mut origin = Origin::new(OriginKind::Stock, "pixabay");
    origin.title = "Blue wall".into();
    origin.creator = "Someone".into();
    origin.source_url = "https://example.invalid/photo/1".into();
    origin.licence = Licence::from_cc_url("https://creativecommons.org/licenses/by/4.0/");
    provenance::write_sidecar(&out, &origin).unwrap();
    out
}

/// A warm 2-point .cube: a LUT whose effect is visible and exactly known.
fn cube(dir: &Path) -> PathBuf {
    let out = dir.join("warm.cube");
    let mut text = String::from("TITLE \"warm\"\nLUT_3D_SIZE 2\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                let (r, g, b) = (r as f32, g as f32, b as f32);
                text.push_str(&format!(
                    "{:.3} {:.3} {:.3}\n",
                    (r * 0.9 + 0.1).min(1.0),
                    g,
                    b * 0.8
                ));
            }
        }
    }
    std::fs::write(&out, text).unwrap();
    out
}

fn srt(dir: &Path) -> PathBuf {
    let out = dir.join("captions.srt");
    std::fs::write(
        &out,
        "2\n00:00:03,000 --> 00:00:04,200\nsecond caption line\n\n\
         3\n00:00:06,000 --> 00:00:07,500\nthe third one\n",
    )
    .unwrap();
    out
}

fn ok<T>(label: &str, result: Result<T, String>) -> T {
    match result {
        Ok(v) => v,
        Err(e) => panic!("{label}: {e}"),
    }
}

fn snapshot(state: &Arc<AppState>) -> Project {
    state.with_project(|p| p.clone()).unwrap()
}

fn json(project: &Project) -> serde_json::Value {
    serde_json::to_value(project).unwrap()
}

/// The first segment on the first lane of `kind` holding `material`.
fn find(project: &Project, kind: TrackKind, material: &str) -> Vec<Segment> {
    project
        .tracks
        .iter()
        .filter(|t| t.kind == kind)
        .flat_map(|t| t.segments.iter())
        .filter(|s| s.material_id == material)
        .cloned()
        .collect()
}

/// `edits::append` from the app: the material at the end of the first lane
/// of its kind.
fn append(state: &Arc<AppState>, material: &str, kind: TrackKind, duration: Micros) -> String {
    let command = state
        .with_project(|p| {
            let track = p
                .tracks
                .iter()
                .find(|t| t.kind == kind && !t.locked)
                .unwrap();
            let start = track
                .segments
                .iter()
                .map(|s| s.target_range.end())
                .max()
                .unwrap_or(0);
            let id = new_id();
            (
                id.clone(),
                EditCommand::InsertSegment {
                    track_id: track.id.clone(),
                    index: track.segments.len(),
                    segment: Segment {
                        id,
                        material_id: material.to_string(),
                        target_range: TimeRange::new(start, duration),
                        source_range: TimeRange::new(0, duration),
                        render_index: 0,
                        speed: 1.0,
                        volume: 1.0,
                        transform: Transform::default(),
                        crop: None,
                        extras: Vec::new(),
                        keyframes: Vec::new(),
                    },
                },
            )
        })
        .unwrap();
    ok("append", timeline::timeline_apply(state, command.1));
    command.0
}

fn remove_command(project: &Project, id: &str) -> EditCommand {
    for track in &project.tracks {
        if let Some(index) = track.segments.iter().position(|s| s.id == id) {
            return EditCommand::RemoveSegment {
                track_id: track.id.clone(),
                segment: track.segments[index].clone(),
                index,
            };
        }
    }
    panic!("no segment {id}");
}

/// The app's ripple delete (`timeline::ripple::remove`): the clip, then every
/// later clip on its lane pulled left into the hole.
fn ripple_delete(state: &Arc<AppState>, id: &str) {
    let commands = state
        .with_project(|p| {
            let (track, segment) = p.segment(id).unwrap();
            let shift = segment.target_range.duration;
            let mut commands = vec![remove_command(p, id)];
            for s in &track.segments {
                if s.target_range.start > segment.target_range.start {
                    commands.push(EditCommand::MoveSegment {
                        segment_id: s.id.clone(),
                        from_track: track.id.clone(),
                        to_track: track.id.clone(),
                        from_start: s.target_range.start,
                        to_start: s.target_range.start - shift,
                    });
                }
            }
            commands
        })
        .unwrap();
    ok(
        "ripple delete",
        timeline::timeline_apply_many(state, commands, "Delete".into()),
    );
}

/// Run an export through `export_start` and wait for its terminal message.
fn run_export(
    state: &Arc<AppState>,
    output: &Path,
    hardware: Option<String>,
) -> Result<ExportProgress, String> {
    let (tx, rx) = mpsc::channel::<ExportProgress>();
    let channel = Channel::new(move |p: ExportProgress| tx.send(p).is_ok());
    let request = ExportRequest {
        output_path: output.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: Some(ExportOverrides {
            audio_codec: Some(AudioCodec::Aac),
            loudness_target: Some(-14.0),
            ..Default::default()
        }),
        hardware,
        include_audio: true,
        range: None,
    };
    export::export_start(state, request, channel)?;
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut last = None;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(progress) => {
                let terminal = progress.output_path.is_some() || progress.message.is_some();
                let failed = progress.message.is_some() && progress.output_path.is_none();
                last = Some(progress.clone());
                if failed {
                    return Err(progress.message.unwrap_or_default());
                }
                if terminal {
                    return Ok(progress);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Err(format!("the export never finished; last progress {last:?}"))
}

fn probe(path: &Path, entries: &str, stream: &str) -> String {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", stream, "-count_frames"])
        .args(["-show_entries", entries, "-of", "default=nw=1:nk=1"])
        .arg(path)
        .output()
        .expect("ffprobe");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn integrated_loudness(file: &Path) -> Option<f64> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(file)
        .args(["-af", "ebur128", "-f", "null", "-"])
        .output()
        .ok()?;
    let log = String::from_utf8_lossy(&out.stderr);
    let summary = log.rsplit("Summary:").next()?;
    summary.lines().find_map(|line| {
        line.trim()
            .strip_prefix("I:")
            .and_then(|rest| rest.trim().strip_suffix("LUFS"))
            .and_then(|v| v.trim().parse().ok())
    })
}

/// The decoded RGB frame of `file` at `seconds`.
fn frame_at(file: &Path, seconds: f64, (w, h): (u32, u32)) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-ss", &format!("{seconds:.3}"), "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("ffmpeg frame");
    assert_eq!(
        out.stdout.len(),
        (w * h * 3) as usize,
        "frame at {seconds}s of {}",
        file.display()
    );
    out.stdout
}

fn magenta_pixels(frame: &[u8]) -> usize {
    frame
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p[0] > 180 && p[1] < 90 && p[2] > 180)
        .count()
}

#[test]
fn a_whole_session_through_the_command_layer() {
    if !has("ffmpeg") || !has("ffprobe") {
        eprintln!("skipping: ffmpeg and ffprobe are needed");
        return;
    }
    if chukcut_engine::modules::gpu::render_context().is_none() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let work = scratch();
    // Autosave, caches, settings and logs stay out of the user's home.
    for (var, sub) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_DATA_HOME", "data"),
    ] {
        std::env::set_var(var, work.join(sub));
    }
    // What the app's `main` runs first: logging, settings, and the export's
    // audio source (without it every export is silent).
    chukcut_engine::init();
    // The app turns the working copy on; this session checks it, into the
    // scratch config above.
    chukcut_engine::modules::project::autosave::enable_for_process();

    let media = fixture_dir();
    let (a_path, b_path, still_path) = (clip_a(&media), clip_b(&media), still(&media));
    let lut_path = cube(&work);
    let srt_path = srt(&work);

    // --- new project, imports --------------------------------------------------
    let state = AppState::new();
    ok(
        "new",
        project::project_new(&state, "QA".into(), W, H, FPS, false).map(|_| ()),
    );
    let import = |path: &Path| {
        ok(
            "import",
            pollster::block_on(project::project_import_media(
                &state,
                path.to_string_lossy().into_owned(),
            )),
        )
    };
    let a = import(&a_path);
    let b = import(&b_path);
    let img = import(&still_path);
    assert!(a.has_audio && b.has_audio, "both clips carry sound");
    assert_eq!(
        snapshot(&state).materials.origins.len(),
        1,
        "the stock still's licence record was read"
    );
    // Importing into the pool is not an edit, but the first clip's canvas is:
    // the project was not given a canvas on purpose, so it takes clip A's 16:9
    // shape at a 1080 short edge, and that change is on the undo stack
    // (STATUS.md, "Documents"). It must be the only entry there.
    let adopted = snapshot(&state);
    assert_eq!(
        (adopted.canvas.width, adopted.canvas.height),
        (1920, 1080),
        "the first clip set the canvas"
    );
    assert_eq!(
        state.history.read().undo_label().as_deref(),
        Some("Project settings"),
        "the canvas adoption is the edit on the undo stack"
    );
    ok("undo adoption", timeline::timeline_undo(&state).map(|_| ()));
    let empty = snapshot(&state);
    assert_eq!((empty.canvas.width, empty.canvas.height), (W, H));
    assert!(
        !state.history.read().can_undo(),
        "the imports themselves left nothing to undo"
    );
    assert_eq!(
        serde_json::to_value(&empty.materials).unwrap(),
        serde_json::to_value(&adopted.materials).unwrap(),
        "undoing the canvas keeps every imported material"
    );
    ok("redo adoption", timeline::timeline_redo(&state).map(|_| ()));
    let redone = snapshot(&state);
    assert_eq!(
        (redone.canvas.width, redone.canvas.height, redone.fps),
        (adopted.canvas.width, adopted.canvas.height, adopted.fps),
        "redo puts the adopted canvas back"
    );

    // --- place, split, ripple delete, move, multi-select delete ---------------
    let a_seg = append(&state, &a.id, TrackKind::Video, a.duration);
    let b_seg = append(&state, &b.id, TrackKind::Video, b.duration);
    let img_seg = append(&state, &img.id, TrackKind::Video, 3 * S);
    let p = snapshot(&state);
    eprintln!(
        "after placing: {:?}",
        p.tracks
            .iter()
            .map(|t| (t.kind, t.segments.len()))
            .collect::<Vec<_>>()
    );

    ok(
        "split",
        timeline::timeline_split(&state, a_seg.clone(), 5 * S).map(|_| ()),
    );
    let p = snapshot(&state);
    let pieces = find(&p, TrackKind::Video, &a.id);
    assert_eq!(pieces.len(), 2, "the split made two pieces of A");
    let tail = pieces
        .iter()
        .find(|s| s.target_range.start == 5 * S)
        .expect("the second piece starts at the split")
        .id
        .clone();
    ripple_delete(&state, &tail);
    let p = snapshot(&state);
    let (_, b_now) = p.segment(&b_seg).unwrap();
    assert_eq!(b_now.target_range.start, 5 * S, "B closed up the gap");
    let (_, img_now) = p.segment(&img_seg).unwrap();
    assert_eq!(img_now.target_range.start, 9 * S);

    // Move the still one second right.
    let command = state
        .with_project(|p| {
            let (track, seg) = p.segment(&img_seg).unwrap();
            EditCommand::MoveSegment {
                segment_id: img_seg.clone(),
                from_track: track.id.clone(),
                to_track: track.id.clone(),
                from_start: seg.target_range.start,
                to_start: seg.target_range.start + S,
            }
        })
        .unwrap();
    ok("move", timeline::timeline_apply(&state, command));

    // Two more stills, selected together and deleted together.
    let extra1 = append(&state, &img.id, TrackKind::Video, 2 * S);
    let extra2 = append(&state, &img.id, TrackKind::Video, 2 * S);
    let commands = state
        .with_project(|p| vec![remove_command(p, &extra1), remove_command(p, &extra2)])
        .unwrap();
    ok(
        "multi delete",
        timeline::timeline_apply_many(&state, commands, "Delete".into()),
    );
    let p = snapshot(&state);
    assert!(p.segment(&extra1).is_none() && p.segment(&extra2).is_none());

    // --- grade -----------------------------------------------------------------
    let a_seg = find(&p, TrackKind::Video, &a.id)[0].id.clone();
    ok(
        "brightness",
        inspector::inspector_set_grade_control(
            &state,
            a_seg.clone(),
            GradeControl::Brightness,
            0.1,
        )
        .map(|_| ()),
    );
    ok(
        "contrast",
        inspector::inspector_set_grade_control(&state, a_seg.clone(), GradeControl::Contrast, 0.2)
            .map(|_| ()),
    );
    let lut = ok(
        "lut probe",
        inspector::inspector_lut_probe(lut_path.to_string_lossy().into_owned()),
    );
    eprintln!("lut: {lut:?}");
    ok(
        "lut",
        inspector::inspector_set_lut(
            &state,
            a_seg.clone(),
            Some(LutRef {
                path: lut_path.to_string_lossy().into_owned(),
                intensity: 0.8,
            }),
        )
        .map(|_| ()),
    );
    ok(
        "curve",
        inspector::inspector_set_curve(
            &state,
            a_seg.clone(),
            CurveChannel::Master,
            vec![[0.0, 0.0], [0.25, 0.2], [0.75, 0.82], [1.0, 1.0]],
        )
        .map(|_| ()),
    );

    // --- effects ---------------------------------------------------------------
    let catalog = fx::fx_catalog();
    assert!(!catalog.is_empty());
    let kind = catalog
        .iter()
        .find(|e| e.id == "vignette")
        .unwrap_or(&catalog[0])
        .id
        .to_string();
    ok(
        "fx add",
        fx::fx_add(&state, a_seg.clone(), kind.clone()).map(|_| ()),
    );
    let clip_kind = catalog
        .iter()
        .find(|e| e.id == "shake")
        .unwrap_or(&catalog[catalog.len() - 1])
        .id
        .to_string();
    let effect_clip = ok(
        "fx clip",
        fx::fx_add_clip(&state, clip_kind, 6 * S, Some(S), None),
    );

    // --- motion and titles -----------------------------------------------------
    let catalog = motion::motion_catalog();
    let preset = catalog
        .clip
        .iter()
        .find(|p| p.in_out)
        .expect("an In preset");
    ok(
        "clip animation",
        motion::motion_set_animation(
            &state,
            b_seg.clone(),
            AnimationSlot::In,
            Some(preset.animation()),
        )
        .map(|_| ()),
    );
    let title = ok(
        "title",
        text::text_add(&state, 0, Some("QA TITLE".into()), Some(4 * S)),
    );
    let mut material = state
        .with_project(|p| {
            p.materials
                .texts
                .iter()
                .find(|t| t.id == title.material_id)
                .cloned()
        })
        .unwrap()
        .expect("the title's material");
    material.font_size = 96.0;
    material.background = Some([1.0, 0.0, 1.0, 1.0]);
    ok("title style", text::text_set(&state, material).map(|_| ()));
    let mut animator = catalog.text[0].animator;
    animator.duration = 600_000;
    ok(
        "text animator",
        motion::motion_set_text_animation(
            &state,
            title.segment_id.clone(),
            TextSlot::In,
            Some(animator),
        )
        .map(|_| ()),
    );

    // --- captions --------------------------------------------------------------
    let added = ok(
        "captions import",
        captions::captions_import(&state, &srt_path, None, PlaceOptions::default()),
    );
    assert_eq!(added.segment_ids.len(), 2);
    // An .srt has no word timing, so karaoke has nothing to light in it (the
    // Captions tab says so). The first caption comes the way a transcript
    // delivers it, with its words timed.
    let words = ["hello", "there", "world"];
    let cue = chukcut_engine::modules::captions::Cue {
        start: 500_000,
        end: 1_800_000,
        text: words.join(" "),
        words: words
            .iter()
            .enumerate()
            .map(|(i, w)| chukcut_engine::modules::captions::TimedWord {
                text: w.to_string(),
                start: 500_000 + i as i64 * 433_000,
                end: 500_000 + (i as i64 + 1) * 433_000,
            })
            .collect(),
    };
    let timed = ok(
        "timed caption",
        captions::captions_add(&state, &[cue], None, PlaceOptions::default()),
    );
    let added_first = timed.segment_ids[0].clone();
    let listed = ok("captions list", captions::captions_list(&state));
    assert_eq!(listed.len(), 3, "all three captions on one lane");
    let mut style: CaptionStyle = ok(
        "caption style",
        captions::captions_style_of(&state, Some(&added_first)),
    );
    style.font_size *= 1.2;
    style.color = [1.0, 1.0, 0.9, 1.0];
    style.highlight = Some([1.0, 0.85, 0.0, 1.0]);
    ok(
        "caption restyle",
        captions::captions_set_style(&state, None, &style).map(|_| ()),
    );

    // --- transition between A and B -------------------------------------------
    ok(
        "transition",
        transitions::transitions_add(&state, b_seg.clone(), Default::default(), Some(400_000))
            .map(|_| ()),
    );

    // The working copy has the transition: a crash now must not lose it.
    chukcut_engine::modules::project::autosave::flush();
    let working = std::fs::read_to_string(chukcut_engine::modules::project::autosave::file())
        .expect("the working copy is written");
    let working: Project = serde_json::from_str(&working).unwrap();
    assert_eq!(
        working.materials.transitions.len(),
        1,
        "the transition is autosaved"
    );

    // --- tracking with a follower title ----------------------------------------
    let follower = ok(
        "follower title",
        text::text_add(&state, 5 * S, Some("FOLLOW".into()), Some(3 * S)),
    );
    // A cyan box, so the export can be searched for where the follower is.
    let mut material = state
        .with_project(|p| {
            p.materials
                .texts
                .iter()
                .find(|t| t.id == follower.material_id)
                .cloned()
        })
        .unwrap()
        .unwrap();
    material.font_size = 40.0;
    material.background = Some([0.0, 1.0, 1.0, 1.0]);
    ok(
        "follower style",
        text::text_set(&state, material).map(|_| ()),
    );
    let b_start = 5 * S;
    let job = ok(
        "tracking start",
        tracking::tracking_start(
            &state,
            tracking::StartTracking {
                target_segment_id: b_seg.clone(),
                at: b_start,
                rect: [140.0 / 1280.0, 360.0 / 720.0, 84.0 / 1280.0, 84.0 / 720.0],
                direction: Default::default(),
                overlay_id: Some(follower.segment_id.clone()),
                mode: FollowMode::default(),
                retrack: None,
                tracker: None,
            },
            None,
        ),
    );
    let deadline = Instant::now() + Duration::from_secs(300);
    let outcome = loop {
        let status = tracking::tracking_status(job).expect("job status");
        if let Some(finished) = status.finished {
            break ok("tracking", finished);
        }
        assert!(Instant::now() < deadline, "tracking never finished");
        std::thread::sleep(Duration::from_millis(100));
    };
    tracking::tracking_forget(job);
    assert!(outcome.frames >= 100, "tracked {} frames", outcome.frames);
    let p = snapshot(&state);
    let (_, overlay) = p.segment(&follower.segment_id).unwrap();
    assert!(
        p.materials.follow_of(overlay).is_some(),
        "the follower title is linked to the track"
    );

    // --- silence removal on A, everything kept in sync --------------------------
    let p = snapshot(&state);
    let a_seg = find(&p, TrackKind::Video, &a.id)[0].id.clone();
    let before = p.duration();
    let analysis = ok(
        "silence analyse",
        silence::silence_analyse(&state, a_seg.clone(), false, &AtomicBool::new(false)),
    );
    let params = SilenceParams {
        threshold_db: analysis.suggested_threshold_db,
        min_silence: 500_000,
        padding: 100_000,
        ..Default::default()
    };
    let cuts = silence::silence_detect(&analysis, &params);
    eprintln!("silence cuts: {cuts:?}");
    assert_eq!(cuts.len(), 1, "one pause inside the first 5 s: {cuts:?}");
    let cut: Micros = cuts.iter().map(|c| c.duration).sum();
    ok(
        "silence remove",
        silence::silence_remove(&state, a_seg, cuts, "Remove silences".into(), true).map(|_| ()),
    );
    let p = snapshot(&state);
    assert_eq!(p.duration(), before - cut, "every lane rippled by the cut");
    let issues = ok("validate", project::project_validate(&state));
    eprintln!("validation after the session: {issues:?}");
    let (_, b_after) = p.segment(&b_seg).expect("B survived");
    let (_, f_after) = p.segment(&follower.segment_id).expect("follower survived");
    assert_eq!(
        b_after.target_range.start, f_after.target_range.start,
        "the follower moved with B"
    );

    let final_project = snapshot(&state);

    // --- export: software and every usable hardware encoder ---------------------
    let mut encoders: Vec<(String, Option<String>)> = vec![("software".into(), None)];
    for encoder in chukcut_engine::modules::export::hwaccel::detect() {
        if encoder.usable && encoder.codec == VideoCodec::H264 {
            encoders.push((encoder.id.clone(), Some(encoder.id.clone())));
        }
    }
    eprintln!("encoders: {encoders:?}");
    let duration = final_project.duration();
    let canvas = (final_project.canvas.width, final_project.canvas.height);
    let expected_frames = ((duration as f64 / S as f64) * FPS).round() as i64;
    let title_time = 1.8;
    for (label, hardware) in encoders {
        let output = work.join(format!("export-{label}.mp4"));
        let done = ok(
            &format!("export {label}"),
            run_export(&state, &output, hardware),
        );
        let output = PathBuf::from(done.output_path.unwrap());
        let frames: i64 = probe(&output, "stream=nb_read_frames", "v:0")
            .parse()
            .unwrap_or(-1);
        assert!(
            (frames - expected_frames).abs() <= 1,
            "{label}: {frames} frames, expected {expected_frames}"
        );
        let seconds: f64 = probe(&output, "format=duration", "v:0")
            .lines()
            .last()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0.0);
        assert!(
            (seconds - duration as f64 / S as f64).abs() < 0.1,
            "{label}: {seconds} s, expected {}",
            duration as f64 / S as f64
        );
        let lufs = integrated_loudness(&output).expect("ffmpeg measured the export");
        assert!((lufs + 14.0).abs() <= 1.0, "{label}: {lufs} LUFS");
        let frame = frame_at(&output, title_time, canvas);
        let magenta = magenta_pixels(&frame);
        assert!(
            magenta > 2_000,
            "{label}: the title box is not on screen ({magenta} magenta pixels)"
        );
        // The grade: clip A is a cool grey (0x283038); the warm LUT and the
        // brightness lift make it warm.
        let (w, h) = canvas;
        let patch = |frame: &[u8], cx: u32, cy: u32| {
            let mut sum = [0u64; 3];
            let mut n = 0;
            for y in cy - 20..cy + 20 {
                for x in cx - 20..cx + 20 {
                    let i = ((y * w + x) * 3) as usize;
                    for c in 0..3 {
                        sum[c] += frame[i + c] as u64;
                    }
                    n += 1;
                }
            }
            sum.map(|v| v / n)
        };
        let graded = patch(&frame, w / 4, h / 3);
        assert!(
            graded[0] > graded[2] + 5,
            "{label}: clip A is not graded warm: {graded:?}"
        );
        // The first caption, in its highlight colour as the words are
        // spoken: yellow pixels in the lower part of the frame.
        let caption = frame_at(&output, 1.2, canvas);
        let yellow = caption[(w * h * 3 / 2) as usize..]
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|p| p[0] > 200 && p[1] > 160 && p[2] < 90)
            .count();
        assert!(yellow > 200, "{label}: no karaoke highlight ({yellow} px)");
        // The follower keeps its own place and copies the disc's motion
        // (`tracking_attach` does not move it): its cyan box travels exactly
        // as far as the red disc between two instants.
        let b_at = b_after.target_range.start;
        let mut seen = Vec::new();
        // Early enough that the follower, which starts mid-frame, stays in it.
        for t in [0.3f64, 1.3] {
            let at = b_at as f64 / S as f64 + t;
            let frame = frame_at(&output, at, canvas);
            let centre = |hit: &dyn Fn(&[u8]) -> bool| {
                let (mut sx, mut sy, mut n) = (0u64, 0u64, 0u64);
                for (i, p) in frame.as_chunks::<3>().0.iter().enumerate() {
                    if hit(p) {
                        sx += (i as u32 % w) as u64;
                        sy += (i as u32 / w) as u64;
                        n += 1;
                    }
                }
                (n > 50).then(|| (sx as f64 / n as f64, sy as f64 / n as f64))
            };
            let disc = centre(&|p| p[0] > 170 && p[1] < 80 && p[2] < 90);
            let cyan = centre(&|p| p[0] < 80 && p[1] > 180 && p[2] > 180);
            let (Some(disc), Some(cyan)) = (disc, cyan) else {
                panic!("{label} at {at:.2}s: disc {disc:?}, follower {cyan:?}");
            };
            seen.push((disc, cyan));
        }
        let moved = |a: (f64, f64), b: (f64, f64)| (b.0 - a.0, b.1 - a.1);
        let disc_moved = moved(seen[0].0, seen[1].0);
        let follower_moved = moved(seen[0].1, seen[1].1);
        let error = ((disc_moved.0 - follower_moved.0).powi(2)
            + (disc_moved.1 - follower_moved.1).powi(2))
        .sqrt();
        assert!(
            disc_moved.0 > 0.2 * w as f64 && error < 0.02 * w as f64,
            "{label}: the disc moved {disc_moved:?}, the follower {follower_moved:?}"
        );
        let late = frame_at(&output, 7.0, canvas);
        assert!(
            magenta_pixels(&late) < 50,
            "{label}: magenta where no title is"
        );

        // What the export dialog does after a finished export.
        let sidecar = captions::sidecar_path(&output, SubtitleFormat::Srt);
        let written = ok(
            "captions sidecar",
            captions::captions_export(&state, &sidecar, None),
        );
        assert_eq!(written, 3);
        assert!(std::fs::read_to_string(&sidecar)
            .unwrap()
            .contains("hello there world"));
        let credits = ok("credits", cloud::cloud_write_credits(&state, &output));
        let credits = credits.expect("a CC BY still needs a credits file");
        let credits = std::fs::read_to_string(credits).unwrap();
        assert!(credits.contains("Blue wall"), "credits: {credits}");
    }

    // --- save, reopen, byte-identical round trip --------------------------------
    let saved = work.join("session.chukcut");
    ok(
        "save",
        project::project_save(&state, Some(saved.to_string_lossy().into_owned())).map(|_| ()),
    );
    let reopened = AppState::new();
    ok(
        "open",
        project::project_open(&reopened, saved.to_string_lossy().into_owned()).map(|_| ()),
    );
    let again = work.join("session-again.chukcut");
    ok(
        "save again",
        project::project_save(&reopened, Some(again.to_string_lossy().into_owned())).map(|_| ()),
    );
    assert_eq!(
        std::fs::read(&saved).unwrap(),
        std::fs::read(&again).unwrap(),
        "a reopened project saves byte for byte the same"
    );
    // The file leaves out the materials nothing reaches any more (the grade
    // edits above left three colour adjustments behind); everything else is
    // the session's document exactly.
    let mut final_file = final_project.clone();
    let pruned = chukcut_engine::modules::project::prune::prune_unreferenced(&mut final_file);
    assert!(
        pruned >= 3,
        "superseded grades are left out of the file: {pruned}"
    );
    assert_eq!(json(&snapshot(&reopened)), json(&final_file));
    // The live pool still has them: undo below needs every one.
    assert!(
        snapshot(&state).materials.color_adjusts.len() > final_file.materials.color_adjusts.len()
    );

    // --- undo everything, redo everything ---------------------------------------
    let mut undone = 0;
    while state.history.read().can_undo() {
        ok("undo", timeline::timeline_undo(&state).map(|_| ()));
        undone += 1;
        assert!(undone < 1000, "undo never reached the start");
    }
    eprintln!("undid {undone} steps");
    let start = snapshot(&state);
    assert_eq!(
        serde_json::to_value(&start.tracks).unwrap(),
        serde_json::to_value(&empty.tracks).unwrap(),
        "undo reached the empty timeline"
    );
    let mut start_rest = json(&start);
    let mut empty_rest = json(&empty);
    let (start_pool, empty_pool) = (
        start_rest.as_object_mut().unwrap().remove("materials"),
        empty_rest.as_object_mut().unwrap().remove("materials"),
    );
    assert_eq!(start_rest, empty_rest, "undo reached the empty project");
    // Titles and captions add their materials outside the undo stack (as
    // imports do); whatever is left over must at least be unreferenced.
    if start_pool != empty_pool {
        let (a, b) = (start_pool.unwrap(), empty_pool.unwrap());
        for (key, value) in a.as_object().unwrap() {
            if b.get(key) != Some(value) {
                let count = |v: &serde_json::Value| {
                    v.as_array()
                        .map(|a| a.len())
                        .or(v.as_object().map(|o| o.len()))
                        .unwrap_or(0)
                };
                eprintln!(
                    "pool `{key}` after undoing everything: {} entries, at the start {}",
                    count(value),
                    b.get(key).map(count).unwrap_or(0)
                );
            }
        }
    }
    let mut redone = 0;
    while state.history.read().can_redo() {
        ok("redo", timeline::timeline_redo(&state).map(|_| ()));
        redone += 1;
    }
    assert_eq!(undone, redone);
    let end = snapshot(&state);
    assert_eq!(
        json(&end),
        json(&final_project),
        "redo rebuilt the session exactly"
    );
    assert!(end.segment(&effect_clip.segment_id).is_some());
}

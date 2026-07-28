//! Removing imported media without losing the cut.
//!
//! The owner's scenario, end to end: import two files, place a clip from each,
//! remove one material as an undoable edit — the clip stays on the timeline
//! and goes *offline* rather than being deleted. Saving and reloading keeps
//! it, `validate()` names it as a warning rather than branding the file
//! corrupt, the compositor draws an unmistakable placeholder where its picture
//! would be, and one undo makes everything whole again.
//!
//! The same missing state has a second cause with the same contract: a file
//! that is gone from *disk* while its material is still in the pool. And the
//! export refuses both, by name, because a placeholder is honest in a preview
//! and a defect in a delivered file.

mod support;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_lib::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportRequest,
};
use chukcut_lib::modules::media::{self, MediaSourceProvider, MISSING_MEDIA_RGBA};
use chukcut_lib::modules::project::commands::import_material;
use chukcut_lib::modules::project::document::{
    CanvasConfig, Project, Severity, Track, TrackKind,
};
use chukcut_lib::modules::project::migrate;
use chukcut_lib::modules::render::{Compositor, CompositorConfig, SourceProvider};
use chukcut_lib::modules::timeline::ops::{EditCommand, PoolMaterial};
use chukcut_lib::modules::timeline::History;

use support::{assert_pixel_near, canonical, segment};

/// The placeholder is a flat upload and a straight copy through the sRGB
/// pipeline — nothing here has been near a codec, so the tolerance only covers
/// the 8-bit round trip through the linear render target.
const PLACEHOLDER_TOLERANCE: i32 = 6;

fn blank_project() -> Project {
    Project::new(
        "missing media",
        CanvasConfig {
            width: 320,
            height: 240,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    )
}

/// Import `file` the way the app does — probe, then the pure half of
/// `project_import_media` — and answer the material id.
fn import(project: &mut Project, file: &std::path::Path) -> String {
    let info = media::probe(file).expect("probe the fixture");
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    import_material(project, &file.to_string_lossy(), &name, &info)
        .expect("import the fixture")
        .id
}

/// The `RemoveMaterial` command for a video material, as the frontend builds
/// it: the whole pool entry plus its index.
fn remove_video(project: &Project, id: &str) -> EditCommand {
    let index = project
        .materials
        .videos
        .iter()
        .position(|m| m.id == id)
        .expect("the material is in the pool");
    EditCommand::RemoveMaterial {
        material: PoolMaterial::Video(project.materials.videos[index].clone()),
        index,
    }
}

fn dangling_warnings(project: &Project) -> Vec<String> {
    project
        .validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Warning && i.message.contains("unknown material"))
        .map(|i| i.message)
        .collect()
}

#[test]
fn removing_a_material_keeps_the_clip_survives_a_reload_and_undoes() {
    let media = require_media!();
    let ctx = require_gpu!();

    // Two imports, one clip each: red covers [0, 2 s), green [2 s, 4 s).
    let mut project = blank_project();
    let red = import(&mut project, &media.solid_red_landscape);
    let green = import(&mut project, &media.solid_green_portrait);
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment(&red, 0, 2_000_000));
    track.segments.push(segment(&green, 2_000_000, 2_000_000));
    project.tracks.push(track);
    assert!(dangling_warnings(&project).is_empty());
    let before_removal = canonical(&project);

    // Remove the red material the way the user does: one undoable edit.
    let mut history = History::new();
    let removal = remove_video(&project, &red);
    history
        .apply(&mut project, removal)
        .expect("removing a material is an ordinary edit");

    // The material is gone from the pool; the clip is not gone from the cut.
    assert!(project.materials.video(&red).is_none());
    assert_eq!(project.tracks[0].segments.len(), 2, "both clips survive");
    let warned = dangling_warnings(&project);
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(
        !project
            .validate()
            .iter()
            .any(|i| i.severity == Severity::Error),
        "an offline clip is a warning, never a corrupt document"
    );

    // Save and reload through the real load path — the "logical container"
    // must survive the trip with the offline clip still in it.
    let saved = serde_json::to_string_pretty(&project).expect("save");
    let reloaded = migrate::load(&saved).expect("a project with an offline clip reopens");
    assert!(reloaded.warnings.is_empty(), "{:?}", reloaded.warnings);
    let reloaded = reloaded.project;
    assert_eq!(reloaded.tracks[0].segments.len(), 2);
    assert_eq!(dangling_warnings(&reloaded).len(), 1);

    // The compositor draws the placeholder where the offline clip is, and the
    // clip whose media survived is untouched.
    let compositor = Compositor::new(ctx);
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&reloaded));
    let offline = compositor
        .render(&reloaded, 1_000_000, (320, 240), sources.as_ref())
        .expect("an offline clip renders rather than failing the frame");
    assert_pixel_near(
        offline.pixel(160, 120),
        MISSING_MEDIA_RGBA,
        PLACEHOLDER_TOLERANCE,
        "the offline clip composites the placeholder",
    );
    let healthy = compositor
        .render(&reloaded, 3_000_000, (320, 240), sources.as_ref())
        .expect("render the healthy clip");
    assert_pixel_near(
        healthy.pixel(160, 120),
        [0, 255, 0, 255],
        8,
        "the clip whose media survived still draws its own picture",
    );

    // One undo step restores the material and normality, byte-exactly.
    history.undo(&mut project).expect("undo the removal");
    assert_eq!(canonical(&project), before_removal);
    assert!(dangling_warnings(&project).is_empty());
}

#[test]
fn a_file_gone_from_disk_warns_and_composites_the_placeholder() {
    let media = require_media!();
    let ctx = require_gpu!();

    // The imported file lives outside the fixture cache so it can be deleted.
    let doomed = std::env::temp_dir().join(format!(
        "chukcut-missing-media-{}.mp4",
        std::process::id()
    ));
    std::fs::copy(&media.solid_red_landscape, &doomed).expect("copy the fixture");

    let mut project = blank_project();
    let id = import(&mut project, &doomed);
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment(&id, 0, 2_000_000));
    project.tracks.push(track);

    std::fs::remove_file(&doomed).expect("delete the file behind the clip");

    // The material is still in the pool — nothing deleted the user's cut —
    // and validate names the missing file as a warning.
    assert!(project.materials.video(&id).is_some());
    let issues = project.validate();
    assert!(
        issues.iter().any(|i| {
            i.severity == Severity::Warning && i.message.contains("media file is missing")
        }),
        "{issues:?}"
    );

    let compositor = Compositor::new(ctx);
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
    let frame = compositor
        .render(&project, 1_000_000, (320, 240), sources.as_ref())
        .expect("a clip whose file is gone renders rather than failing the frame");
    assert_pixel_near(
        frame.pixel(160, 120),
        MISSING_MEDIA_RGBA,
        PLACEHOLDER_TOLERANCE,
        "the clip with no file composites the placeholder",
    );
}

#[test]
fn an_export_refuses_missing_media_and_names_the_clips() {
    let media = require_media!();
    let ctx = require_gpu!();

    // Three clips: one healthy, one whose material was removed from the pool,
    // one whose file is gone from disk.
    let mut project = blank_project();
    let healthy = import(&mut project, &media.counter);
    project.materials.videos.push(
        chukcut_lib::modules::project::document::VideoMaterial {
            id: "ghost-file".into(),
            path: "/nonexistent/chukcut-gone.mp4".into(),
            width: 320,
            height: 240,
            duration: 4_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        },
    );
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment(&healthy, 0, 1_000_000));
    track
        .segments
        .push(segment("removed-material", 1_000_000, 1_000_000));
    track
        .segments
        .push(segment("ghost-file", 2_000_000, 1_000_000));
    project.tracks.push(track);

    // The helper alone: exactly the two broken clips, in timeline order.
    let lines = job::missing_media(&project);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("removed from the project"), "{lines:?}");
    assert!(lines[1].contains("gone from disk"), "{lines:?}");
    assert!(
        lines[1].contains("/nonexistent/chukcut-gone.mp4"),
        "the message names the file: {lines:?}"
    );

    // And the whole export refuses before writing anything.
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("missing_media")
        .join("refused.mp4");
    std::fs::create_dir_all(out.parent().unwrap()).expect("scratch dir");
    let settings = resolve_settings(
        &project,
        &ExportRequest {
            output_path: out.to_string_lossy().into_owned(),
            preset_id: None,
            overrides: None,
            hardware: None,
            include_audio: false,
            range: None,
        },
    )
    .expect("the settings themselves are fine");
    let export = ExportJob {
        job_id: "missing-media".into(),
        project: project.clone(),
        settings,
        compositor: Arc::new(Compositor::with_config(
            ctx,
            CompositorConfig {
                strict_sources: true,
                ..Default::default()
            },
        )),
        sources: Arc::new(MediaSourceProvider::from_project(&project)),
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    };

    let error = run_export(&export, &()).expect_err("missing media refuses the export");
    let message = error.to_string();
    assert!(
        message.contains("2 clips reference media that is missing"),
        "{message}"
    );
    assert!(message.contains("removed from the project"), "{message}");
    assert!(message.contains("gone from disk"), "{message}");
    assert!(
        !out.exists(),
        "a refused export leaves nothing that looks like a video"
    );
}

//! The project-level settings edit: name, canvas, frame rate, background.
//!
//! Everything a timeline edit touches goes through `timeline::ops::EditCommand`,
//! but that enum is about tracks and segments — nothing in it can write the
//! canvas. Rather than growing a document-level variant into an enum another
//! team owns, the configure edit is its own command with the same contract:
//! it carries both the state it writes and the state it replaces, so its
//! inverse is exact and free. `crate::state::DocumentHistory` is what makes it
//! share one undo stack with the timeline commands.
//!
//! What this deliberately does **not** do: re-time anything. Times in the
//! document are `i64` microseconds and never frames, so changing `fps` changes
//! how the timeline is *rendered and exported*, not where any clip sits or how
//! long it runs. The settings dialog says so out loud, because "change the
//! project to 60 fps" reads like it might re-time and it must not.

use serde::{Deserialize, Serialize};

use super::document::Project;

/// The whole of what the settings dialog can change, snapshotted together.
///
/// One struct rather than a field per command, because the dialog commits all
/// of it at once and a single undo step has to put all of it back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Linear RGBA 0..1, exactly as `CanvasConfig::background` stores it.
    pub background: [f32; 4],
}

impl ProjectConfig {
    pub fn of(project: &Project) -> Self {
        Self {
            name: project.name.clone(),
            width: project.canvas.width,
            height: project.canvas.height,
            fps: project.fps,
            background: project.canvas.background,
        }
    }

    /// Refuse values the rest of the engine cannot work with.
    ///
    /// The rules mirror what canvas adoption in `commands::import_material`
    /// already enforces for the sizes it computes: even dimensions because
    /// every 4:2:0 encoder requires them (an odd canvas is an export error
    /// found half an hour after the choice that caused it), and a finite
    /// positive fps because `frame_duration` divides by it.
    fn validate(&self) -> Result<(), String> {
        if self.width < 2 || self.height < 2 {
            return Err("the canvas must be at least 2\u{d7}2 pixels".to_string());
        }
        if !self.width.is_multiple_of(2) || !self.height.is_multiple_of(2) {
            return Err(format!(
                "the canvas must have even dimensions; {}x{} has an odd edge",
                self.width, self.height
            ));
        }
        if self.width > 8192 || self.height > 8192 {
            return Err("the canvas cannot be larger than 8192 pixels on an edge".to_string());
        }
        if !self.fps.is_finite() || self.fps < 1.0 || self.fps > 240.0 {
            return Err("the frame rate must be between 1 and 240".to_string());
        }
        if self.name.trim().is_empty() {
            return Err("the project needs a name".to_string());
        }
        if self.background.iter().any(|c| !c.is_finite()) {
            return Err("the background colour is not a colour".to_string());
        }
        Ok(())
    }

    fn apply_to(&self, project: &mut Project) {
        project.name = self.name.clone();
        project.canvas.width = self.width;
        project.canvas.height = self.height;
        project.canvas.background = self.background;
        project.fps = self.fps;
    }
}

/// One settings change, invertible.
///
/// The same shape as every `EditCommand` variant: `before` is not derived at
/// undo time, it was captured from the live document when the edit was made,
/// which is what makes the inverse exact rather than approximate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigureCommand {
    pub before: ProjectConfig,
    pub after: ProjectConfig,
}

impl ConfigureCommand {
    /// Build the command against the live document.
    pub fn new(project: &Project, after: ProjectConfig) -> Self {
        Self {
            before: ProjectConfig::of(project),
            after,
        }
    }

    /// Whether applying it would change anything at all. A no-op must not land
    /// on the undo stack: an "Undo Project Settings" that visibly does nothing
    /// teaches the user that undo is broken.
    pub fn is_noop(&self) -> bool {
        self.before == self.after
    }

    pub fn apply(&self, project: &mut Project) -> Result<(), String> {
        self.after.validate()?;
        self.after.apply_to(project);
        Ok(())
    }

    pub fn invert(&self) -> Self {
        Self {
            before: self.after.clone(),
            after: self.before.clone(),
        }
    }

    pub fn label(&self) -> String {
        // Sentence case, like every label in `EditCommand::label`.
        "Project settings".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::CanvasConfig;

    fn project() -> Project {
        Project::new("Cut", CanvasConfig::default(), 30.0)
    }

    fn config(project: &Project) -> ProjectConfig {
        ProjectConfig::of(project)
    }

    #[test]
    fn applies_and_inverts_byte_exactly() {
        let mut p = project();
        let original = config(&p);

        let command = ConfigureCommand::new(
            &p,
            ProjectConfig {
                name: "Renamed".into(),
                width: 1920,
                height: 1080,
                fps: 60.0,
                background: [0.1, 0.2, 0.3, 1.0],
            },
        );
        command.apply(&mut p).unwrap();
        assert_eq!(p.name, "Renamed");
        assert_eq!((p.canvas.width, p.canvas.height), (1920, 1080));
        assert_eq!(p.fps, 60.0);
        assert_eq!(p.canvas.background, [0.1, 0.2, 0.3, 1.0]);

        command.invert().apply(&mut p).unwrap();
        assert_eq!(config(&p), original);
    }

    #[test]
    fn refuses_what_the_engine_cannot_encode() {
        let p = project();
        let base = config(&p);

        // An odd canvas is an export error waiting half an hour downstream.
        let odd = ConfigureCommand::new(
            &p,
            ProjectConfig {
                width: 1235,
                ..base.clone()
            },
        );
        assert!(odd.apply(&mut project()).unwrap_err().contains("even"));

        let zero = ConfigureCommand::new(
            &p,
            ProjectConfig {
                width: 0,
                ..base.clone()
            },
        );
        assert!(zero.apply(&mut project()).is_err());

        let nan_fps = ConfigureCommand::new(
            &p,
            ProjectConfig {
                fps: f64::NAN,
                ..base.clone()
            },
        );
        assert!(nan_fps.apply(&mut project()).is_err());

        let unnamed = ConfigureCommand::new(
            &p,
            ProjectConfig {
                name: "  ".into(),
                ..base
            },
        );
        assert!(unnamed.apply(&mut project()).is_err());

        // And a refused command left the document untouched.
        let mut untouched = project();
        let before = config(&untouched);
        let bad = ConfigureCommand::new(
            &untouched,
            ProjectConfig {
                fps: 0.0,
                ..before.clone()
            },
        );
        assert!(bad.apply(&mut untouched).is_err());
        assert_eq!(config(&untouched), before);
    }

    #[test]
    fn changing_fps_touches_no_time_in_the_document() {
        // The whole warning in the dialog, as a property: times are micros and
        // fps is presentation, so a rate change moves nothing.
        use crate::modules::project::{Segment, TimeRange, Track, TrackKind, Transform};

        let mut p = project();
        let mut track = Track::new(TrackKind::Video, "V1");
        let segment = Segment {
            id: "clip-1".into(),
            material_id: "mat-1".into(),
            target_range: TimeRange {
                start: 1_000_000,
                duration: 2_000_000,
            },
            source_range: TimeRange {
                start: 0,
                duration: 2_000_000,
            },
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        track.segments.push(segment.clone());
        p.tracks.push(track);

        let command = ConfigureCommand::new(
            &p,
            ProjectConfig {
                fps: 24.0,
                ..config(&p)
            },
        );
        command.apply(&mut p).unwrap();

        assert_eq!(p.tracks[0].segments[0].target_range, segment.target_range);
        assert_eq!(p.tracks[0].segments[0].source_range, segment.source_range);
    }

    #[test]
    fn a_noop_knows_it_is_one() {
        let p = project();
        assert!(ConfigureCommand::new(&p, config(&p)).is_noop());
        let renamed = ConfigureCommand::new(
            &p,
            ProjectConfig {
                name: "x".into(),
                ..config(&p)
            },
        );
        assert!(!renamed.is_noop());
    }
}

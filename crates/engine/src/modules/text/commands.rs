//! Commands for titles.
//!
//! List the fonts this machine can draw, put a title on the timeline (plain,
//! in a style, or from a template), and change one that is already there.
//!
//! ## Undo
//!
//! Every change to a title is one `EditCommand`, so it undoes like any other
//! edit: [`text_set`] and [`text_set_content`] are one
//! `EditCommand::SetTextMaterial` each, and a style, a template or a position
//! preset is one `Composite`. The inspector previews a slider drag on a copy
//! of the document and sends one of these on release, so a drag is one step
//! and not forty.
//!
//! [`text_add`] puts the *material* into the pool directly, exactly as
//! `project_import_media` does and for the same reason — a material nothing
//! references is inert, and putting library additions in the undo stack
//! means Ctrl+Z after a cut silently empties the panel. The segment that
//! names it is inserted through `History::apply`, so the title itself
//! appears and disappears with Ctrl+Z.
//!
//! All paths schedule an autosave, so none is lost to a restart.

use std::sync::Arc;

use super::{edit, presets};
use crate::modules::project::document::{Micros, TextMaterial};
use crate::modules::timeline::commands::EditResponse;
use crate::state::AppState;

/// Every font family this machine can draw, sorted.
///
/// From the same `fontique` collection the rasteriser resolves against, so a
/// name offered here is a name that will render. Building the shared renderer
/// scans the system's fonts — tens of milliseconds, once per process — which is
/// why this goes off the main thread even though every call after the first is
/// a clone of a `Vec<String>`.
pub async fn text_fonts() -> Result<Vec<String>, String> {
    crate::shell::spawn_blocking(|| super::TextRenderer::shared().font_families())
        .await
        .map_err(|error| format!("listing the fonts failed: {error}"))
}

/// What the frontend needs after a title has been added: the document, the
/// history state, and enough identity to select what was just created.
#[derive(serde::Serialize)]
pub struct TextAdded {
    #[serde(flatten)]
    pub edit: EditResponse,
    pub material_id: String,
    pub segment_id: String,
    pub track_id: String,
    /// Where it actually landed, which is not `at` when that instant was taken.
    pub start: Micros,
}

/// Put a new title on the timeline at `at`.
///
/// `content` is optional: the button sends nothing and gets the placeholder,
/// while a preset can send its own words. `duration` likewise defaults to
/// [`edit::DEFAULT_DURATION`].
pub fn text_add(
    state: &Arc<AppState>,
    at: Micros,
    content: Option<String>,
    duration: Option<Micros>,
) -> Result<TextAdded, String> {
    text_add_on(state, at, content, duration, None)
}

/// [`text_add`] on `lane` when that is a title lane (a text lane that is not
/// locked and holds no captions); elsewhere where [`text_add`] would put it.
pub fn text_add_on(
    state: &Arc<AppState>,
    at: Micros,
    content: Option<String>,
    duration: Option<Micros>,
    lane: Option<String>,
) -> Result<TextAdded, String> {
    let (material_id, placement) = {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;

        let material = edit::default_material(project, content);
        let material_id = material.id.clone();
        let placement = edit::insert_command_on(
            project,
            &material_id,
            at,
            duration.unwrap_or(edit::DEFAULT_DURATION),
            lane.as_deref(),
        )?;

        // Before the command, because `InsertSegment` produces a document that
        // `validate()` reads: a segment naming a material that is not in the
        // pool is an error there, and it would be a real one for the moment
        // between the two writes if an autosave landed in between.
        project.materials.texts.push(material);
        state
            .history
            .write()
            .apply(project, placement.command.clone())?;

        (material_id, placement)
    };

    Ok(TextAdded {
        edit: respond(state)?,
        material_id,
        segment_id: placement.segment_id,
        track_id: placement.track_id,
        start: placement.start,
    })
}

/// Replace a title's parameters wholesale, keeping its identity, as one undo
/// step.
///
/// Wholesale rather than a patch of changed fields for the reason the rest of
/// this boundary gives: the document is server state, the frontend holds a
/// complete copy of it, and a patch is a second description of the same object
/// that can disagree with the first. Setting what is already there changes
/// nothing and adds no step.
pub fn text_set(state: &Arc<AppState>, material: TextMaterial) -> Result<EditResponse, String> {
    // Before the lock, because a rejected edit should cost nothing and because
    // the message is the user's.
    edit::check_material(&material)?;
    let before = state
        .with_project(|p| p.materials.text(&material.id).cloned())?
        .ok_or_else(|| format!("no title with the id {}", material.id))?;
    if before == material {
        return respond(state);
    }
    crate::modules::timeline::commands::timeline_apply(
        state,
        crate::modules::timeline::ops::EditCommand::SetTextMaterial {
            before,
            after: material,
        },
    )
}

/// Every title style, in the order the asset panel shows them.
pub fn text_styles() -> Vec<presets::TitleStyle> {
    presets::styles()
}

/// Every text template, in the order the asset panel shows them.
pub fn text_templates() -> Vec<presets::TextTemplate> {
    presets::templates()
}

/// Put a new title in style `style_id` on the timeline at `at`, on `lane`
/// when that is a title lane. It says the style's sample unless `content`
/// is given, and lands where the style puts it (a lower third low and left).
pub fn text_add_style(
    state: &Arc<AppState>,
    at: Micros,
    style_id: &str,
    content: Option<String>,
    lane: Option<String>,
) -> Result<TextAdded, String> {
    text_add_style_for(state, at, style_id, content, lane, None)
}

/// [`text_add_style`] for `duration` instead of the default three seconds.
pub fn text_add_style_for(
    state: &Arc<AppState>,
    at: Micros,
    style_id: &str,
    content: Option<String>,
    lane: Option<String>,
    duration: Option<Micros>,
) -> Result<TextAdded, String> {
    let style = presets::style(style_id)
        .ok_or_else(|| format!("there is no title style called {style_id}"))?;
    add_title(state, at, lane, duration, None, |project| {
        (style.material(project, content), style.transform(), None)
    })
}

/// Put a new title from template `template_id` on the timeline at `at`: its
/// style and its animation, as one clip and one undo step.
pub fn text_add_template(
    state: &Arc<AppState>,
    at: Micros,
    template_id: &str,
    content: Option<String>,
    lane: Option<String>,
) -> Result<TextAdded, String> {
    text_add_template_for(state, at, template_id, content, lane, None)
}

/// [`text_add_template`] for `duration` instead of the default three seconds.
pub fn text_add_template_for(
    state: &Arc<AppState>,
    at: Micros,
    template_id: &str,
    content: Option<String>,
    lane: Option<String>,
    duration: Option<Micros>,
) -> Result<TextAdded, String> {
    let template = presets::template(template_id)
        .ok_or_else(|| format!("there is no text template called {template_id}"))?;
    let style = template.title_style();
    add_title(state, at, lane, duration, Some(template.name), |project| {
        let content = content.unwrap_or_else(|| template.sample().to_string());
        (
            style.material(project, Some(content)),
            style.transform(),
            Some(template.animation()),
        )
    })
}

/// The shared body of the styled adds: mint the material, place it, give the
/// segment its transform and animation, all as one step.
fn add_title(
    state: &Arc<AppState>,
    at: Micros,
    lane: Option<String>,
    duration: Option<Micros>,
    label: Option<&str>,
    make: impl FnOnce(
        &crate::modules::project::document::Project,
    ) -> (
        TextMaterial,
        crate::modules::project::document::Transform,
        Option<crate::modules::project::animation::AnimationMaterial>,
    ),
) -> Result<TextAdded, String> {
    let (material_id, placement) = {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (material, transform, animation) = make(project);
        let material_id = material.id.clone();
        let mut placement = edit::insert_command_on(
            project,
            &material_id,
            at,
            duration.unwrap_or(edit::DEFAULT_DURATION),
            lane.as_deref(),
        )?;
        edit::dress_placement(&mut placement, transform, animation, label);
        // Before the command: see `text_add`.
        project.materials.texts.push(material);
        state
            .history
            .write()
            .apply(project, placement.command.clone())?;
        (material_id, placement)
    };
    Ok(TextAdded {
        edit: respond(state)?,
        material_id,
        segment_id: placement.segment_id,
        track_id: placement.track_id,
        start: placement.start,
    })
}

/// Restyle the title on `segment_id` with style `style_id`, keeping its words
/// and size. One undo step.
pub fn text_apply_style(
    state: &Arc<AppState>,
    segment_id: &str,
    style_id: &str,
) -> Result<EditResponse, String> {
    let style = presets::style(style_id)
        .ok_or_else(|| format!("there is no title style called {style_id}"))?;
    let command =
        state.with_project(|project| edit::restyle_command(project, segment_id, &style))??;
    crate::modules::timeline::commands::timeline_apply(state, command)
}

/// Give the title on `segment_id` template `template_id`'s style and
/// animation, keeping its words and size. One undo step.
pub fn text_apply_template(
    state: &Arc<AppState>,
    segment_id: &str,
    template_id: &str,
) -> Result<EditResponse, String> {
    let template = presets::template(template_id)
        .ok_or_else(|| format!("there is no text template called {template_id}"))?;
    let command =
        state.with_project(|project| edit::template_command(project, segment_id, &template))??;
    crate::modules::timeline::commands::timeline_apply(state, command)
}

/// Move the title on `segment_id` to a cell of the 3 × 3 grid and align its
/// paragraph to match. One undo step.
pub fn text_set_position(
    state: &Arc<AppState>,
    segment_id: &str,
    position: presets::TextPosition,
) -> Result<EditResponse, String> {
    let command =
        state.with_project(|project| edit::position_command(project, segment_id, position))??;
    crate::modules::timeline::commands::timeline_apply(state, command)
}

/// The asset panel's tile of a title style, drawn by the compositor and
/// cached as a PNG. Blocking and GPU-bound.
pub fn text_style_tile(style_id: &str, size: (u32, u32)) -> Result<std::path::PathBuf, String> {
    presets::style_tile(style_id, size)
}

/// The asset panel's tile of a text template, caught mid-entrance.
/// Blocking and GPU-bound.
pub fn text_template_tile(
    template_id: &str,
    size: (u32, u32),
) -> Result<std::path::PathBuf, String> {
    presets::template_tile(template_id, size)
}

/// Change the words of a title, as one undo step.
///
/// Unlike [`text_set`], this goes through the history as
/// `EditCommand::SetTextMaterial`: it is what a finished edit sends (the
/// timeline's inline editor commits once, on Enter or a click elsewhere), not
/// what each keystroke sends. Only the content changes; the style stays.
pub fn text_set_content(
    state: &Arc<AppState>,
    material_id: &str,
    content: &str,
) -> Result<EditResponse, String> {
    let before = state
        .with_project(|p| p.materials.text(material_id).cloned())?
        .ok_or_else(|| format!("no title with the id {material_id}"))?;
    let mut after = before.clone();
    after.content = content.to_string();
    crate::modules::timeline::commands::timeline_apply(
        state,
        crate::modules::timeline::ops::EditCommand::SetTextMaterial { before, after },
    )
}

/// Copy a title's material under a fresh id and return that id.
///
/// What a pasted or duplicated title needs: two clips naming one material
/// would make editing one title's words edit the other's. The material goes
/// into the pool directly, for the reason [`text_add`] gives — a material
/// nothing references is inert — and the clip that names it is inserted
/// through `History::apply` by the caller, so the paste itself undoes.
pub fn text_duplicate(state: &Arc<AppState>, material_id: String) -> Result<String, String> {
    let mut guard = state.project.write();
    let project = guard.as_mut().ok_or("no project is open")?;
    let mut copy = project
        .materials
        .texts
        .iter()
        .find(|m| m.id == material_id)
        .cloned()
        .ok_or_else(|| format!("no title with the id {material_id}"))?;
    copy.id = crate::modules::project::new_id();
    let id = copy.id.clone();
    project.materials.texts.push(copy);
    Ok(id)
}

/// The reply to an edit, with the working copy written on the way out.
///
/// A copy of `timeline::commands::respond`, which is private to that module.
/// Duplicated rather than exported because the two will diverge the moment
/// `EditCommand::SetTextMaterial` exists and `text_set` starts going through
/// the history like everything else.
fn respond(state: &AppState) -> Result<EditResponse, String> {
    let project = state.project.read().clone().ok_or("no project is open")?;
    let origin = state.project_path.read().clone();
    crate::modules::project::autosave::schedule(&project, origin);

    let history = state.history.read();
    Ok(EditResponse {
        project,
        can_undo: history.can_undo(),
        can_redo: history.can_redo(),
        undo_label: history.undo_label(),
        redo_label: history.redo_label(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, Project};

    /// Two titles at the same time on two lanes: the second goes on the lane
    /// it names instead of moving to the next gap, and the styled adds take a
    /// length of their own.
    #[test]
    fn a_title_lands_on_the_named_lane_for_the_asked_length() {
        use crate::modules::project::document::{Track, TrackKind};
        use crate::modules::timeline::ops::EditCommand;
        let state = AppState::new();
        *state.project.write() = Some(Project::new("t", CanvasConfig::default(), 30.0));
        let first = text_add(&state, 0, Some("Headline".into()), None).expect("first");

        let lane = Track::new(TrackKind::Text, "Text 2");
        let lane_id = lane.id.clone();
        let index = state.with_project(|p| p.tracks.len()).unwrap();
        crate::modules::timeline::commands::timeline_apply(
            &state,
            EditCommand::AddTrack { track: lane, index },
        )
        .expect("lane");
        let second = text_add_on(
            &state,
            0,
            Some("Subtitle".into()),
            Some(2_000_000),
            Some(lane_id.clone()),
        )
        .expect("second");
        assert_eq!(second.track_id, lane_id);
        assert_ne!(second.track_id, first.track_id);
        assert_eq!(second.start, 0, "not pushed to the next gap");

        let styled = text_add_style_for(
            &state,
            0,
            "lower-third",
            None,
            Some(lane_id.clone()),
            Some(5_000_000),
        )
        .expect("styled");
        let templated =
            text_add_template_for(&state, 9_000_000, "neon-sign", None, None, Some(1_500_000))
                .expect("templated");
        let length = |id: &str| {
            state
                .with_project(|p| p.segment(id).map(|(_, s)| s.target_range.duration))
                .unwrap()
                .unwrap()
        };
        assert_eq!(length(&second.segment_id), 2_000_000);
        assert_eq!(length(&styled.segment_id), 5_000_000);
        assert_eq!(length(&templated.segment_id), 1_500_000);
    }

    /// A title's new words are one undo step, and undo brings the old ones
    /// back with the style untouched.
    #[test]
    fn setting_a_titles_content_is_one_undo_step() {
        let state = AppState::new();
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut original = super::edit::default_material(&project, Some("Hello".into()));
        original.font_size = 77.0;
        let id = original.id.clone();
        project.materials.texts.push(original);
        *state.project.write() = Some(project);

        let content = |state: &Arc<AppState>| {
            state
                .with_project(|p| p.materials.text(&id).cloned())
                .unwrap()
                .unwrap()
        };
        let response = text_set_content(&state, &id, "Hello\nworld").expect("set");
        assert!(response.can_undo);
        assert_eq!(content(&state).content, "Hello\nworld");
        assert_eq!(content(&state).font_size, 77.0);

        crate::modules::timeline::commands::timeline_undo(&state).expect("undo");
        assert_eq!(content(&state).content, "Hello");
        assert!(!state.history.read().can_undo());
        assert!(text_set_content(&state, "nope", "x").is_err());
    }

    fn open_project() -> Arc<AppState> {
        let state = AppState::new();
        *state.project.write() = Some(Project::new("t", CanvasConfig::default(), 30.0));
        state
    }

    fn undo_steps(state: &Arc<AppState>) -> usize {
        let mut steps = 0;
        while state.history.read().can_undo() {
            crate::modules::timeline::commands::timeline_undo(state).expect("undo");
            steps += 1;
        }
        steps
    }

    /// A style change through `text_set` is one undo step, and setting the
    /// same values again adds none.
    #[test]
    fn a_style_edit_is_one_undo_step() {
        let state = open_project();
        let added = text_add(&state, 0, Some("Hi".into()), None).expect("added");
        let mut material = state
            .with_project(|p| p.materials.text(&added.material_id).cloned())
            .unwrap()
            .unwrap();
        material.underline = true;
        material.letter_spacing = 4.0;
        material.background = Some([0.0, 0.0, 0.0, 0.5]);
        material.background_radius = 12.0;
        text_set(&state, material.clone()).expect("restyled");
        text_set(&state, material).expect("the same again");
        assert_eq!(undo_steps(&state), 2, "add, then one restyle");
        let mut bad = state
            .with_project(|p| p.materials.text(&added.material_id).cloned())
            .unwrap()
            .unwrap();
        bad.background_radius = f32::NAN;
        assert!(text_set(&state, bad).is_err());
    }

    /// A title in a style lands where the style puts it, and is one step.
    #[test]
    fn a_styled_title_is_added_in_one_step_at_its_place() {
        let state = open_project();
        let added = text_add_style(&state, 1_000_000, "lower-third", None, None).expect("added");
        let (material, transform) = state
            .with_project(|p| {
                let (_, segment) = p.segment(&added.segment_id).unwrap();
                (
                    p.materials.text(&added.material_id).cloned().unwrap(),
                    segment.transform,
                )
            })
            .unwrap();
        assert_eq!(
            material.align,
            crate::modules::project::document::TextAlign::Left
        );
        assert!(material.background.is_some());
        assert!(
            transform.position[1] < -0.3,
            "low in the frame: {:?}",
            transform.position
        );
        assert!(text_add_style(&state, 0, "no-such-style", None, None).is_err());
        assert_eq!(undo_steps(&state), 1);
    }

    /// A template is a styled title with its animation, added and undone as
    /// one step; applying one to a title restyles and animates it in one.
    #[test]
    fn a_template_adds_style_and_animation_as_one_step() {
        let state = open_project();
        let added = text_add_template(&state, 0, "neon-sign", None, None).expect("added");
        let animated = state
            .with_project(|p| {
                let (_, segment) = p.segment(&added.segment_id).unwrap();
                p.materials.animation_of(segment).cloned()
            })
            .unwrap()
            .expect("the clip is animated");
        assert!(animated.text_in.is_some() && animated.combo.is_some());
        assert_eq!(
            state.history.read().undo_label().as_deref(),
            Some("Add Neon sign")
        );

        let plain = text_add(&state, 5_000_000, Some("Plain".into()), None).expect("added");
        text_apply_template(&state, &plain.segment_id, "pop-headline").expect("applied");
        let (material, animation) = state
            .with_project(|p| {
                let (_, segment) = p.segment(&plain.segment_id).unwrap();
                (
                    p.materials.text(&plain.material_id).cloned().unwrap(),
                    p.materials.animation_of(segment).cloned(),
                )
            })
            .unwrap();
        assert_eq!(material.content, "Plain", "the words stay");
        assert!(animation.is_some_and(|a| a.text_in.is_some()));
        assert_eq!(undo_steps(&state), 3);
        assert!(state
            .with_project(|p| p.materials.animations.is_empty())
            .unwrap());
    }

    #[test]
    fn a_style_and_a_position_apply_to_an_existing_title_in_one_step_each() {
        let state = open_project();
        let added = text_add(&state, 0, Some("Mine".into()), None).expect("added");
        text_apply_style(&state, &added.segment_id, "neon-cyan").expect("styled");
        text_set_position(&state, &added.segment_id, presets::TextPosition::TopLeft)
            .expect("moved");
        let (material, transform) = state
            .with_project(|p| {
                let (_, segment) = p.segment(&added.segment_id).unwrap();
                (
                    p.materials.text(&added.material_id).cloned().unwrap(),
                    segment.transform,
                )
            })
            .unwrap();
        assert_eq!(material.content, "Mine");
        assert!(material.shadow.is_some());
        assert_eq!(
            material.align,
            crate::modules::project::document::TextAlign::Left
        );
        assert!(transform.position[1] > 0.5);
        assert!(
            text_set_position(&state, &added.segment_id, presets::TextPosition::TopLeft).is_err(),
            "already there"
        );
        assert_eq!(undo_steps(&state), 3);
    }

    /// A duplicated title is a second, equal material under its own id, so a
    /// pasted title can be edited without editing the one it came from.
    #[test]
    fn duplicating_a_title_mints_an_equal_material_under_a_new_id() {
        let state = AppState::new();
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut original = super::edit::default_material(&project, Some("Hello".into()));
        original.font_size = 77.0;
        let id = original.id.clone();
        project.materials.texts.push(original);
        *state.project.write() = Some(project);

        let copy = text_duplicate(&state, id.clone()).expect("duplicated");
        assert_ne!(copy, id);
        let texts = state.with_project(|p| p.materials.texts.clone()).unwrap();
        assert_eq!(texts.len(), 2);
        let made = texts.iter().find(|m| m.id == copy).expect("in the pool");
        assert_eq!(made.content, "Hello");
        assert_eq!(made.font_size, 77.0);
        assert!(text_duplicate(&state, "nope".into()).is_err());
    }

    /// `TextAdded` is flattened, and the frontend's type says so.
    ///
    /// `src/modules/text/lib/api.ts` declares `interface TextAdded extends
    /// EditResponse`, which is only true if `can_undo` and friends sit at the
    /// top level rather than under an `edit` key. A `#[serde(flatten)]` is easy
    /// to drop while refactoring and nothing else would notice until the undo
    /// button stopped updating after a title was added.
    #[test]
    fn the_add_response_carries_the_edit_response_at_the_top_level() {
        let added = TextAdded {
            edit: EditResponse {
                project: Project::new("t", CanvasConfig::default(), 30.0),
                can_undo: true,
                can_redo: false,
                undo_label: Some("Add title".into()),
                redo_label: None,
            },
            material_id: "m1".into(),
            segment_id: "s1".into(),
            track_id: "t1".into(),
            start: 1_500_000,
        };

        let json: serde_json::Value = serde_json::to_value(&added).expect("serialize");
        let object = json.as_object().expect("an object");

        for key in [
            "project",
            "can_undo",
            "can_redo",
            "undo_label",
            "material_id",
        ] {
            assert!(object.contains_key(key), "{key} is missing from {json}");
        }
        assert!(
            !object.contains_key("edit"),
            "the EditResponse was nested instead of flattened: {json}"
        );
        assert_eq!(object["can_undo"], serde_json::Value::Bool(true));
        assert_eq!(object["start"], serde_json::Value::from(1_500_000));
        assert_eq!(object["segment_id"], serde_json::Value::from("s1"));
    }
}

//! The Adjust tab's colour tools: Auto adjust, Colour match, and Save as
//! preset.
//!
//! The answers come from the engine (`grading::commands`), which decodes a
//! few frames, so both run on a blocking task and the panel shows a busy
//! line meanwhile. What they write is the clip's ordinary grade: the sliders
//! below move to the new values and stay editable, and one Ctrl+Z takes it
//! back.

use chukcut_engine::modules::grading::commands::{self as grading, AutoAdjust, ColourMatch};
use chukcut_engine::shell::spawn_blocking;
use gpui::component::input::{Input, InputState};
use gpui::{AnyElement, Entity};

use super::controls::*;
use super::*;

/// The intensities the Auto section offers, as CapCut's slider stops.
const AMOUNTS: [(&str, f32); 3] = [("Subtle", 0.5), ("Medium", 0.75), ("Full", 1.0)];

/// The colour tools' state between frames.
#[derive(Default)]
pub(crate) struct ColourToolsState {
    /// Index into [`AMOUNTS`]; `None` is "Full".
    amount: Option<usize>,
    /// Whether the reference list is open.
    match_menu: bool,
    /// The clip chosen to match to.
    reference: Option<String>,
    /// Match only the reference's frame under the playhead.
    match_frame: bool,
    /// What is running, for the busy line.
    busy: Option<&'static str>,
    /// The preset name field, while "Save as preset" is open.
    preset_name: Option<Entity<InputState>>,
}

impl ColourToolsState {
    fn amount(&self) -> f32 {
        AMOUNTS[self.amount.unwrap_or(AMOUNTS.len() - 1)].1
    }
}

/// A clip's name for the reference list: its own name, or its file's, with
/// the lane and position `info` uses.
fn clip_label(project: &Project, lane: usize, index: usize, segment: &Segment) -> String {
    let materials = &project.materials;
    let file = materials
        .video(&segment.material_id)
        .map(|v| v.path.as_str())
        .or_else(|| {
            materials
                .image(&segment.material_id)
                .map(|i| i.path.as_str())
        })
        .and_then(|p| std::path::Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip".into());
    // A name the user gave the clip lives in the extras pool
    // (`inspector::edit::rename_clip_command`).
    let name = segment
        .extras
        .iter()
        .find_map(|id| {
            materials
                .extras
                .get(id)
                .and_then(|v| v.get("clip_name"))
                .and_then(|n| n.as_str())
                .map(str::to_string)
        })
        .unwrap_or(file);
    format!("{name}  ·  {lane}:{index}")
}

/// Every picture clip on the open timeline other than `except`.
fn picture_clips(project: &Project, except: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (lane, track) in project.tracks.iter().enumerate() {
        for (index, segment) in track.segments.iter().enumerate() {
            let id = &segment.material_id;
            let picture =
                project.materials.video(id).is_some() || project.materials.image(id).is_some();
            if picture && segment.id != except {
                out.push((
                    segment.id.clone(),
                    clip_label(project, lane, index, segment),
                ));
            }
        }
    }
    out
}

impl Editor {
    /// The "Auto" section at the top of Adjust › Basic.
    pub(super) fn colour_tools(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tools = &self.inspector.colour_tools;
        let busy = tools.busy;
        let picture = {
            let m = &self.project.materials;
            m.video(&segment.material_id).is_some() || m.image(&segment.material_id).is_some()
        };
        let mut rows = Vec::new();
        if !picture {
            rows.push(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Auto adjust and colour match read a video or photo clip.")
                    .into_any_element(),
            );
        }

        // Auto adjust: intensity, then the button.
        let selected = tools.amount.unwrap_or(AMOUNTS.len() - 1);
        let entity = cx.entity().downgrade();
        let amounts = crate::ui::SegmentedTabs::new(
            "auto-amount",
            AMOUNTS.iter().map(|(label, _)| *label),
            selected,
        )
        .on_select(move |index, _, cx| {
            let _ = entity.update(cx, |this, cx| {
                this.inspector.colour_tools.amount = Some(index);
                cx.notify();
            });
        });
        rows.push(label_row("Intensity", div().w(px(220.0)).child(amounts)));
        rows.push(label_row(
            "Auto adjust",
            panel_button(
                "auto-adjust",
                if busy == Some("auto") {
                    "Measuring\u{2026}"
                } else {
                    "Auto adjust"
                },
                false,
                picture && busy.is_none(),
                cx.listener(|this, _, _, cx| this.run_auto_adjust(cx)),
            ),
        ));

        // Colour match: a reference clip, whole or its frame at the playhead.
        let clips = picture_clips(&self.project, &segment.id);
        let reference = tools
            .reference
            .clone()
            .filter(|id| clips.iter().any(|(c, _)| c == id));
        let current = reference
            .as_ref()
            .and_then(|id| clips.iter().find(|(c, _)| c == id))
            .map(|(_, label)| label.clone());
        let open = tools.match_menu;
        let picker = div()
            .id("match-picker")
            .w(px(220.0))
            .h(px(CONTROL_H))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .rounded(px(R_SM))
            .bg(rgb(WELL))
            .border_1()
            .border_color(rgb(if open { ACCENT } else { BORDER }))
            .cursor_pointer()
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(if current.is_some() { TEXT } else { TEXT_DIM }))
            .on_click(cx.listener(|this, _, _, cx| {
                let tools = &mut this.inspector.colour_tools;
                tools.match_menu = !tools.match_menu;
                cx.notify();
            }))
            .child(
                div()
                    .overflow_hidden()
                    .child(current.unwrap_or_else(|| "Choose a clip".into())),
            )
            .child(icon(
                if open { icons::UP } else { icons::DOWN },
                10.0,
                TEXT_DIM,
            ));
        rows.push(label_row("Match to", picker));
        if open {
            let mut list = div()
                .flex()
                .flex_col()
                .p_1()
                .rounded(px(R_SM))
                .bg(rgb(OVERLAY))
                .border_1()
                .border_color(rgb(BORDER));
            if clips.is_empty() {
                list = list.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(TEXT_CAPTION))
                        .text_color(rgb(TEXT_MUTED))
                        .child("Put another video or photo on the timeline to match to."),
                );
            }
            for (i, (id, label)) in clips.iter().enumerate() {
                let id = id.clone();
                let active = reference.as_deref() == Some(id.as_str());
                list = list.child(
                    div()
                        .id(SharedString::from(format!("match-ref-{i}")))
                        .h(px(24.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(R_XS))
                        .cursor_pointer()
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(if active { ACCENT } else { TEXT }))
                        .hover(|style| style.bg(rgb(PANEL_RAISED)))
                        .child(label.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let tools = &mut this.inspector.colour_tools;
                            tools.reference = Some(id.clone());
                            tools.match_menu = false;
                            cx.notify();
                        })),
                );
            }
            rows.push(list.into_any_element());
        }
        let frame = tools.match_frame;
        rows.push(label_row(
            "Frame at playhead",
            check_box(
                "match-frame".into(),
                frame,
                true,
                cx.listener(move |this, _, _, cx| {
                    this.inspector.colour_tools.match_frame = !frame;
                    cx.notify();
                }),
            ),
        ));
        rows.push(label_row(
            "Colour match",
            panel_button(
                "colour-match",
                if busy == Some("match") {
                    "Matching\u{2026}"
                } else {
                    "Match colour"
                },
                false,
                picture && reference.is_some() && busy.is_none(),
                cx.listener(|this, _, _, cx| this.run_colour_match(cx)),
            ),
        ));

        // Save as preset, while open: a name and Save / Cancel.
        if let Some(name) = self.inspector.colour_tools.preset_name.clone() {
            rows.push(label_row(
                "Preset name",
                div().w(px(220.0)).child(Input::new(&name)),
            ));
            rows.push(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(panel_button(
                        "preset-cancel",
                        "Cancel",
                        false,
                        true,
                        cx.listener(|this, _, _, cx| {
                            this.inspector.colour_tools.preset_name = None;
                            cx.notify();
                        }),
                    ))
                    .child(panel_button(
                        "preset-save",
                        "Save preset",
                        true,
                        true,
                        cx.listener(|this, _, _, cx| this.save_grade_preset(cx)),
                    ))
                    .into_any_element(),
            );
        }
        let _ = window;
        Section::new("Auto").render(self.collapsed("Auto"), rows, cx)
    }

    fn run_auto_adjust(&mut self, cx: &mut Context<Self>) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let state = Arc::clone(&self.state);
        let amount = self.inspector.colour_tools.amount();
        self.inspector.colour_tools.busy = Some("auto");
        self.run_colour_task(cx, move || {
            grading::grading_auto_adjust(&state, AutoAdjust { segment_id, amount }).map(|done| {
                let c = done.controls;
                format!(
                    "Auto adjust: exposure {:+.2}, temperature {:+.2}, tint {:+.2}",
                    c.exposure, c.temperature, c.tint
                )
            })
        });
    }

    fn run_colour_match(&mut self, cx: &mut Context<Self>) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let tools = &self.inspector.colour_tools;
        let Some(reference_id) = tools.reference.clone() else {
            return;
        };
        let at = tools.match_frame.then(|| self.clock.position());
        let amount = tools.amount();
        let state = Arc::clone(&self.state);
        self.inspector.colour_tools.busy = Some("match");
        self.run_colour_task(cx, move || {
            grading::grading_match(
                &state,
                ColourMatch {
                    segment_id,
                    reference_id,
                    at,
                    amount,
                },
            )
            .map(|done| {
                format!(
                    "Colour matched: difference {:.0} \u{2192} {:.0}",
                    done.distance_before, done.distance_after
                )
            })
        });
    }

    fn run_colour_task(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        cx.notify();
        let future = spawn_blocking(work);
        cx.spawn(async move |this, cx| {
            let result = future
                .await
                .unwrap_or_else(|_| Err("the colour tool stopped".into()));
            let _ = this.update(cx, |editor, cx| {
                editor.inspector.colour_tools.busy = None;
                editor.refresh(cx);
                match result {
                    Ok(message) => {
                        editor.status = Some(message.into());
                        cx.notify();
                    }
                    Err(error) => editor.report(Err(error), cx),
                }
            });
        })
        .detach();
    }

    /// "Save as preset": open the name field, suggesting the next free
    /// "Preset N".
    pub(super) fn open_save_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let suggestion = grading::grading_preset_name();
        let field = cx.new(|cx| InputState::new(window, cx).default_value(suggestion));
        self.inspector.colour_tools.preset_name = Some(field);
        // The field lives in Basic's "Auto" section: show it.
        self.inspector.sub_tab.insert("Adjust", "Basic");
        self.inspector.collapsed.remove("Auto");
        cx.notify();
    }

    fn save_grade_preset(&mut self, cx: &mut Context<Self>) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let Some(field) = self.inspector.colour_tools.preset_name.clone() else {
            return;
        };
        let name = field.read(cx).value().to_string();
        match grading::grading_save_preset(&self.state, segment_id, name, false) {
            Ok(entry) => {
                self.inspector.colour_tools.preset_name = None;
                // The Filters tab lists presets from disk; make it look again.
                self.assets.library.grade_presets = None;
                self.status = Some(
                    format!(
                        "Saved \u{201c}{}\u{201d} in Filters \u{203a} My presets",
                        entry.name
                    )
                    .into(),
                );
                cx.notify();
            }
            Err(error) => self.report(Err(error), cx),
        }
    }
}

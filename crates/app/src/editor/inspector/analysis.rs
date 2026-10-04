//! The inspector's analysis sections: Stabilise, Scene detection and Auto
//! reframe in the Video tab, Beats in the Audio tab.
//!
//! The slow parts run as engine jobs (`editor/analysis.rs` starts them and
//! shows their progress in the status line); every change of a setting is
//! one engine command and one undo step. Settings are offered as a few
//! presets rather than sliders, because each change is a new entry in the
//! document and a drag would leave dozens behind.

use chukcut_engine::modules::analysis::commands as analysis;
use chukcut_engine::modules::analysis::edits::picture_aspect;
use chukcut_engine::modules::analysis::stabilise::{CROPS, STRENGTHS};
use chukcut_engine::modules::analysis::store::{entry_of, SceneCuts};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::Disableable as _;
use gpui::component::Sizable as _;
use gpui::AnyElement;

use super::controls::*;
use super::*;
use crate::ui::SegmentedTabs;

const STABILISE: &str = "Stabilise";
const SCENES: &str = "Scene detection";
const REFRAME: &str = "Auto reframe";
const BEATS: &str = "Beats";

/// The shapes "Auto reframe" offers, as CapCut's ratio picker does.
const RATIOS: [(&str, (u32, u32)); 4] = [
    ("9:16", (9, 16)),
    ("1:1", (1, 1)),
    ("4:5", (4, 5)),
    ("16:9", (16, 9)),
];

/// "Auto-cut to beat" keeps every n-th beat.
const EVERY: [(&str, usize); 3] = [("Every beat", 1), ("Every 2", 2), ("Every 4", 4)];

fn readout(text: impl Into<SharedString>) -> AnyElement {
    div()
        .font_family(FONT_MONO)
        .text_size(px(TEXT_LABEL))
        .text_color(rgb(TEXT))
        .child(text.into())
        .into_any_element()
}

fn hint(text: impl Into<SharedString>) -> AnyElement {
    div()
        .text_size(px(TEXT_CAPTION))
        .text_color(rgb(TEXT_MUTED))
        .child(text.into())
        .into_any_element()
}

fn buttons(children: Vec<Button>) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap_2()
        .children(children)
        .into_any_element()
}

impl Editor {
    /// The Video tab's analysis sections for the picture `segment`.
    pub(super) fn analysis_video_sections(
        &mut self,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let pool = &self.project.materials;
        let is_video = pool.video(&segment.material_id).is_some();
        let is_compound = pool.sequence(&segment.material_id).is_some();
        let mut sections = Vec::new();
        if is_video {
            sections.push(self.stabilise_section(segment, cx));
            sections.push(self.scenes_section(segment, cx));
        } else if is_compound {
            // Scene changes are found in the compound clip's rendered
            // contents; stabilising one would need its camera, which it does
            // not have.
            sections.push(self.scenes_section(segment, cx));
        } else {
            sections.push(
                Section::missing(STABILISE, "Only a video clip can be stabilised").render(
                    true,
                    Vec::new(),
                    cx,
                ),
            );
        }
        sections.push(self.reframe_section(segment, cx));
        sections
    }

    fn running_row(&self, segment: &Segment, cx: &mut Context<Self>) -> Option<AnyElement> {
        let fraction = self.analysing(&segment.id)?;
        Some(label_row(
            format!("Analysing… {:.0} %", fraction * 100.0),
            Button::new("analysis-cancel")
                .small()
                .outline()
                .label("Cancel")
                .on_click(cx.listener(|this, _, _, cx| this.cancel_analysis(cx))),
        ))
    }

    fn stabilise_section(&mut self, segment: &Segment, cx: &mut Context<Self>) -> AnyElement {
        let carried = analysis::analysis_of(&self.project, &segment.id);
        let settings = carried.stabilise.clone();
        let mut rows = Vec::new();
        if let Some(running) = self.running_row(segment, cx) {
            rows.push(running);
        }
        if let Some(s) = &settings {
            let strength = STRENGTHS
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    (a.1 .1 - s.strength)
                        .abs()
                        .total_cmp(&(b.1 .1 - s.strength).abs())
                })
                .map_or(1, |(i, _)| i);
            let editor = cx.entity().downgrade();
            let id = segment.id.clone();
            rows.push(label_row(
                "Strength",
                SegmentedTabs::new(
                    "stabilise-strength",
                    STRENGTHS.iter().map(|(label, _)| *label),
                    strength,
                )
                .on_select(move |index, _, cx| {
                    let id = id.clone();
                    let _ = editor.update(cx, |this, cx| {
                        let result = analysis::analysis_set_stabilise(
                            &this.state,
                            id,
                            Some(true),
                            Some(STRENGTHS[index].1),
                            None,
                        )
                        .map(|_| ());
                        this.refresh(cx);
                        this.report(result, cx);
                    });
                }),
            ));
            let crop = CROPS
                .iter()
                .position(|(_, c)| match (c, s.crop) {
                    (None, None) => true,
                    (Some(a), Some(b)) => (a - b).abs() < 1e-3,
                    _ => false,
                })
                .unwrap_or(usize::MAX);
            let editor = cx.entity().downgrade();
            let id = segment.id.clone();
            rows.push(label_row(
                "Crop",
                SegmentedTabs::new(
                    "stabilise-crop",
                    CROPS.iter().map(|(label, _)| *label),
                    crop,
                )
                .on_select(move |index, _, cx| {
                    let id = id.clone();
                    let _ = editor.update(cx, |this, cx| {
                        let result = analysis::analysis_set_stabilise(
                            &this.state,
                            id,
                            Some(true),
                            None,
                            Some(CROPS[index].1),
                        )
                        .map(|_| ());
                        this.refresh(cx);
                        this.report(result, cx);
                    });
                }),
            ));
            if !analysis::stabilise_covers(&self.project, &segment.id) {
                rows.push(hint(
                    "The clip now shows more than was analysed; the edges hold still.",
                ));
            }
            let strength = s.strength;
            rows.push(buttons(vec![Button::new("stabilise-again")
                .small()
                .outline()
                .label("Analyse again")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.stabilise(strength, cx)
                }))]));
        } else if self.analysing(&segment.id).is_none() {
            rows.push(hint(
                "Measures the camera shake once, then holds the picture still, zoomed in just enough to hide the moving edges.",
            ));
        }
        let analysed = carried.stabilise.is_some();
        Section {
            checkbox: Some(carried.stabilise.as_ref().is_some_and(|s| s.enabled)),
            on_check: Some(Box::new(move |this: &mut Editor, on, cx| {
                let Some(id) = this.selected.clone() else {
                    return;
                };
                if on && !analysed {
                    this.stabilise(0.6, cx);
                    return;
                }
                let result =
                    analysis::analysis_set_stabilise(&this.state, id, Some(on), None, None)
                        .map(|_| ());
                this.refresh(cx);
                this.report(result, cx);
            })),
            ..Section::new(STABILISE)
        }
        .render(self.collapsed(STABILISE), rows, cx)
    }

    fn scenes_section(&mut self, segment: &Segment, cx: &mut Context<Self>) -> AnyElement {
        let carried = analysis::analysis_of(&self.project, &segment.id);
        let found = carried.scenes.len();
        let marked = entry_of::<SceneCuts>(&self.project, segment).is_some();
        let mut rows = Vec::new();
        rows.push(label_row(
            "Found",
            readout(if marked {
                format!("{found} in this clip")
            } else {
                "not analysed".to_string()
            }),
        ));
        // The primary button is the next step: detecting while the clip is
        // not analysed, splitting once there are cuts to split at.
        let detect = Button::new("scenes-detect")
            .small()
            .label(if marked {
                "Detect again"
            } else {
                "Detect scenes"
            })
            .on_click(cx.listener(|this, _, _, cx| this.detect_scenes(false, cx)));
        let split = Button::new("scenes-split")
            .small()
            .label("Split at scene changes")
            .disabled(marked && found == 0)
            .on_click(cx.listener(|this, _, _, cx| this.split_at_scenes(cx)));
        let mut actions = if marked && found > 0 {
            vec![detect.outline(), split.primary()]
        } else {
            vec![detect.primary(), split.outline()]
        };
        if marked {
            let id = segment.id.clone();
            actions.push(
                Button::new("scenes-clear")
                    .small()
                    .ghost()
                    .label("Clear marks")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let result =
                            analysis::analysis_clear_scenes(&this.state, id.clone()).map(|_| ());
                        this.refresh(cx);
                        this.report(result, cx);
                    })),
            );
        }
        rows.push(buttons(actions));
        Section::new(SCENES).render(self.collapsed(SCENES), rows, cx)
    }

    fn reframe_section(&mut self, segment: &Segment, cx: &mut Context<Self>) -> AnyElement {
        let canvas = self.project.canvas;
        let selected = RATIOS
            .iter()
            .position(|(_, (w, h))| {
                canvas.width as u64 * *h as u64 == canvas.height as u64 * *w as u64
            })
            .unwrap_or(usize::MAX);
        let editor = cx.entity().downgrade();
        let mut rows = vec![label_row(
            "Project",
            SegmentedTabs::new(
                "reframe-ratio",
                RATIOS.iter().map(|(label, _)| *label),
                selected,
            )
            .on_select(move |index, _, cx| {
                let _ = editor.update(cx, |this, cx| this.reframe_project(RATIOS[index].1, cx));
            }),
        )];
        let canvas_aspect = canvas.width as f32 / canvas.height.max(1) as f32;
        let differs = picture_aspect(&self.project, segment)
            .is_some_and(|a| (a / canvas_aspect - 1.0).abs() > 0.02);
        let id = segment.id.clone();
        rows.push(buttons(vec![Button::new("reframe-clip")
            .small()
            .outline()
            .label("Reframe this clip")
            .disabled(!differs)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.reframe(vec![id.clone()], None, cx)
            }))]));
        rows.push(hint(
            "Switches the project's shape and follows the subject of every full-frame clip with position keyframes.",
        ));
        Section::new(REFRAME).render(self.collapsed(REFRAME), rows, cx)
    }

    /// The Audio tab's Beats section for the sound clip `segment`.
    pub(super) fn beats_section(
        &mut self,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let carried = analysis::analysis_of(&self.project, &segment.id);
        let marked = carried.bpm.is_some();
        let mut rows = Vec::new();
        if let Some(running) = self.running_row(segment, cx) {
            rows.push(running);
        }
        if let Some(bpm) = carried.bpm {
            rows.push(label_row(
                "Tempo",
                readout(format!("{bpm:.0} BPM · {} beats", carried.beats.len())),
            ));
            let every = EVERY
                .iter()
                .position(|(_, n)| *n == self.analysis.every.max(1))
                .unwrap_or(0);
            let editor = cx.entity().downgrade();
            rows.push(label_row(
                "Cut",
                SegmentedTabs::new("beats-every", EVERY.iter().map(|(label, _)| *label), every)
                    .on_select(move |index, _, cx| {
                        let _ = editor.update(cx, |this, cx| {
                            this.analysis.every = EVERY[index].1;
                            cx.notify();
                        });
                    }),
            ));
            let range = segment.target_range;
            let targets = move |this: &Editor| -> Vec<String> {
                this.project
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Video && !t.locked)
                    .flat_map(|t| t.segments.iter())
                    .filter(|s| s.target_range.overlaps(&range))
                    .map(|s| s.id.clone())
                    .collect()
            };
            let snap_targets = targets;
            rows.push(buttons(vec![
                Button::new("beats-auto-cut")
                    .small()
                    .primary()
                    .label("Auto-cut to beat")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let ids = targets(this);
                        this.auto_cut_to_beat(ids, cx)
                    })),
                Button::new("beats-snap")
                    .small()
                    .outline()
                    .label("Snap cuts to beats")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let ids = snap_targets(this);
                        this.snap_cuts_to_beats(ids, cx)
                    })),
            ]));
            rows.push(hint(
                "Cuts the video above this music on its beats; snapping rolls existing cuts onto the nearest beat.",
            ));
        } else if self.analysing(&segment.id).is_none() {
            rows.push(hint("Finds the tempo and marks every beat on the clip."));
        }
        let id = segment.id.clone();
        Section {
            checkbox: Some(marked),
            on_check: Some(Box::new(move |this: &mut Editor, on, cx| {
                if on {
                    let started = analysis::analysis_detect_beats(&this.state, id.clone(), None);
                    this.start_analysis_from_inspector(started, cx);
                } else {
                    let result =
                        analysis::analysis_clear_beats(&this.state, id.clone()).map(|_| ());
                    this.refresh(cx);
                    this.report(result, cx);
                }
            })),
            ..Section::new(BEATS)
        }
        .render(self.collapsed(BEATS), rows, cx)
    }
}

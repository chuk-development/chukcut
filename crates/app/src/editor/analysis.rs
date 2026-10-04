//! Analysis in the editor: the clip menu's analysis entries, the running
//! jobs in the status line, and the scene and beat marks drawn on clips.
//!
//! Every action is an engine command in `modules/analysis/commands.rs`; this
//! file only picks the clips, starts the jobs, and shows how far they are.
//! The inspector's Stabilise, Scenes, Auto reframe and Beats sections are in
//! `inspector/analysis.rs`.

use chukcut_engine::modules::analysis::commands::{self as analysis, Reframe, SubjectCue};
use chukcut_engine::modules::project::{Segment, TimeRange};
use gpui::component::menu::PopupMenu;
use gpui::{actions, AnyElement};

use super::*;

actions!(
    chukcut,
    [
        DetectScenes,
        SplitAtScenes,
        StabiliseClip,
        DetectBeats,
        AutoCutToBeat,
        SnapCutsToBeats,
        AutoReframe,
        ReframeVertical,
        ReframeLandscape,
        CancelAnalysis
    ]
);

/// The analysis jobs this window started.
#[derive(Default)]
pub(crate) struct AnalysisUi {
    jobs: Vec<u64>,
    /// "Auto-cut to beat" keeps every n-th beat.
    pub(crate) every: usize,
}

/// What the clip menu's analysis entries may do for the clicked selection.
#[derive(Clone, Copy, Default)]
pub(crate) struct MenuFlags {
    video: bool,
    /// A video or a compound clip: something whose picture can be read.
    picture: bool,
    sound: bool,
    beats: bool,
    can_reframe: bool,
    portrait: bool,
    running: bool,
}

/// The clip menu's analysis entries, appended under the editing ones.
pub(crate) fn analysis_menu(menu: PopupMenu, f: MenuFlags) -> PopupMenu {
    let reframe_label = if f.portrait {
        "Reframe project to 16:9"
    } else {
        "Reframe project to 9:16"
    };
    let reframe: Box<dyn gpui::Action> = if f.portrait {
        Box::new(ReframeLandscape)
    } else {
        Box::new(ReframeVertical)
    };
    let menu = menu
        .separator()
        .menu_with_disabled("Detect scenes", Box::new(DetectScenes), !f.picture)
        .menu_with_disabled(
            "Split at scene changes",
            Box::new(SplitAtScenes),
            !f.picture,
        )
        .menu_with_disabled("Stabilise", Box::new(StabiliseClip), !f.video)
        .menu_with_disabled("Detect beats", Box::new(DetectBeats), !f.sound)
        .menu_with_disabled("Auto-cut to beat", Box::new(AutoCutToBeat), !f.beats)
        .menu_with_disabled("Snap cuts to beats", Box::new(SnapCutsToBeats), !f.beats)
        .menu_with_disabled("Auto reframe", Box::new(AutoReframe), !f.can_reframe)
        .menu(reframe_label, reframe);
    if f.running {
        menu.menu("Cancel analysis", Box::new(CancelAnalysis))
    } else {
        menu
    }
}

/// Scene changes and beats, as extra snap targets for the timeline.
pub(crate) fn snap_points(project: &Project) -> Vec<Micros> {
    if project.materials.extras.is_empty() {
        return Vec::new();
    }
    let mut points = Vec::new();
    for track in &project.tracks {
        for segment in &track.segments {
            if segment.extras.is_empty() {
                continue;
            }
            let carried = analysis::analysis_of(project, &segment.id);
            points.extend(carried.scenes);
            points.extend(carried.beats);
        }
    }
    points
}

impl Editor {
    /// The flags for the clip menu, from the selection.
    pub(crate) fn analysis_flags(&self) -> MenuFlags {
        let primary = self
            .selected
            .as_deref()
            .and_then(|id| self.project.segment(id));
        let pool = &self.project.materials;
        let video = primary.is_some_and(|(_, s)| pool.video(&s.material_id).is_some());
        let compound = primary.is_some_and(|(_, s)| pool.sequence(&s.material_id).is_some());
        let sound = compound
            || primary.is_some_and(|(_, s)| {
                pool.audio(&s.material_id).is_some()
                    || pool.video(&s.material_id).is_some_and(|v| v.has_audio)
            });
        let beats = !self.selection().is_empty() && !snap_points(&self.project).is_empty();
        let canvas = self.project.canvas;
        let canvas_aspect = canvas.width as f32 / canvas.height.max(1) as f32;
        let can_reframe = self.selection().iter().any(|id| {
            self.project
                .segment(id)
                .and_then(|(_, s)| {
                    chukcut_engine::modules::analysis::edits::picture_aspect(&self.project, s)
                })
                .is_some_and(|aspect| (aspect / canvas_aspect - 1.0).abs() > 0.02)
        });
        MenuFlags {
            video,
            picture: video || compound,
            sound,
            beats,
            can_reframe,
            portrait: canvas.height > canvas.width,
            running: !self.analysis.jobs.is_empty(),
        }
    }

    pub(crate) fn analysis_actions(&self, root: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        root.on_action(cx.listener(|this, _: &DetectScenes, _, cx| this.detect_scenes(false, cx)))
            .on_action(cx.listener(|this, _: &SplitAtScenes, _, cx| this.split_at_scenes(cx)))
            .on_action(cx.listener(|this, _: &StabiliseClip, _, cx| this.stabilise(0.6, cx)))
            .on_action(cx.listener(|this, _: &DetectBeats, _, cx| this.detect_beats(cx)))
            .on_action(cx.listener(|this, _: &AutoCutToBeat, _, cx| {
                let ids = this.selection();
                this.auto_cut_to_beat(ids, cx)
            }))
            .on_action(cx.listener(|this, _: &SnapCutsToBeats, _, cx| {
                let ids = this.selection();
                this.snap_cuts_to_beats(ids, cx)
            }))
            .on_action(cx.listener(|this, _: &AutoReframe, _, cx| {
                let ids = this.selection();
                this.reframe(ids, None, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ReframeVertical, _, cx| this.reframe_project((9, 16), cx)),
            )
            .on_action(
                cx.listener(|this, _: &ReframeLandscape, _, cx| this.reframe_project((16, 9), cx)),
            )
            .on_action(cx.listener(|this, _: &CancelAnalysis, _, cx| this.cancel_analysis(cx)))
    }

    /// Track a job the inspector started.
    pub(crate) fn start_analysis_from_inspector(
        &mut self,
        started: Result<u64, String>,
        cx: &mut Context<Self>,
    ) {
        self.start_analysis(started, cx);
    }

    fn start_analysis(&mut self, started: Result<u64, String>, cx: &mut Context<Self>) {
        match started {
            Ok(id) => {
                self.analysis.jobs.push(id);
                self.poll_analysis(cx);
            }
            Err(error) => self.status = Some(error.into()),
        }
        cx.notify();
    }

    pub(crate) fn detect_scenes(&mut self, split: bool, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let request = analysis::DetectScenes {
            segment_id: id,
            sensitivity: 0.5,
            split,
        };
        let started = analysis::analysis_detect_scenes(&self.state, request, None);
        self.start_analysis(started, cx);
    }

    /// Split at the scene changes found already, or find them and split in
    /// the same step.
    pub(crate) fn split_at_scenes(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let known = !analysis::analysis_of(&self.project, &id).scenes.is_empty();
        if known {
            let result = analysis::analysis_split_at_scenes(&self.state, id).map(|_| ());
            self.refresh(cx);
            self.report(result, cx);
        } else {
            self.detect_scenes(true, cx);
        }
    }

    pub(crate) fn stabilise(&mut self, strength: f32, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let started = analysis::analysis_stabilise(&self.state, id, strength, None, None);
        self.start_analysis(started, cx);
    }

    pub(crate) fn detect_beats(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let started = analysis::analysis_detect_beats(&self.state, id, None);
        self.start_analysis(started, cx);
    }

    pub(crate) fn auto_cut_to_beat(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        let every = self.analysis.every.max(1);
        let result = analysis::analysis_auto_cut_to_beat(&self.state, ids, every).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    pub(crate) fn snap_cuts_to_beats(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        // Half a beat at the slowest tempo marked: a cut further than that
        // from a beat is nearer the next one.
        let bpm = self
            .project
            .tracks
            .iter()
            .flat_map(|t| t.segments.iter())
            .filter_map(|s| analysis::analysis_of(&self.project, &s.id).bpm)
            .fold(f32::INFINITY, f32::min);
        let tolerance = if bpm.is_finite() && bpm > 0.0 {
            (30.0 / bpm * 1e6) as Micros
        } else {
            250_000
        };
        let result = analysis::analysis_snap_cuts_to_beats(&self.state, ids, tolerance).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    pub(crate) fn reframe(
        &mut self,
        ids: Vec<String>,
        ratio: Option<(u32, u32)>,
        cx: &mut Context<Self>,
    ) {
        let started = analysis::analysis_reframe(
            &self.state,
            Reframe {
                segment_ids: ids,
                ratio,
                subject: SubjectCue::Auto,
            },
            None,
        );
        self.start_analysis(started, cx);
    }

    /// Switch the project to `ratio` and reframe every clip that filled the
    /// old frame, as one undo step.
    pub(crate) fn reframe_project(&mut self, ratio: (u32, u32), cx: &mut Context<Self>) {
        let ids = analysis::reframe_candidates(&self.project);
        self.reframe(ids, Some(ratio), cx);
    }

    pub(crate) fn cancel_analysis(&mut self, cx: &mut Context<Self>) {
        for id in &self.analysis.jobs {
            analysis::analysis_cancel(*id);
        }
        self.status = Some("Stopping the analysis…".into());
        cx.notify();
    }

    /// Whether an analysis of `kind` is running on `segment_id`.
    pub(crate) fn analysing(&self, segment_id: &str) -> Option<f32> {
        self.analysis.jobs.iter().find_map(|id| {
            analysis::analysis_status(*id)
                .filter(|s| s.segment_id == segment_id && s.finished.is_none())
                .map(|s| s.fraction)
        })
    }

    /// Called from the editor's tick: progress in the status line, and the
    /// new document when a job ends. Returns whether anything changed.
    /// Show an analysis started elsewhere (face landmarks for retouch) in
    /// the status line, and refresh when it ends.
    pub(crate) fn follow_analysis_job(&mut self, id: u64) {
        if !self.analysis.jobs.contains(&id) {
            self.analysis.jobs.push(id);
        }
    }

    pub(crate) fn poll_analysis(&mut self, cx: &mut Context<Self>) -> bool {
        if self.analysis.jobs.is_empty() {
            return false;
        }
        let mut changed = false;
        let mut finished_any = false;
        let mut running = Vec::new();
        let mut line: Option<String> = None;
        for id in std::mem::take(&mut self.analysis.jobs) {
            let Some(status) = analysis::analysis_status(id) else {
                continue;
            };
            match status.finished {
                Some(result) => {
                    analysis::analysis_forget(id);
                    finished_any = true;
                    line = Some(match result {
                        Ok(message) => message,
                        Err(error) => error,
                    });
                }
                None => {
                    running.push(id);
                    if line.is_none() {
                        line = Some(format!(
                            "{}… {:.0} %",
                            status.kind.busy(),
                            status.fraction * 100.0
                        ));
                    }
                }
            }
        }
        self.analysis.jobs = running;
        if finished_any {
            self.refresh(cx);
            changed = true;
        }
        if let Some(line) = line {
            if self.status.as_deref() != Some(line.as_str()) {
                self.status = Some(line.into());
                changed = true;
            }
        }
        changed
    }

    /// Scene changes and beats on a clip in the timeline: a thin line where a
    /// shot changes, a tick on the bottom edge on each beat.
    pub(crate) fn analysis_marks(
        &self,
        segment: &Segment,
        target: TimeRange,
        width: f32,
        height: f32,
        top: f32,
        zoom: f32,
    ) -> Option<AnyElement> {
        if segment.extras.is_empty() || self.project.materials.extras.is_empty() {
            return None;
        }
        let carried = analysis::analysis_of(&self.project, &segment.id);
        if carried.scenes.is_empty() && carried.beats.is_empty() {
            return None;
        }
        let x_of = |t: Micros| (t - target.start) as f32 / 1e6 * zoom;
        let mut layer = div().absolute().left(px(0.0)).top(px(0.0)).size_full();
        for t in carried.scenes {
            let x = x_of(t);
            if x <= 1.0 || x >= width - 1.0 {
                continue;
            }
            layer = layer
                .child(
                    div()
                        .absolute()
                        .left(px(x - 0.5))
                        .top(px(top))
                        .w(px(1.0))
                        .h(px((height - top).max(0.0)))
                        .bg(with_alpha(PLAYHEAD, 0.75)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x - 3.0))
                        .top(px(top))
                        .size(px(6.0))
                        .rounded(px(R_XS))
                        .bg(with_alpha(PLAYHEAD, 0.9)),
                );
        }
        // Beats closer than four pixels apart would be a solid bar.
        let mut last = f32::NEG_INFINITY;
        for t in carried.beats {
            let x = x_of(t);
            if x <= 0.0 || x >= width || x - last < 4.0 {
                continue;
            }
            last = x;
            layer = layer.child(
                div()
                    .absolute()
                    .left(px(x - 1.0))
                    .bottom(px(1.0))
                    .w(px(2.0))
                    .h(px(5.0))
                    .rounded(px(1.0))
                    .bg(rgb(SNAP_LINE)),
            );
        }
        Some(layer.into_any_element())
    }
}

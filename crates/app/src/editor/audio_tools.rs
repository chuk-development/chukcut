//! Audio tools on the timeline: "Duck under speech" in a music clip's menu,
//! and the voiceover record button in the toolbar.
//!
//! Both are engine commands (`modules/audiofx/commands.rs`); this file picks
//! the clip, runs the slow part off the UI thread, and draws the record
//! button's count-in, time and input meter.

use std::sync::Arc;
use std::time::Duration;

use chukcut_engine::modules::audiofx::commands as fx_commands;
use chukcut_engine::modules::audiofx::record::Recorder;
use chukcut_engine::modules::audiofx::{fx_or_default, DuckParams};
use gpui::component::menu::PopupMenu;
use gpui::{actions, AnyElement};

use super::*;
use crate::ui::icons::Glyph;
use crate::ui::IconButton;

actions!(chukcut, [DuckUnderSpeech, RemoveDucking, ToggleVoiceover]);

/// How long the count-in before a take lasts.
const COUNT_IN: Duration = Duration::from_secs(3);

/// A microphone, on the kit's grid and stroke.
const MIC: Glyph = Glyph(
    concat!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">"#,
        r#"<rect x="9" y="3.5" width="6" height="11" rx="3"/><path d="M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v2.5"/>"#,
        "</svg>"
    )
    .as_bytes(),
);

/// A stop square.
const STOP: Glyph = Glyph(
    concat!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="black" stroke="black" stroke-width="1.75" stroke-linejoin="round">"#,
        r#"<rect x="7" y="7" width="10" height="10" rx="1.5"/>"#,
        "</svg>"
    )
    .as_bytes(),
);

/// A take in progress.
struct Take {
    recorder: Recorder,
    /// Where the take goes on the timeline: the playhead when the button was
    /// pressed.
    punch_in: Micros,
    /// Whether the playhead has been started, which happens when the count-in
    /// ends.
    rolling: bool,
}

/// The timeline's audio-tool state, one field on the editor.
#[derive(Default)]
pub(crate) struct AudioToolsUi {
    take: Option<Take>,
    /// A ducking analysis is running.
    ducking: bool,
}

/// What the clip menu's audio entries may do.
#[derive(Clone, Copy, Default)]
pub(crate) struct MenuFlags {
    /// The clicked clip is a sound clip on an audio lane.
    music: bool,
    ducked: bool,
}

/// The clip menu's audio entries, appended under the others.
pub(crate) fn audio_menu(menu: PopupMenu, f: MenuFlags) -> PopupMenu {
    let menu = menu.separator().menu_with_disabled(
        if f.ducked {
            "Duck under speech again"
        } else {
            "Duck under speech"
        },
        Box::new(DuckUnderSpeech),
        !f.music,
    );
    if f.ducked {
        menu.menu("Remove ducking", Box::new(RemoveDucking))
    } else {
        menu
    }
}

impl Editor {
    pub(crate) fn audio_menu_flags(&self) -> MenuFlags {
        let Some((track, segment)) = self
            .selected
            .as_deref()
            .and_then(|id| self.project.segment(id))
        else {
            return MenuFlags::default();
        };
        let pool = &self.project.materials;
        let sound = pool.audio(&segment.material_id).is_some()
            || pool
                .video(&segment.material_id)
                .is_some_and(|v| v.has_audio);
        MenuFlags {
            music: track.kind == TrackKind::Audio && sound && !self.audio_tools.ducking,
            ducked: fx_or_default(&self.project, segment).ducking.is_some(),
        }
    }

    pub(crate) fn audio_tool_actions(&self, root: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        root.on_action(cx.listener(|this, _: &DuckUnderSpeech, _, cx| this.duck_selected(cx)))
            .on_action(cx.listener(|this, _: &RemoveDucking, _, cx| this.unduck_selected(cx)))
            .on_action(cx.listener(|this, _: &ToggleVoiceover, _, cx| this.toggle_voiceover(cx)))
    }

    /// Duck the selected music clip under the speech on the other lanes. The
    /// speech analysis decodes every speaking clip, so it runs off the UI
    /// thread; the edit lands as one undo step.
    fn duck_selected(&mut self, cx: &mut Context<Self>) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let state = Arc::clone(&self.state);
        self.audio_tools.ducking = true;
        self.status = Some("Finding speech…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let never = std::sync::atomic::AtomicBool::new(false);
                    fx_commands::audiofx_duck(&state, segment_id, DuckParams::default(), &never)
                        .map(|_| ())
                })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.audio_tools.ducking = false;
                editor.refresh(cx);
                editor.report(result, cx);
            });
        })
        .detach();
    }

    fn unduck_selected(&mut self, cx: &mut Context<Self>) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let result = fx_commands::audiofx_unduck(&self.state, segment_id).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    // --- voiceover ---------------------------------------------------------------

    pub(crate) fn toggle_voiceover(&mut self, cx: &mut Context<Self>) {
        if self.audio_tools.take.is_some() {
            self.stop_voiceover(cx);
        } else {
            self.start_voiceover(cx);
        }
    }

    /// Open the input and count in; the playhead starts with the take.
    fn start_voiceover(&mut self, cx: &mut Context<Self>) {
        if self.clock.is_playing() {
            self.pause();
        }
        let path = fx_commands::audiofx_take_path(&self.state);
        match Recorder::start(&path, COUNT_IN) {
            Ok(recorder) => {
                self.audio_tools.take = Some(Take {
                    recorder,
                    punch_in: self.clock.position(),
                    rolling: false,
                });
                self.status = Some("Recording a voiceover… press the button again to stop".into());
            }
            Err(error) => self.status = Some(error.into()),
        }
        cx.notify();
    }

    /// Stop, put the take on a new lane at the punch-in, and park the
    /// playhead there to play it back.
    fn stop_voiceover(&mut self, cx: &mut Context<Self>) {
        let Some(take) = self.audio_tools.take.take() else {
            return;
        };
        if self.clock.is_playing() {
            self.pause();
        }
        let punch_in = take.punch_in;
        let result = take
            .recorder
            .stop()
            .and_then(|recorded| fx_commands::audiofx_place_take(&self.state, &recorded, punch_in))
            .map(|_| ());
        self.refresh(cx);
        self.seek(punch_in);
        self.report(result, cx);
    }

    /// Called every tick: start the playhead when the count-in ends, and stop
    /// a take whose device went away. Answers whether to redraw.
    pub(crate) fn poll_voiceover(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(take) = &self.audio_tools.take else {
            return false;
        };
        if take.recorder.is_lost() {
            self.stop_voiceover(cx);
            return true;
        }
        let status = take.recorder.status();
        if !status.counting_in && !take.rolling {
            let punch_in = take.punch_in;
            if let Some(take) = self.audio_tools.take.as_mut() {
                take.rolling = true;
            }
            self.seek(punch_in);
            self.play();
        }
        true
    }

    /// The toolbar's record button: a microphone at rest; while recording a
    /// stop square, the count-in or the time, and the input level.
    pub(crate) fn record_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(take) = &self.audio_tools.take else {
            return IconButton::new("voiceover", MIC)
                .tooltip("Record voiceover at the playhead")
                .on_click(cx.listener(|this, _, _, cx| this.toggle_voiceover(cx)))
                .into_any_element();
        };
        let status = take.recorder.status();
        let label = if status.counting_in {
            // The count-in in whole seconds left: 3, 2, 1.
            let left = COUNT_IN.as_secs_f64() - take.recorder_elapsed_secs();
            format!("{}", left.ceil().max(1.0) as u32)
        } else {
            let s = status.elapsed / 1_000_000;
            format!("{}:{:02}", s / 60, s % 60)
        };
        let db = 20.0 * status.level.max(1e-6).log10();
        let fill = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let meter = div()
            .w(px(36.0))
            .h(px(4.0))
            .rounded(px(2.0))
            .bg(rgb(WELL))
            .child(
                div()
                    .h_full()
                    .rounded(px(2.0))
                    .w(gpui::relative(fill))
                    .bg(rgb(if db > -3.0 { DANGER } else { SUCCESS })),
            );
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                IconButton::new("voiceover", STOP)
                    .tint(DANGER)
                    .toggled(true)
                    .tooltip("Stop recording")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_voiceover(cx))),
            )
            .child(
                div()
                    .min_w(px(28.0))
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(if status.counting_in { WARNING } else { DANGER }))
                    .child(label),
            )
            .child(meter)
            .into_any_element()
    }
}

impl Take {
    /// Seconds since the input opened, for the count-in's countdown. The
    /// recorder counts only written frames, so the count-in is timed here.
    fn recorder_elapsed_secs(&self) -> f64 {
        self.recorder.opened().elapsed().as_secs_f64()
    }
}

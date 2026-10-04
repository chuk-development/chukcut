//! "Preparing N frames": the bakes an opened project starts in the
//! background (`modules::prepare`), as one chip in the title bar with Stop.
//!
//! The engine does the work and keeps the count; this file reads it a few
//! times a second, redraws the preview as frames land, and says once at the
//! end what could not be made.

use std::time::{Duration, Instant};

use chukcut_engine::modules::prepare::commands::{self as prepare, PrepareStatus};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::Sizable as _;
use gpui::AnyElement;

use super::*;

/// How often the status is read: the tick runs every few milliseconds.
const READ_EVERY: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct PrepareUi {
    /// The last status read, while a run is busy.
    status: Option<PrepareStatus>,
    read_at: Option<Instant>,
    /// The run whose end has been reported.
    reported: Option<u64>,
}

impl Editor {
    /// Start preparing the project this editor opened with.
    pub(crate) fn start_preparing(&mut self) {
        prepare::prepare_start(&self.state);
    }

    /// Called from the tick. Answers whether anything on screen changed.
    pub(crate) fn poll_prepare(&mut self) -> bool {
        if self
            .prepare
            .read_at
            .is_some_and(|at| at.elapsed() < READ_EVERY)
        {
            return false;
        }
        self.prepare.read_at = Some(Instant::now());
        let Some(now) = prepare::prepare_status() else {
            return false;
        };
        let before = self.prepare.status.clone();
        let landed = before.as_ref().is_some_and(|b| {
            b.run == now.run
                && (b.frames_done != now.frames_done || b.sounds_done != now.sounds_done)
        });
        if landed {
            // New frames in the cache: render the picture again.
            self.generation += 1;
        }
        if now.finished && self.prepare.reported != Some(now.run) {
            self.prepare.reported = Some(now.run);
            if !now.stopped && !now.failures.is_empty() {
                let n = now.failures.len();
                self.status = Some(
                    format!(
                        "{} could not be prepared: {}",
                        if n == 1 {
                            "One clip".to_string()
                        } else {
                            format!("{n} clips")
                        },
                        now.failures[0]
                    )
                    .into(),
                );
            }
        }
        let shown = now.busy().then_some(now);
        let changed = shown != self.prepare.status || landed;
        self.prepare.status = shown;
        changed
    }

    /// The title bar's chip while a run is busy: what, how far, Stop.
    pub(crate) fn render_prepare_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.prepare.status.as_ref()?;
        let label = format!(
            "{} \u{b7} {:.0} %",
            status.sentence(),
            status.fraction() * 100.0
        );
        let tooltip = status
            .stage
            .clone()
            .unwrap_or_else(|| "Reading the cache".to_string());
        Some(
            div()
                .id("prepare-chip")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .max_w(px(360.0))
                .child(
                    div()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(TEXT_CAPTION + 1.0))
                        .text_color(rgb(TEXT_DIM))
                        .child(label),
                )
                .child(
                    Button::new("prepare-stop")
                        .label("Stop")
                        .xsmall()
                        .ghost()
                        .tooltip(tooltip)
                        .on_click(cx.listener(|this, _, _, cx| {
                            prepare::prepare_stop();
                            this.prepare.status = None;
                            this.prepare.read_at = None;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}

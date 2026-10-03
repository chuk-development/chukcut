//! The export dialog's loudness target, and a meter for the mix as it is.
//!
//! The target is an export override (`ExportOverrides::loudness_target`): the
//! engine measures the finished mix with EBU R128, applies the gain and limits
//! true peaks to −1 dBTP (`modules::loudness`). The meter here answers the
//! question a creator has before choosing — "how loud is it now?" — on
//! demand, because measuring decodes every clip on the timeline.

use std::rc::Rc;
use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::loudness::commands::loudness_measure_mix;
use chukcut_engine::modules::loudness::{Loudness, TARGETS};
use gpui::component::button::Button;
use gpui::component::{Disableable as _, Sizable as _};

use super::settings::ExportChoices;
use super::*;

/// The loudness options as the dialog's dropdown takes them: label, whether
/// it is the current one, and what choosing it writes.
#[allow(clippy::type_complexity)]
pub(super) fn picks(
    current: Option<f32>,
) -> Vec<(SharedString, bool, Rc<dyn Fn(&mut ExportChoices)>)> {
    let mut picks: Vec<(SharedString, bool, Rc<dyn Fn(&mut ExportChoices)>)> = vec![(
        "Off · keep the mix as edited".into(),
        current.is_none(),
        Rc::new(|c: &mut ExportChoices| c.loudness_target = None),
    )];
    for (lufs, label) in TARGETS {
        let lufs = *lufs;
        picks.push((
            (*label).into(),
            current.is_some_and(|c| (c - lufs).abs() < 0.05),
            Rc::new(move |c: &mut ExportChoices| c.loudness_target = Some(lufs)),
        ));
    }
    picks
}

enum Reading {
    Idle,
    Measuring,
    Done(Loudness),
    Failed(String),
}

/// "Mix: −18.3 LUFS · peak −2.1 dBTP", measured when asked.
pub(crate) struct MixMeter {
    state: Arc<AppState>,
    reading: Reading,
}

impl MixMeter {
    pub(crate) fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            reading: Reading::Idle,
        }
    }

    fn measure(&mut self, cx: &mut Context<Self>) {
        if matches!(self.reading, Reading::Measuring) {
            return;
        }
        self.reading = Reading::Measuring;
        let state = Arc::clone(&self.state);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { loudness_measure_mix(&state, &AtomicBool::new(false)) })
                .await;
            let _ = this.update(cx, |meter, cx| {
                meter.reading = match result {
                    Ok(loudness) => Reading::Done(loudness),
                    Err(error) => Reading::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for MixMeter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = match &self.reading {
            Reading::Idle => "Not measured".to_string(),
            Reading::Measuring => "Measuring…".to_string(),
            Reading::Done(l) => match l.integrated {
                Some(i) => format!("{i:.1} LUFS · peak {:.1} dBTP", l.true_peak_db),
                None => "Silent".to_string(),
            },
            Reading::Failed(error) => error.clone(),
        };
        let busy = matches!(self.reading, Reading::Measuring);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(if matches!(self.reading, Reading::Done(_)) {
                        TEXT
                    } else {
                        TEXT_MUTED
                    }))
                    .child(text),
            )
            .child(
                Button::new("export-measure-mix")
                    .xsmall()
                    .outline()
                    .label("Measure")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.measure(cx))),
            )
    }
}

//! fal.ai tools on the selected clip: the Media tab's "AI tools" category.
//!
//! The price comes first: picking an action asks fal's pricing API, and the
//! Run button carries the amount. Running uploads the clip's file, polls the
//! queue with progress, and can be cancelled (fal stops the job and does not
//! bill a cancelled request). The result is new media next to the clip or in
//! its place; the original file is never touched.

use std::sync::Arc;

use chukcut_engine::modules::cloud::providers::fal::{self as engine_fal, MediaFacts};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::{Disableable as _, Sizable as _};

use super::*;

/// The selected clip, when it is a video: its id, its file, what it is.
fn selected_video(editor: &Editor) -> Option<(String, String, MediaFacts)> {
    let id = editor.selected.clone()?;
    let (_, segment) = editor.project.segment(&id)?;
    let video = editor.project.materials.video(&segment.material_id)?;
    let bytes = std::fs::metadata(&video.path).map(|m| m.len()).unwrap_or(0);
    Some((
        id,
        video.path.clone(),
        MediaFacts {
            duration_seconds: video.duration as f64 / 1_000_000.0,
            width: video.width,
            height: video.height,
            fps: video.fps,
            bytes,
        },
    ))
}

fn describe(event: &JobEvent) -> String {
    match event {
        JobEvent::Uploading { sent, total } if *total > 0 && sent < total => {
            format!("Uploading the clip ({} MB)\u{2026}", total / 1_000_000)
        }
        JobEvent::Uploading { .. } => "Uploaded".into(),
        JobEvent::Queued { position: Some(p) } if *p > 0 => {
            format!("Waiting in fal's queue, {p} ahead")
        }
        JobEvent::Queued { .. } => "Waiting for a worker\u{2026}".into(),
        JobEvent::Running { log: Some(log) } => format!("Working: {log}"),
        JobEvent::Running { log: None } => "Working\u{2026}".into(),
        JobEvent::Downloading { bytes } => {
            format!("Downloading the result ({:.1} MB)", *bytes as f64 / 1e6)
        }
        JobEvent::Done => "Done".into(),
    }
}

impl Editor {
    pub(crate) fn render_ai_tools(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.assets.cloud.refresh_accounts();
        let able = self.assets.cloud.able(Capability::Process);
        let Some(account) = self.assets.cloud.chosen_of("fal", &able) else {
            return no_account("fal-empty", "AI tools", "fal.ai", cx);
        };
        let actions = cloud_commands::cloud_fal_actions();
        if actions.is_empty() {
            return hint("No actions are configured.").into_any_element();
        }
        let index = self.assets.cloud.fal_action.min(actions.len() - 1);
        let action = &actions[index];
        let clip = selected_video(self);

        // Price the action for this clip once per (clip, action).
        if let Some((segment, _, facts)) = &clip {
            let key = (segment.clone(), index);
            if self.assets.cloud.fal_estimate_for.as_ref() != Some(&key)
                && self.assets.cloud.fal_run.is_none()
            {
                self.assets.cloud.fal_estimate_for = Some(key.clone());
                self.assets.cloud.fal_estimate = None;
                self.assets.cloud.fal_estimating = true;
                self.assets.cloud.fal_placement = usize::from(action.result == "replace");
                let (id, action_id, facts) =
                    (account.account.id.clone(), action.id.clone(), *facts);
                self.cloud_task(
                    cx,
                    move || {
                        cloud_commands::cloud_fal_estimate(
                            &CloudStore::user(),
                            &id,
                            &action_id,
                            &facts,
                        )
                    },
                    move |editor, result, _| {
                        if editor.assets.cloud.fal_estimate_for.as_ref() == Some(&key) {
                            editor.assets.cloud.fal_estimating = false;
                            editor.assets.cloud.fal_estimate = Some(result);
                        }
                    },
                );
            }
        }

        let cloud = &self.assets.cloud;
        let action_chips = row().children(actions.iter().enumerate().map(|(i, a)| {
            chip(
                SharedString::from(format!("fal-action-{}", a.id)),
                a.label.clone(),
                i == index,
                cx.listener(move |this, _, _, cx| {
                    this.assets.cloud.fal_action = i;
                    cx.notify();
                }),
            )
        }));
        let placement = cloud.fal_placement;
        let placement_chips = row()
            .child(label("Result"))
            .child(chip(
                "fal-place-beside",
                "On a lane above the clip",
                placement == 0,
                cx.listener(|this, _, _, cx| {
                    this.assets.cloud.fal_placement = 0;
                    cx.notify();
                }),
            ))
            .child(chip(
                "fal-place-replace",
                "Replace the clip",
                placement == 1,
                cx.listener(|this, _, _, cx| {
                    this.assets.cloud.fal_placement = 1;
                    cx.notify();
                }),
            ));

        let body = match (&clip, &cloud.fal_run) {
            (_, Some(run)) => {
                let event = run.event.lock().clone();
                column()
                    .gap(px(8.0))
                    .child(div().text_size(px(TEXT_BODY)).child(format!(
                                "{}: {}",
                                engine_fal::action(&run.action)
                                    .map(|a| a.label.as_str())
                                    .unwrap_or("fal.ai"),
                                event
                                    .as_ref()
                                    .map(describe)
                                    .unwrap_or_else(|| "Starting\u{2026}".into())
                            )))
                    .child(
                        row().child(Button::new("fal-cancel").label("Cancel").small().on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(run) = &this.assets.cloud.fal_run {
                                    run.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                                cx.notify();
                            }),
                        )),
                    )
                    .child(hint(
                        "fal stops the job and does not bill a cancelled request.",
                    ))
                    .into_any_element()
            }
            (None, None) => {
                hint("Select a video clip on the timeline to process it.").into_any_element()
            }
            (Some(_), None) => {
                let (cost, ready) = match &cloud.fal_estimate {
                    _ if cloud.fal_estimating => (
                        hint("Asking fal.ai for the price\u{2026}").into_any_element(),
                        false,
                    ),
                    Some(Ok(estimate)) => (
                        column()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .text_color(rgb(TEXT))
                                    .child(format!("Estimated cost: {}", estimate.summary)),
                            )
                            .child(hint(format!(
                                "{}. Billed by fal.ai to your account.",
                                estimate.upload
                            )))
                            .into_any_element(),
                        true,
                    ),
                    Some(Err(error)) => (
                        div()
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(DANGER))
                            .child(error.clone())
                            .into_any_element(),
                        false,
                    ),
                    None => (div().into_any_element(), false),
                };
                let amount = match &cloud.fal_estimate {
                    Some(Ok(e)) => format!("Run for ${:.2}", e.amount),
                    _ => "Run".into(),
                };
                column()
                    .gap(px(8.0))
                    .child(cost)
                    .child(
                        row().child(
                            Button::new("fal-run")
                                .label(amount)
                                .small()
                                .primary()
                                .disabled(!ready)
                                .on_click(cx.listener(|this, _, _, cx| this.run_fal(cx))),
                        ),
                    )
                    .into_any_element()
            }
        };

        div()
            .id("ai-tools")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(
                column()
                    .gap(px(12.0))
                    .pb(px(12.0))
                    .child(
                        row()
                            .child(label("Account"))
                            .child(account_picker("fal", able, &account, cx)),
                    )
                    .child(action_chips)
                    .child(hint(action.description.clone()))
                    .child(placement_chips)
                    .child(body)
                    .child(hint(
                        "The whole source file is uploaded and processed; each fal.ai model has its own licence.",
                    )),
            )
            .into_any_element()
    }

    fn run_fal(&mut self, cx: &mut Context<Self>) {
        let able = self.assets.cloud.able(Capability::Process);
        let Some(account) = self.assets.cloud.chosen_of("fal", &able) else {
            return;
        };
        let Some((segment, path, _)) = selected_video(self) else {
            return;
        };
        let actions = cloud_commands::cloud_fal_actions();
        let Some(action) = actions.get(self.assets.cloud.fal_action) else {
            return;
        };
        let estimate = self.assets.cloud.fal_estimate.clone().and_then(|e| e.ok());
        let event = Arc::new(Mutex::new(None));
        let cancel = Arc::new(AtomicBool::new(false));
        let placement = if self.assets.cloud.fal_placement == 1 {
            Placement::Replace(segment)
        } else {
            Placement::Beside(segment)
        };
        self.assets.cloud.fal_run = Some(FalRun {
            event: Arc::clone(&event),
            cancel: Arc::clone(&cancel),
            action: action.id.clone(),
        });
        let (id, action_id) = (account.account.id.clone(), action.id.clone());
        let sink = Arc::clone(&event);
        self.cloud_task(
            cx,
            move || {
                cloud_commands::cloud_fal_run(
                    &CloudStore::user(),
                    &id,
                    &action_id,
                    std::path::Path::new(&path),
                    estimate.as_ref(),
                    &cloud_commands::generated_root(),
                    &|e| *sink.lock() = Some(e),
                    &cancel,
                )
            },
            move |editor, result, cx| {
                editor.assets.cloud.fal_run = None;
                match result {
                    Ok(path) => editor.cloud_import(path, placement, Vec::new(), cx),
                    Err(error) if error == "cancelled" => {
                        editor.status = Some("Cancelled; fal.ai was asked to stop the job".into())
                    }
                    Err(error) => editor.report(Err(error), cx),
                }
            },
        );
        // Redraw the progress line while the job runs.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let running = this.update(cx, |editor, cx| {
                cx.notify();
                editor.assets.cloud.fal_run.is_some()
            });
            if !matches!(running, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.notify();
    }
}

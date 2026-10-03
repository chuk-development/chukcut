//! The export queue in the app: the list in the export dialog, and the
//! editor's watch on it — the status line while it runs and a notification
//! when an item finishes, whether or not the dialog is open.

use chukcut_engine::modules::export::{QueueEvent, QueueItem, QueueStatus};
use gpui::assets::IconName as Lucide;
use gpui::component::progress::Progress;

use super::dialog::ExportDialog;
use super::*;
use crate::ui::{Badge, IconButton, Tone};

/// The queue as the dialog's right-hand side.
pub(super) fn render_list(items: &[QueueItem], cx: &mut Context<ExportDialog>) -> impl IntoElement {
    let count = items.len();
    let rows = items.iter().enumerate().map(|(index, item)| {
        let (tone, state) = match item.status {
            QueueStatus::Queued => (Tone::Neutral, "Queued".to_string()),
            QueueStatus::Running => (
                Tone::Accent,
                format!(
                    "{:.0} %",
                    item.progress.as_ref().map_or(0.0, |p| p.fraction) * 100.0
                ),
            ),
            QueueStatus::Done => (
                Tone::Success,
                item.bytes
                    .map(super::settings::bytes_label)
                    .unwrap_or_else(|| "Done".into()),
            ),
            QueueStatus::Failed => (Tone::Danger, "Failed".to_string()),
            QueueStatus::Cancelled => (Tone::Warning, "Cancelled".to_string()),
        };
        let detail = match item.status {
            QueueStatus::Failed => item.error.clone().unwrap_or_default(),
            QueueStatus::Done => item.output_path.clone().unwrap_or_default(),
            _ => item.request.output_path.clone(),
        };
        let id = item.id.clone();
        let (up, down, stop, drop_id, reveal) = (
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            item.output_path.clone(),
        );
        let running = item.status == QueueStatus::Running;
        let finished = item.status.is_finished();
        div()
            .id(SharedString::from(format!("export-queue-row-{id}")))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(R_SM))
            .bg(rgb(WELL))
            .border_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(TEXT))
                            .child(item.label.clone()),
                    )
                    .child(Badge::new(state).tone(tone))
                    .child(
                        IconButton::new(SharedString::from(format!("q-up-{id}")), Lucide::ArrowUp)
                            .small()
                            .tooltip("Run earlier")
                            .disabled(finished || running || index == 0)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let _ = export_commands::export_queue_move(
                                    &up,
                                    index.saturating_sub(1),
                                );
                                cx.notify();
                            })),
                    )
                    .child(
                        IconButton::new(
                            SharedString::from(format!("q-down-{id}")),
                            Lucide::ArrowDown,
                        )
                        .small()
                        .tooltip("Run later")
                        .disabled(finished || running || index + 1 >= count)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            let _ = export_commands::export_queue_move(&down, index + 1);
                            cx.notify();
                        })),
                    )
                    .children(
                        reveal
                            .filter(|_| item.status == QueueStatus::Done)
                            .map(|path| {
                                IconButton::new(
                                    SharedString::from(format!("q-reveal-{id}")),
                                    Lucide::FolderOpen,
                                )
                                .small()
                                .tooltip("Show in folder")
                                .on_click(move |_, _, cx| {
                                    cx.reveal_path(std::path::Path::new(&path))
                                })
                            }),
                    )
                    .child(if finished {
                        IconButton::new(SharedString::from(format!("q-remove-{id}")), Lucide::Trash)
                            .small()
                            .tooltip("Remove from the list")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let _ = export_commands::export_queue_remove(&drop_id);
                                cx.notify();
                            }))
                    } else {
                        IconButton::new(
                            SharedString::from(format!("q-cancel-{id}-{running}")),
                            Lucide::X,
                        )
                        .small()
                        .tooltip(if running {
                            "Stop this export"
                        } else {
                            "Skip this export"
                        })
                        .on_click(cx.listener(move |_, _, _, cx| {
                            export_commands::export_queue_cancel(&stop);
                            cx.notify();
                        }))
                    }),
            )
            .when(running, |row| {
                row.child(
                    Progress::new(SharedString::from(format!("q-progress-{}", item.id)))
                        .color(rgb(ACCENT))
                        .value(item.progress.as_ref().map_or(0.0, |p| p.fraction) * 100.0),
                )
            })
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(if item.status == QueueStatus::Failed {
                        DANGER
                    } else {
                        TEXT_MUTED
                    }))
                    .child(super::settings::display_path(std::path::Path::new(&detail))),
            )
    });
    div()
        .id("export-queue")
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .overflow_y_scroll()
        .px(px(20.0))
        .py(px(16.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(TEXT_DISPLAY))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child("Export queue"),
        )
        .child(
            div()
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(TEXT_MUTED))
                .child("One export at a time, top to bottom. The queue keeps running when this dialog is closed."),
        )
        .children(rows)
}

/// What the editor knows about the queue between ticks.
#[derive(Default)]
pub(crate) struct QueueWatch {
    events: Arc<parking_lot::Mutex<Vec<QueueEvent>>>,
    subscribed: bool,
}

impl QueueWatch {
    /// Start listening. The engine calls back on the export thread; the
    /// events wait here for the editor's next tick.
    fn ensure_subscribed(&mut self) {
        if self.subscribed {
            return;
        }
        self.subscribed = true;
        let events = Arc::clone(&self.events);
        export_commands::export_queue_subscribe(Channel::new(move |event: QueueEvent| {
            // Progress floods `Changed`; one pending is enough to redraw.
            let mut pending = events.lock();
            if !(matches!(event, QueueEvent::Changed)
                && pending.iter().any(|e| matches!(e, QueueEvent::Changed)))
            {
                pending.push(event);
            }
            true
        }));
    }
}

impl Editor {
    /// Turn queue events into the status line and notifications. `true`
    /// when something changed on screen.
    pub(crate) fn poll_export_queue(&mut self) -> bool {
        self.export_queue.ensure_subscribed();
        let events: Vec<QueueEvent> = std::mem::take(&mut *self.export_queue.events.lock());
        if events.is_empty() {
            return false;
        }
        for event in events {
            match event {
                QueueEvent::Finished { item } => {
                    let line = finished_line(&item);
                    notify_desktop(&line);
                    self.status = Some(line.into());
                }
                QueueEvent::Changed => {
                    if let Some(line) = running_line(&export_commands::export_queue_list()) {
                        self.status = Some(line.into());
                    }
                }
                QueueEvent::Idle => {}
            }
        }
        true
    }
}

/// "Queue 2 of 3 · TikTok · My project · 45 %".
fn running_line(items: &[QueueItem]) -> Option<String> {
    let active: Vec<&QueueItem> = items.iter().filter(|i| !i.status.is_finished()).collect();
    let running = active.iter().find(|i| i.status == QueueStatus::Running)?;
    let done = items.iter().filter(|i| i.status.is_finished()).count();
    Some(format!(
        "Queue {} of {} · {} · {:.0} %",
        done + 1,
        done + active.len(),
        running.label,
        running.progress.as_ref().map_or(0.0, |p| p.fraction) * 100.0
    ))
}

fn finished_line(item: &QueueItem) -> String {
    match item.status {
        QueueStatus::Done => format!(
            "Exported {} → {}",
            item.label,
            item.output_path.as_deref().unwrap_or("")
        ),
        QueueStatus::Failed => format!(
            "Export failed: {}: {}",
            item.label,
            item.error.as_deref().unwrap_or("unknown error")
        ),
        _ => format!("Export cancelled: {}", item.label),
    }
}

/// A desktop notification, when the session has a notification daemon. A
/// missing `notify-send` or daemon is not an error: the status line already
/// says it.
fn notify_desktop(text: &str) {
    let spawned = std::process::Command::new("notify-send")
        .args(["--app-name=chukcut", "--icon=chukcut", "chukcut", text])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = spawned {
        // Reaped on a thread so a slow daemon never holds the UI.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chukcut_engine::modules::export::{ExportProgress, ExportRequest, ExportStage};

    fn item(label: &str, status: QueueStatus, fraction: f32) -> QueueItem {
        QueueItem {
            id: label.into(),
            label: label.into(),
            project_name: "p".into(),
            request: ExportRequest {
                output_path: "/x.mp4".into(),
                preset_id: None,
                overrides: None,
                hardware: None,
                include_audio: true,
                range: None,
            },
            status,
            progress: Some(ExportProgress {
                job_id: "j".into(),
                stage: ExportStage::Encoding,
                frame: 1,
                total_frames: 2,
                fraction,
                fps: 0.0,
                elapsed_seconds: 0.0,
                remaining_seconds: None,
                output_path: None,
                message: None,
            }),
            output_path: Some("/x.mp4".into()),
            bytes: Some(1),
            elapsed_seconds: None,
            error: Some("no disk".into()),
        }
    }

    #[test]
    fn the_status_line_counts_through_the_queue() {
        let items = [
            item("A", QueueStatus::Done, 1.0),
            item("B", QueueStatus::Running, 0.45),
            item("C", QueueStatus::Queued, 0.0),
        ];
        assert_eq!(
            running_line(&items).as_deref(),
            Some("Queue 2 of 3 · B · 45 %")
        );
        assert_eq!(running_line(&items[..1]), None);
        assert_eq!(
            finished_line(&item("A", QueueStatus::Failed, 0.0)),
            "Export failed: A: no disk"
        );
    }
}

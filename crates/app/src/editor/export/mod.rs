//! Export: CapCut's modal dialog, the job it starts, and the progress line
//! the title bar shows while it runs.

mod dialog;
mod loudness;
mod settings;

use gpui::component::WindowExt as _;

use dialog::ExportDialog;

use super::*;

impl Editor {
    pub(super) fn on_export(&mut self, _: &Export, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("export requested");
        if self.project.duration() <= 0 {
            self.status = Some("Nothing on the timeline to export".into());
            cx.notify();
            return;
        }
        let editor = cx.entity().downgrade();
        let state = Arc::clone(&self.state);
        let project = Arc::clone(&self.project);
        let progress = Arc::clone(&self.export_progress);
        let dialog = cx.new(|cx| ExportDialog::new(editor, state, project, progress, window, cx));
        window.open_dialog(cx, move |surface, _, cx| {
            // A running export cannot be dismissed by a stray click or Esc;
            // its own Cancel button is the way out.
            let busy = dialog.read(cx).busy();
            surface
                .w(px(880.0))
                .p_0()
                .bg(rgb(PANEL))
                .border_color(rgb(BORDER))
                .close_button(!busy)
                .overlay_closable(!busy)
                .keyboard(!busy)
                .child(dialog.clone())
        });
    }
}

/// One line for the status bar from an export progress message.
pub(super) fn export_status(progress: &ExportProgress) -> String {
    match progress.stage {
        ExportStage::Done => format!(
            "Exported {} frames in {:.1} s → {}",
            progress.total_frames,
            progress.elapsed_seconds,
            progress.output_path.as_deref().unwrap_or("")
        ),
        ExportStage::Failed => format!(
            "Export failed: {}",
            progress.message.as_deref().unwrap_or("unknown error")
        ),
        ExportStage::Cancelled => "Export cancelled".into(),
        _ => format!(
            "Exporting {:.0} % · {:.0} fps",
            progress.fraction * 100.0,
            progress.fps
        ),
    }
}

//! Export: the path prompt, the job, and its progress line.

use super::*;

impl Editor {
    pub(super) fn on_export(&mut self, _: &Export, _: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("export requested");
        if self.project.duration() <= 0 {
            self.status = Some("Nothing on the timeline to export".into());
            cx.notify();
            return;
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let videos = home.join("Videos");
        let directory = if videos.is_dir() { videos } else { home };
        let name = format!("{}.mp4", self.project.name);
        let picked = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |editor, cx| editor.dialog_failed(error, cx));
                    return;
                }
                Err(_) => return,
            };
            let _ = this.update(cx, |editor, cx| editor.start_export(path, cx));
        })
        .detach();
    }

    pub(super) fn start_export(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // The GPU encoder when this machine has one that passed its trial
        // encode — NVENC, VAAPI or QSV — and libx264 otherwise.
        let hardware = chukcut_engine::modules::export::hwaccel::detect()
            .into_iter()
            .find(|encoder| encoder.usable && encoder.codec == VideoCodec::H264)
            .map(|encoder| encoder.id);
        let request = ExportRequest {
            output_path: path.to_string_lossy().into_owned(),
            preset_id: None,
            overrides: None,
            hardware,
            include_audio: true,
            range: None,
        };
        let slot = Arc::clone(&self.export_progress);
        let channel = Channel::new(move |progress: ExportProgress| {
            *slot.lock() = Some(progress);
            true
        });
        self.status = Some(
            match export_commands::export_start(&self.state, request, channel) {
                Ok(_) => "Exporting…".into(),
                Err(error) => error.into(),
            },
        );
        cx.notify();
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

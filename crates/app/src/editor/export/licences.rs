//! The licence summary at the top of the export dialog, and the credits file
//! after a good export (`chukcut_engine::modules::cloud::credits`).

use chukcut_engine::modules::cloud::commands as cloud_commands;
use chukcut_engine::modules::cloud::credits::LicenceSummary;
use gpui::AnyElement;

use super::*;

/// The summary block, or nothing when no online media is on the timeline.
pub(super) fn summary(summary: &LicenceSummary) -> Option<AnyElement> {
    if summary.is_empty() {
        return None;
    }
    let tone = if summary.has_warnings() {
        DANGER
    } else if !summary.unknown.is_empty() || summary.wants_credits_file() {
        WARNING
    } else {
        SUCCESS
    };
    let lines = summary.lines().into_iter().map(|line| {
        div()
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(TEXT))
            .child(format!("\u{2022} {line}"))
    });
    Some(
        div()
            .px(px(10.0))
            .py(px(8.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .rounded(px(R_SM))
            .bg(with_alpha(tone, 0.08))
            .border_1()
            .border_color(with_alpha(tone, 0.4))
            .child(
                div()
                    .text_size(px(TEXT_LABEL))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(tone))
                    .child("Licences"),
            )
            .children(lines)
            .into_any_element(),
    )
}

/// After the export: write `<name>.credits.txt` when anything needs credit.
pub(super) fn after_export(state: &Arc<AppState>, video: &std::path::Path) -> Option<PathBuf> {
    match cloud_commands::cloud_write_credits(state, video) {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(%error, "no credits file written");
            None
        }
    }
}

/// The "Credits written to …" line under a finished export.
pub(super) fn credits_line(path: &std::path::Path) -> AnyElement {
    div()
        .text_size(px(TEXT_LABEL))
        .text_color(rgb(TEXT_DIM))
        .child(format!(
            "Credits for the online media written to {}",
            super::settings::display_path(path)
        ))
        .into_any_element()
}

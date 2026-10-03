//! The settings dialog (Ctrl+,).
//!
//! Decision 0004: every change is written through at once, the whole
//! `Settings` struct at a time, and reverted on screen if the write fails.
//! There is no OK button and nothing is batched. A partial write would drop
//! every preference this build does not know about, which is why the engine
//! has no per-field setter.

use std::rc::Rc;

use chukcut_engine::modules::proxy::commands as proxy_commands;
use chukcut_engine::modules::workspace::commands as workspace_commands;
use chukcut_engine::modules::workspace::settings::ProxyPolicy;
use chukcut_engine::modules::workspace::{HardwareCodec, HardwareReport, Settings};
use gpui::assets::IconName as Lucide;
use gpui::component::button::Button;
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::switch::Switch;
use gpui::component::{Icon, Sizable as _, WindowExt as _};
use gpui::{AnyElement, WeakEntity};

use super::preview::PreviewQuality;
use super::*;

/// Canvas presets, shared with the start screen: (label, width, height).
pub(crate) const CANVAS_PRESETS: [(&str, u32, u32); 4] = [
    ("9:16", 1080, 1920),
    ("16:9", 1920, 1080),
    ("1:1", 1080, 1080),
    ("4:5", 1080, 1350),
];

/// Frame rates offered for new projects.
pub(crate) const FRAME_RATES: [f64; 5] = [24.0, 25.0, 30.0, 50.0, 60.0];

const PREVIEW_SCALES: [(&str, f32); 3] = [("Full", 1.0), ("Half", 0.5), ("Quarter", 0.25)];

const LONG_EDGES: [(&str, u32); 6] = [
    ("Automatic", 0),
    ("2160 px", 2160),
    ("1440 px", 1440),
    ("1080 px", 1080),
    ("720 px", 720),
    ("540 px", 540),
];

const GIB: u64 = 1024 * 1024 * 1024;
const CACHE_LIMITS: [(&str, u64); 6] = [
    ("2 GB", 2 * GIB),
    ("4 GB", 4 * GIB),
    ("8 GB", 8 * GIB),
    ("16 GB", 16 * GIB),
    ("32 GB", 32 * GIB),
    ("No limit", 0),
];

const PROXY_POLICIES: [(&str, ProxyPolicy); 3] = [
    ("Automatic", ProxyPolicy::Auto),
    ("Always", ProxyPolicy::Always),
    ("Off", ProxyPolicy::Off),
];

pub(crate) fn quality_for_scale(scale: f32) -> PreviewQuality {
    if scale <= 0.25 {
        PreviewQuality::Quarter
    } else if scale <= 0.5 {
        PreviewQuality::Half
    } else {
        PreviewQuality::Full
    }
}

pub(crate) fn fps_label(fps: f64) -> String {
    if fps.fract() == 0.0 {
        format!("{fps:.0} fps")
    } else {
        format!("{fps:.3} fps")
    }
}

pub(crate) fn bytes_label(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= GIB as f64 {
        format!("{:.1} GB", bytes / GIB as f64)
    } else if bytes >= 1024.0 * 1024.0 {
        format!("{:.0} MB", bytes / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", bytes / 1024.0)
    }
}

/// Open the dialog. `editor` receives every change that applies live.
pub(crate) fn open(editor: Option<WeakEntity<Editor>>, window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|cx| SettingsDialog::new(editor, cx));
    window.open_dialog(cx, move |surface, _, _| {
        surface
            .w(px(720.0))
            .p_0()
            .title(div().px_4().pt_3().child("Settings"))
            .child(dialog.clone())
    });
}

pub(crate) struct SettingsDialog {
    settings: Settings,
    editor: Option<WeakEntity<Editor>>,
    hardware: Option<HardwareReport>,
    cache_bytes: Option<u64>,
    proxy_bytes: Option<u64>,
    log_dir: String,
    log_file: Option<String>,
    /// The last thing that went wrong, shown at the top.
    notice: Option<SharedString>,
}

impl SettingsDialog {
    fn new(editor: Option<WeakEntity<Editor>>, cx: &mut Context<Self>) -> Self {
        let logs = workspace_commands::workspace_log_path();
        // Both probes open devices or walk directories; neither may run on
        // the UI thread. The hardware probe is cached for the process.
        cx.spawn(async move |this, cx| {
            let report = cx
                .background_executor()
                .spawn(async { chukcut_engine::modules::workspace::hardware::report() })
                .await;
            let _ = this.update(cx, |dialog, cx| {
                dialog.hardware = Some(report);
                cx.notify();
            });
        })
        .detach();
        let mut dialog = Self {
            settings: workspace_commands::workspace_settings_get(),
            editor,
            hardware: None,
            cache_bytes: None,
            proxy_bytes: None,
            log_dir: logs.directory,
            log_file: logs.file,
            notice: None,
        };
        dialog.measure_cache(cx);
        dialog
    }

    fn measure_cache(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (cache, proxies) = cx
                .background_executor()
                .spawn(async {
                    (
                        workspace_commands::workspace_cache_size(),
                        proxy_commands::proxy_cache_info().stats.bytes,
                    )
                })
                .await;
            let _ = this.update(cx, |dialog, cx| {
                dialog.cache_bytes = Some(cache);
                dialog.proxy_bytes = Some(proxies);
                cx.notify();
            });
        })
        .detach();
    }

    /// Write through: change, persist, and take it back if the disk refused.
    fn change(&mut self, edit: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let before = self.settings.clone();
        edit(&mut self.settings);
        match workspace_commands::workspace_settings_set(self.settings.clone()) {
            Ok(()) => {
                self.notice = None;
                if let Some(editor) = self.editor.as_ref().and_then(|e| e.upgrade()) {
                    let settings = self.settings.clone();
                    editor.update(cx, |editor, cx| editor.apply_settings(&settings, cx));
                }
            }
            Err(error) => {
                self.settings = before;
                self.notice = Some(format!("Could not save the settings: {error}").into());
            }
        }
        cx.notify();
    }

    fn clear_cache(&mut self, cx: &mut Context<Self>) {
        // A running proxy job would write its output straight back.
        proxy_commands::cancel_all_proxies();
        match workspace_commands::workspace_cache_clear() {
            Ok(()) => self.notice = Some("Cache cleared".into()),
            Err(error) => self.notice = Some(error.into()),
        }
        self.cache_bytes = None;
        self.proxy_bytes = None;
        self.measure_cache(cx);
        cx.notify();
    }

    fn clear_proxies(&mut self, cx: &mut Context<Self>) {
        match proxy_commands::proxy_cache_clear() {
            Ok(()) => self.notice = Some("Proxies cleared".into()),
            Err(error) => self.notice = Some(error.into()),
        }
        self.measure_cache(cx);
        cx.notify();
    }

    /// A dropdown of `options`, with `current` checked, writing through.
    fn picker<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        options: Vec<(SharedString, T)>,
        current: T,
        apply: impl Fn(&mut Settings, T) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = options
            .iter()
            .find(|(_, value)| *value == current)
            .map(|(label, _)| label.clone())
            .unwrap_or_else(|| "Custom".into());
        let this = cx.entity().downgrade();
        let apply = Rc::new(apply);
        Button::new(id)
            .label(label)
            .small()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                options.iter().fold(menu, |menu, (label, value)| {
                    let (this, apply, value) = (this.clone(), Rc::clone(&apply), *value);
                    menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(value == current)
                            .on_click(move |_, _, cx| {
                                let apply = Rc::clone(&apply);
                                let _ = this.update(cx, |dialog, cx| {
                                    dialog.change(|settings| apply(settings, value), cx)
                                });
                            }),
                    )
                })
            })
    }

    fn render_general(&self, cx: &mut Context<Self>) -> AnyElement {
        let canvas = self.picker(
            "settings-canvas",
            CANVAS_PRESETS
                .iter()
                .map(|(label, w, h)| (format!("{label} \u{b7} {w}\u{d7}{h}").into(), (*w, *h)))
                .collect(),
            self.settings.default_canvas,
            |settings, canvas| settings.default_canvas = canvas,
            cx,
        );
        // f64 is not `Eq`; the picker compares the integer millihertz.
        let fps = self.picker(
            "settings-fps",
            FRAME_RATES
                .iter()
                .map(|fps| (fps_label(*fps).into(), (*fps * 1000.0) as u64))
                .collect(),
            (self.settings.default_fps * 1000.0).round() as u64,
            |settings, millis| settings.default_fps = millis as f64 / 1000.0,
            cx,
        );
        section(
            "New projects",
            vec![
                row(
                    "Canvas",
                    Some("The shape the start screen picks first."),
                    canvas,
                ),
                row("Frame rate", None, fps),
            ],
        )
    }

    fn render_playback(&self, cx: &mut Context<Self>) -> AnyElement {
        let scale = self.picker(
            "settings-preview-scale",
            PREVIEW_SCALES
                .iter()
                .map(|(label, scale)| ((*label).into(), (*scale * 100.0) as u32))
                .collect(),
            (self.settings.preview_scale() * 100.0) as u32,
            |settings, percent| settings.preview_scale = percent as f32 / 100.0,
            cx,
        );
        let edge = self.picker(
            "settings-long-edge",
            LONG_EDGES
                .iter()
                .map(|(label, edge)| ((*label).into(), *edge))
                .collect(),
            self.settings.preview_max_edge,
            |settings, edge| settings.preview_max_edge = edge,
            cx,
        );
        let scrub = Switch::new("settings-scrub")
            .checked(self.settings.audio_scrubbing)
            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                let checked = *checked;
                this.change(|settings| settings.audio_scrubbing = checked, cx);
            }));
        section(
            "Preview and playback",
            vec![
                row(
                    "Preview resolution",
                    Some("Relative to the player's size. Lower plays heavy timelines smoothly."),
                    scale,
                ),
                row(
                    "Largest preview edge",
                    Some("Automatic renders at the player's own size, never above the canvas."),
                    edge,
                ),
                row(
                    "Audio scrubbing",
                    Some("Play a moment of sound while dragging or stepping the playhead."),
                    scrub,
                ),
            ],
        )
    }

    fn render_storage(&self, cx: &mut Context<Self>) -> AnyElement {
        let policy = self.picker(
            "settings-proxy-policy",
            PROXY_POLICIES
                .iter()
                .map(|(label, policy)| ((*label).into(), *policy))
                .collect(),
            self.settings.proxy_policy,
            |settings, policy| settings.proxy_policy = policy,
            cx,
        );
        let limit = self.picker(
            "settings-cache-limit",
            CACHE_LIMITS
                .iter()
                .map(|(label, bytes)| ((*label).into(), *bytes))
                .collect(),
            self.settings.cache_limit,
            |settings, bytes| settings.cache_limit = bytes,
            cx,
        );
        let measured =
            |bytes: Option<u64>| bytes.map(bytes_label).unwrap_or_else(|| "\u{2026}".into());
        let proxies = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(dim(measured(self.proxy_bytes)))
            .child(
                Button::new("settings-clear-proxies")
                    .label("Clear")
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.clear_proxies(cx))),
            );
        let cache = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(dim(measured(self.cache_bytes)))
            .child(
                Button::new("settings-clear-cache")
                    .label("Clear")
                    .small()
                    .tooltip("Thumbnails, waveforms, proxies and preview frames. All of it is rebuilt when needed.")
                    .on_click(cx.listener(|this, _, _, cx| this.clear_cache(cx))),
            );
        section(
            "Proxies and cache",
            vec![
                row(
                    "Proxy media",
                    Some("Small stand-ins for footage too heavy to play; the export always uses the originals. The player does not switch to proxies yet."),
                    policy,
                ),
                row("Proxies on disk", None, proxies),
                row("Cache", Some("Safe to clear at any time."), cache),
                row(
                    "Cache limit",
                    Some("Kept as your limit; the cache is not trimmed to it automatically yet."),
                    limit,
                ),
            ],
        )
    }

    fn render_hardware(&self) -> AnyElement {
        let Some(report) = &self.hardware else {
            return section(
                "Hardware",
                vec![row("Probing the GPU\u{2026}", None, div())],
            );
        };
        let mut rows = vec![
            row(
                "Graphics card",
                report.device_type.as_deref(),
                dim(report
                    .gpu
                    .clone()
                    .unwrap_or_else(|| "No device could be opened".into())),
            ),
            row(
                "Render backend",
                None,
                dim(report.backend.clone().unwrap_or_else(|| "\u{2013}".into())),
            ),
            row(
                "Zero-copy decode",
                Some("Decoded frames become textures without a trip through memory."),
                dim(if report.can_import_dmabuf {
                    "Yes"
                } else {
                    "No"
                }),
            ),
        ];
        rows.push(codec_list("Hardware decoding", &report.decoders, true));
        // Encoder labels already name their accelerator.
        rows.push(codec_list("Hardware encoding", &report.encoders, false));
        if report.partial {
            rows.push(row(
                "",
                Some("The GPU could not be opened, so this report is incomplete."),
                div(),
            ));
        }
        section("Hardware", rows)
    }

    fn render_logs(&self) -> AnyElement {
        let folder = PathBuf::from(&self.log_dir);
        let file = self.log_file.clone().map(PathBuf::from);
        let buttons = div()
            .flex()
            .flex_row()
            .gap_2()
            .child(
                Button::new("settings-open-logs")
                    .icon(Lucide::FolderOpen)
                    .label("Open folder")
                    .small()
                    .on_click(move |_, _, cx| cx.open_with_system(&folder)),
            )
            .children(file.map(|file| {
                Button::new("settings-show-log")
                    .label("Show today's log")
                    .small()
                    .on_click(move |_, _, cx| cx.reveal_path(&file))
            }));
        section(
            "Logs",
            vec![row("Log folder", Some(self.log_dir.as_str()), buttons)],
        )
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings-scroll")
            .max_h(px(620.0))
            .overflow_y_scroll()
            .px_4()
            .pb_4()
            .flex()
            .flex_col()
            .gap_4()
            .text_color(rgb(TEXT))
            .children(self.notice.clone().map(|notice| {
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(PANEL_RAISED))
                    .text_sm()
                    .child(notice)
            }))
            .child(self.render_general(cx))
            .child(self.render_playback(cx))
            .child(self.render_storage(cx))
            .child(self.render_hardware())
            .child(self.render_logs())
    }
}

fn dim(text: impl Into<SharedString>) -> gpui::Div {
    div().text_sm().text_color(rgb(TEXT_DIM)).child(text.into())
}

fn section(title: &str, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .pb_1()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(ACCENT))
                .child(title.to_uppercase()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .rounded_md()
                .bg(rgb(PANEL_RAISED))
                .children(rows),
        )
        .into_any_element()
}

fn row(label: &str, hint: Option<&str>, control: impl IntoElement) -> AnyElement {
    div()
        .min_h(px(44.0))
        .px_3()
        .py_2()
        .flex()
        .flex_row()
        .items_center()
        .gap_4()
        .border_b_1()
        .border_color(rgb(PANEL))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_sm().child(label.to_string()))
                .children(hint.map(|hint| {
                    div()
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .child(hint.to_string())
                })),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// One line per codec: usable, refused with the driver's reason, or absent.
fn codec_list(title: &str, codecs: &[HardwareCodec], with_accel: bool) -> AnyElement {
    let lines = codecs.iter().map(|codec| {
        let (icon, color, state) = if codec.usable {
            (Lucide::CircleCheck, ACCENT, "works".to_string())
        } else if codec.available {
            (
                Lucide::CircleX,
                0xe5484d,
                codec.note.clone().unwrap_or_else(|| "refused".into()),
            )
        } else {
            (Lucide::Minus, TEXT_DIM, "not in this build".into())
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .text_xs()
            .child(Icon::new(icon).size(px(13.0)).text_color(rgb(color)))
            .child(div().w(px(190.0)).flex_none().child(if with_accel {
                format!("{} \u{b7} {}", codec.label, codec.accel.to_uppercase())
            } else {
                codec.label.clone()
            }))
            .child(
                div()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(rgb(TEXT_DIM))
                    .child(state),
            )
    });
    div()
        .px_3()
        .py_2()
        .flex()
        .flex_col()
        .gap_1()
        .border_b_1()
        .border_color(rgb(PANEL))
        .child(div().text_sm().pb_1().child(title.to_string()))
        .children(lines)
        .when(codecs.is_empty(), |list| list.child(dim("None reported")))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_scale_maps_onto_the_players_steps() {
        assert_eq!(quality_for_scale(1.0), PreviewQuality::Full);
        assert_eq!(quality_for_scale(0.5), PreviewQuality::Half);
        assert_eq!(quality_for_scale(0.25), PreviewQuality::Quarter);
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(bytes_label(3 * GIB / 2), "1.5 GB");
        assert_eq!(bytes_label(5 * 1024 * 1024), "5 MB");
        assert_eq!(fps_label(30.0), "30 fps");
        assert_eq!(fps_label(29.97), "29.970 fps");
    }
}

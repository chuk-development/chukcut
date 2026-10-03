//! Nothing selected: the project's details, and the small settings form
//! behind "Change" (name, canvas, frame rate), committed with
//! `project_configure` as one undo step.

use chukcut_engine::modules::project::ProjectConfig;
use gpui::component::input::Input;
use gpui::component::scroll::ScrollableElement;
use gpui::component::Sizable;
use gpui::AnyElement;

use super::controls::*;
use super::*;

/// The open settings form: one text field per value.
pub(crate) struct SettingsForm {
    name: Entity<InputState>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    fps: Entity<InputState>,
}

/// CapCut's ratio presets, as (label, width, height) at 1080 on the short
/// edge.
const RATIOS: [(&str, u32, u32); 6] = [
    ("16:9", 1920, 1080),
    ("9:16", 1080, 1920),
    ("1:1", 1080, 1080),
    ("4:3", 1440, 1080),
    ("3:4", 1080, 1440),
    ("21:9", 2520, 1080),
];

const FRAME_RATES: [f64; 7] = [23.976, 24.0, 25.0, 29.97, 30.0, 50.0, 60.0];

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// "9:16" for 1080×1920, the preset's name when it is one.
fn aspect_label(width: u32, height: u32) -> String {
    let d = gcd(width, height).max(1);
    let (w, h) = (width / d, height / d);
    // 2520×1080 reduces to 7:3, which nobody calls it.
    if (w, h) == (7, 3) {
        return "21:9".into();
    }
    format!("{w}:{h}")
}

fn fps_label(fps: f64) -> String {
    if (fps - fps.round()).abs() < 0.001 {
        format!("{fps:.0} fps")
    } else {
        format!("{fps:.3} fps")
    }
}

impl Editor {
    pub(super) fn render_details(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = crate::ui::PanelHeader::new().title(if self.inspector.settings.is_some() {
            "Project settings"
        } else {
            "Details"
        });

        let (body, footer) = if self.inspector.settings.is_some() {
            (self.settings_body(cx), self.settings_footer(cx))
        } else {
            (self.details_body(), self.details_footer(window, cx))
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(header)
            .child(
                div()
                    .id("details-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scrollbar()
                    .child(body),
            )
            .child(panel_footer(footer))
            .into_any_element()
    }

    fn details_body(&self) -> AnyElement {
        let row = |label: &'static str, value: String| {
            div()
                .min_h(px(ROW_H))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(16.0))
                .child(
                    div()
                        .w(px(140.0))
                        .flex_none()
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(TEXT_DIM))
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(TEXT))
                        .child(value),
                )
        };
        let path = self
            .state
            .project_path
            .read()
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "Not saved yet".into());
        let canvas = self.project.canvas;
        let imported = self.project.materials.videos.len()
            + self.project.materials.images.len()
            + self.project.materials.audios.len();

        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(PAD))
            .py(px(8.0))
            .child(row("Name", self.project.name.clone()))
            .child(row("Path", path))
            .child(row("Colour space", "Rec.709 SDR".into()))
            .child(row(
                "Imported media",
                format!("Kept in place ({imported} files)"),
            ))
            .child(row(
                "Proxy",
                "Automatic, for footage too heavy to play".into(),
            ))
            .child(div().my(px(8.0)).h(px(1.0)).bg(rgb(HAIRLINE)))
            .child(row("Timeline name", "Timeline 01".into()))
            .child(row(
                "Aspect ratio",
                aspect_label(canvas.width, canvas.height),
            ))
            .child(row(
                "Resolution",
                format!("{}×{}", canvas.width, canvas.height),
            ))
            .child(row("Frame rate", fps_label(self.project.fps)))
            .child(row(
                "Duration",
                timecode(self.project.duration(), self.project.fps),
            ))
            .into_any_element()
    }

    fn details_footer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _ = window;
        panel_button(
            "details-change",
            "Change",
            false,
            true,
            cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
        )
        .into_any_element()
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = Arc::clone(&self.project);
        let field = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).default_value(value))
        };
        let form = SettingsForm {
            name: field(project.name.clone(), window, cx),
            width: field(project.canvas.width.to_string(), window, cx),
            height: field(project.canvas.height.to_string(), window, cx),
            fps: field(format!("{}", project.fps), window, cx),
        };
        self.inspector.settings = Some(form);
        cx.notify();
    }

    fn settings_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.inspector.settings else {
            return div().into_any_element();
        };
        let text = |state: &Entity<InputState>, width: f32| {
            div()
                .w(px(width))
                .h(px(CONTROL_H))
                .flex()
                .items_center()
                .rounded(px(R_SM))
                .bg(rgb(WELL))
                .border_1()
                .border_color(rgb(BORDER))
                .child(
                    Input::new(state)
                        .appearance(false)
                        .xsmall()
                        .text_size(px(TEXT_LABEL)),
                )
        };
        let presets = |items: Vec<(String, AnyElement)>| {
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .children(items.into_iter().map(|(_, element)| element))
        };
        let ratio_buttons: Vec<(String, AnyElement)> = RATIOS
            .iter()
            .enumerate()
            .map(|(i, &(label, w, h))| {
                let width = form.width.clone();
                let height = form.height.clone();
                (
                    label.to_string(),
                    preset_chip(
                        ("ratio", i),
                        label,
                        cx.listener(move |_, _, window, cx| {
                            width.update(cx, |s, cx| s.set_value(w.to_string(), window, cx));
                            height.update(cx, |s, cx| s.set_value(h.to_string(), window, cx));
                        }),
                    )
                    .into_any_element(),
                )
            })
            .collect();
        let fps_buttons: Vec<(String, AnyElement)> = FRAME_RATES
            .iter()
            .enumerate()
            .map(|(i, &fps)| {
                let input = form.fps.clone();
                let label = if fps.fract() == 0.0 {
                    format!("{fps:.0}")
                } else {
                    format!("{fps}")
                };
                (
                    label.clone(),
                    preset_chip(
                        ("fps", i),
                        label,
                        cx.listener(move |_, _, window, cx| {
                            input.update(cx, |s, cx| s.set_value(format!("{fps}"), window, cx));
                        }),
                    )
                    .into_any_element(),
                )
            })
            .collect();

        let row = |label: &'static str, right: AnyElement| {
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(16.0))
                .py(px(6.0))
                .child(
                    div()
                        .w(px(120.0))
                        .pt(px(6.0))
                        .flex_none()
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(TEXT_DIM))
                        .child(label),
                )
                .child(div().flex_1().child(right))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .px(px(PAD))
            .py(px(8.0))
            .child(row("Name", text(&form.name, 300.0).into_any_element()))
            .child(row(
                "Aspect ratio",
                presets(ratio_buttons).into_any_element(),
            ))
            .child(row(
                "Resolution",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(text(&form.width, 90.0))
                    .child(
                        div()
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(TEXT_MUTED))
                            .child("×"),
                    )
                    .child(text(&form.height, 90.0))
                    .into_any_element(),
            ))
            .child(row(
                "Frame rate",
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(text(&form.fps, 90.0))
                    .child(presets(fps_buttons))
                    .into_any_element(),
            ))
            .child(
                div()
                    .pt_3()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child(
                        "Changing the frame rate moves nothing: clips keep their times, \
                         only playback and export change.",
                    ),
            )
            .into_any_element()
    }

    fn settings_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(panel_button(
                "settings-cancel",
                "Cancel",
                false,
                true,
                cx.listener(|this, _, window, cx| {
                    this.inspector.settings = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }),
            ))
            .child(panel_button(
                "settings-save",
                "Save",
                true,
                true,
                cx.listener(|this, _, window, cx| this.save_settings(window, cx)),
            ))
            .into_any_element()
    }

    fn save_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.inspector.settings else {
            return;
        };
        let read = |state: &Entity<InputState>| state.read(cx).value().trim().to_string();
        let number = |text: String, what: &str| -> Result<f64, String> {
            text.replace(',', ".")
                .parse::<f64>()
                .map_err(|_| format!("{what} must be a number"))
        };
        let config = (|| -> Result<ProjectConfig, String> {
            Ok(ProjectConfig {
                name: read(&form.name),
                width: number(read(&form.width), "the width")?.round() as u32,
                height: number(read(&form.height), "the height")?.round() as u32,
                fps: number(read(&form.fps), "the frame rate")?,
                background: self.project.canvas.background,
            })
        })();
        let result = config
            .and_then(|config| project_commands::project_configure(&self.state, config))
            .map(|_| ());
        if result.is_ok() {
            self.inspector.settings = None;
            window.focus(&self.focus, cx);
        }
        self.refresh(cx);
        self.report(result, cx);
    }
}

fn preset_chip(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .h(px(24.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .rounded(px(R_SM))
        .bg(rgb(PANEL_RAISED))
        .border_1()
        .border_color(rgb(HAIRLINE))
        .text_size(px(TEXT_LABEL))
        .font_family(FONT_MONO)
        .text_color(rgb(TEXT_DIM))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(OVERLAY)).text_color(rgb(TEXT)))
        .on_click(on_click)
        .child(label.into())
}

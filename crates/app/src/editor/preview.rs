//! The player in the middle, laid out like CapCut's: a header with the
//! timeline's name and a menu, the canvas, and a transport bar with the time
//! on the left, play in the centre and the view controls on the right.

use chukcut_engine::modules::project::ProjectConfig;
use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::Sizable as _;

use super::*;
use crate::ui::{icons, IconButton, Panel, PanelHeader};

/// How many pixels the preview renders, relative to what fits the viewer.
/// Lower is cheaper; playback of heavy timelines stays smooth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PreviewQuality {
    #[default]
    Full,
    Half,
    Quarter,
}

impl PreviewQuality {
    const ALL: [PreviewQuality; 3] = [
        PreviewQuality::Full,
        PreviewQuality::Half,
        PreviewQuality::Quarter,
    ];

    fn label(self) -> &'static str {
        match self {
            PreviewQuality::Full => "Full",
            PreviewQuality::Half => "Half",
            PreviewQuality::Quarter => "Quarter",
        }
    }

    pub(crate) fn scale(self) -> f32 {
        match self {
            PreviewQuality::Full => 1.0,
            PreviewQuality::Half => 0.5,
            PreviewQuality::Quarter => 0.25,
        }
    }
}

/// The player's own state, one field on the editor.
#[derive(Default)]
pub(crate) struct PreviewState {
    pub(crate) quality: PreviewQuality,
}

/// Canvas shapes the ratio menu offers, as (label, long, short) where the
/// short side keeps the canvas's current short side.
const RATIOS: [(&str, u32, u32); 6] = [
    ("16:9", 16, 9),
    ("9:16", 9, 16),
    ("1:1", 1, 1),
    ("4:3", 4, 3),
    ("3:4", 3, 4),
    ("21:9", 21, 9),
];

/// The canvas for a ratio, keeping the current short side.
fn canvas_for_ratio(width: u32, height: u32, (w, h): (u32, u32)) -> (u32, u32) {
    let short = width.min(height).max(2);
    let even = |value: f64| ((value.round() as u32) + 1) & !1;
    if w >= h {
        (even(short as f64 * w as f64 / h as f64), short & !1)
    } else {
        (short & !1, even(short as f64 * h as f64 / w as f64))
    }
}

/// `00:00:00:00` — hours, minutes, seconds, frames, as CapCut writes it.
fn long_timecode(time: Micros, fps: f64) -> String {
    let total = time.max(0) as f64 / 1_000_000.0;
    let whole = total.floor() as i64;
    let frames = if fps > 0.0 {
        (total.fract() * fps).floor() as i64
    } else {
        0
    };
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        whole / 3600,
        whole / 60 % 60,
        whole % 60,
        frames
    )
}

impl Editor {
    pub(super) fn render_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let viewer = Rc::clone(&self.viewer);
        let (cw, ch) = (
            self.project.canvas.width as f32,
            self.project.canvas.height as f32,
        );
        let bounds = self.viewer.get();
        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let fit = if cw > 0.0 && ch > 0.0 {
            (bw / cw).min(bh / ch)
        } else {
            0.0
        };
        let (dw, dh) = (cw * fit, ch * fit);

        let picture = match (&self.frame, self.player.failure()) {
            (_, Some(failure)) => div()
                .max_w(px(360.0))
                .text_center()
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(DANGER))
                .child(failure)
                .into_any_element(),
            (Some(frame), None) => div()
                .w(px(dw))
                .h(px(dh))
                .child(frame.element())
                .into_any_element(),
            (None, None) => div().w(px(dw)).h(px(dh)).bg(rgb(VIEWER)).into_any_element(),
        };

        let playing = self.clock.is_playing();
        let position = self.clock.position();
        let fps = self.project.fps;
        let quality = self.preview.quality;

        let editor = cx.entity().downgrade();
        let header_menu = IconButton::new("player-menu", Lucide::Ellipsis)
            .tooltip("Player options")
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |menu, _, _| {
                let snapshot = editor.clone();
                menu.min_w(px(200.0))
                    .item(
                        PopupMenuItem::new("Save frame as image…").on_click(move |_, _, cx| {
                            let _ = snapshot.update(cx, |editor, cx| editor.save_frame(cx));
                        }),
                    )
            });

        let editor = cx.entity().downgrade();
        let quality_menu = Button::new("player-quality")
            .label(quality.label())
            .ghost()
            .xsmall()
            .dropdown_caret(true)
            .tooltip("Preview quality")
            .dropdown_menu_with_anchor(gpui::Anchor::BottomRight, move |menu, _, _| {
                PreviewQuality::ALL.into_iter().fold(menu, |menu, each| {
                    let editor = editor.clone();
                    menu.item(
                        PopupMenuItem::new(each.label())
                            .checked(each == quality)
                            .on_click(move |_, _, cx| {
                                let _ = editor.update(cx, |editor, cx| {
                                    editor.preview.quality = each;
                                    // A new size is a new request.
                                    editor.last_request = None;
                                    cx.notify();
                                });
                            }),
                    )
                })
            });

        let editor = cx.entity().downgrade();
        let (canvas_w, canvas_h) = (self.project.canvas.width, self.project.canvas.height);
        let ratio_menu = IconButton::new("player-ratio", icons::RATIO)
            .tooltip("Canvas aspect ratio")
            .dropdown_menu_with_anchor(gpui::Anchor::BottomRight, move |menu, _, _| {
                RATIOS.into_iter().fold(menu, |menu, (label, w, h)| {
                    let editor = editor.clone();
                    let (width, height) = canvas_for_ratio(canvas_w, canvas_h, (w, h));
                    let current = (width, height) == (canvas_w, canvas_h);
                    menu.item(PopupMenuItem::new(label).checked(current).on_click(
                        move |_, _, cx| {
                            let _ = editor
                                .update(cx, |editor, cx| editor.set_canvas(width, height, cx));
                        },
                    ))
                })
            });

        let timecode = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .font_family(FONT_MONO)
            .text_size(px(TEXT_LABEL))
            .child(
                div()
                    .text_color(rgb(TEXT))
                    .child(long_timecode(position, fps)),
            )
            .child(div().text_color(rgb(TEXT_DISABLED)).child("/"))
            .child(
                div()
                    .text_color(rgb(TEXT_MUTED))
                    .child(long_timecode(self.project.duration(), fps)),
            );

        let transport = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                IconButton::new("player-step-back", icons::STEP_BACK)
                    .tooltip("Previous frame")
                    .shortcut("left")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.step(-1);
                        cx.notify();
                    })),
            )
            .child(
                // The state is in the id: a tooltip open while playback
                // starts or stops belongs to the old element and closes,
                // instead of keeping "Play" over a pause button.
                IconButton::new(
                    if playing {
                        "player-pause"
                    } else {
                        "player-play"
                    },
                    if playing { icons::PAUSE } else { icons::PLAY },
                )
                .large()
                .tint(TEXT)
                .tooltip(if playing { "Pause" } else { "Play" })
                .shortcut("space")
                .on_click(cx.listener(|this, _, w, cx| this.on_play_pause(&PlayPause, w, cx))),
            )
            .child(
                IconButton::new("player-step-forward", icons::STEP_FORWARD)
                    .tooltip("Next frame")
                    .shortcut("right")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.step(1);
                        cx.notify();
                    })),
            );

        Panel::new("player")
            .flex_1()
            .header(
                PanelHeader::new()
                    .title("Player")
                    .detail(self.project.sequence.name.clone())
                    .action(header_menu),
            )
            .child(
                div()
                    .flex_1()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .m(px(PAD))
                    .child(
                        canvas(move |bounds, _, _| viewer.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        // A hairline frame so a black picture still shows
                        // where the canvas ends.
                        div()
                            .relative()
                            .border_1()
                            .border_color(rgb(HAIRLINE))
                            .child(picture)
                            .children(self.motion_overlay(dw, dh, cx))
                            .children(self.mask_overlay(dw, dh, cx))
                            .children(self.caption_overlay((dw, dh), (bw, bh), cx))
                            .child(self.render_tracking_overlay(cx)),
                    ),
            )
            .child(
                div()
                    .h(px(48.0))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px(px(PAD))
                    .border_t_1()
                    .border_color(rgb(HAIRLINE))
                    // Equal flexible sides keep the transport centred on the
                    // bar while there is room, and let it push the sides
                    // (which clip) rather than overlap them when there is not.
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .child(timecode),
                    )
                    .child(transport)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap(px(2.0))
                            .child(quality_menu)
                            .child(
                                IconButton::new("player-fit", icons::FIT)
                                    .tooltip("Zoom to fit")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        // The canvas always fits the viewer;
                                        // re-render at the current size.
                                        this.last_request = None;
                                        cx.notify();
                                    })),
                            )
                            .child(ratio_menu)
                            .child(
                                IconButton::new("player-fullscreen", icons::FULLSCREEN)
                                    .tooltip("Full screen")
                                    .on_click(|_, window, _| window.toggle_fullscreen()),
                            ),
                    ),
            )
    }

    /// Ratio menu: a new canvas shape, as one undoable project edit.
    fn set_canvas(&mut self, width: u32, height: u32, cx: &mut Context<Self>) {
        let config = ProjectConfig {
            width,
            height,
            // A ratio picked from the menu is a choice, even the one in use.
            canvas_chosen: true,
            ..ProjectConfig::of(&self.project)
        };
        let result = project_commands::project_configure(&self.state, config).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    /// Player menu → Save frame: the frame at the playhead, full size, as PNG.
    fn save_frame(&mut self, cx: &mut Context<Self>) {
        let time = self.clock.position() + chukcut_engine::modules::project::SAMPLE_SLACK;
        let name = files::suggested_name(&format!("{} frame", self.project.name), "png");
        let request = FileRequest::save("Save frame", Filter::Png, name)
            .starting_in(files::home().join("Pictures"));
        let picked = files::choose_one(request, cx);
        let state = Arc::clone(&self.state);
        cx.spawn(async move |this, cx| {
            let Some(path) = picked.await else {
                return;
            };
            let result =
                export_commands::export_snapshot(&state, time, path.to_string_lossy().to_string())
                    .await;
            let _ = this.update(cx, |editor, cx| {
                editor.status = Some(match result {
                    Ok(path) => format!("Saved frame {path}").into(),
                    Err(error) => error.into(),
                });
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ratio_keeps_the_short_side() {
        assert_eq!(canvas_for_ratio(1080, 1920, (16, 9)), (1920, 1080));
        assert_eq!(canvas_for_ratio(1920, 1080, (9, 16)), (1080, 1920));
        assert_eq!(canvas_for_ratio(1080, 1920, (1, 1)), (1080, 1080));
        assert_eq!(canvas_for_ratio(1920, 1080, (4, 3)), (1440, 1080));
        assert_eq!(canvas_for_ratio(1920, 1080, (21, 9)), (2520, 1080));
    }

    #[test]
    fn the_timecode_has_hours_and_frames() {
        assert_eq!(long_timecode(133_966_667, 30.0), "00:02:13:29");
        assert_eq!(long_timecode(0, 30.0), "00:00:00:00");
        assert_eq!(long_timecode(3_600_000_000, 25.0), "01:00:00:00");
    }
}

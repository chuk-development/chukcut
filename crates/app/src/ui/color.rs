//! A colour picker: a saturation–value field, a hue strip, an optional
//! opacity strip, a hex field, our preset colours and the colours used last.
//!
//! The picker is a view (`Entity<ColorPicker>`) because it holds a drag and
//! a text field; [`color_button`] is the swatch that opens it in a popover.
//! Its owner subscribes to [`ColorEvent`]: `Preview` while a strip or the
//! field is dragged — show it, do not write it — and `Commit` once, on
//! release, on a swatch or on Enter in the hex field, which is the one undo
//! step. The owner keeps the picker in step with its document through
//! [`ColorPicker::sync`].
//!
//! Colours are straight sRGB, `[r, g, b, a]` in 0..1, which is what titles
//! store; a caller with linear values (effect parameters) converts at its
//! edge with [`srgb_to_linear`] and [`linear_to_srgb`].

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Mutex;

use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::popover::Popover;
use gpui::component::{Disableable as _, Sizable as _};
use gpui::prelude::*;
use gpui::{
    canvas, div, hsla, linear_color_stop, linear_gradient, px, rgb, App, Bounds, Context,
    DispatchPhase, Entity, EventEmitter, Focusable as _, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, Subscription, Window,
};

use crate::theme::*;

/// What the picker tells its owner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ColorEvent {
    /// Being dragged: show it, do not store it.
    Preview([f32; 4]),
    /// Chosen: store it, as one step.
    Commit([f32; 4]),
}

/// Our preset colours: the ones titles and captions are made with most.
/// Values the user picks, not chrome, so they are not theme tokens.
pub(crate) const PRESETS: [u32; 16] = [
    0xffffff, 0xd9d9d9, 0x8c8c8c, 0x000000, 0xffd400, 0xff9500, 0xff3b30, 0xff2bd6, 0xaf52de,
    0x3478f6, 0x19d3ff, 0x2fd5c8, 0x34c759, 0x9ef01a, 0x8b5a2b, 0xf3e2c0,
];

/// How many recent colours are kept and shown.
const RECENT_MAX: usize = 8;

const WIDTH: f32 = 232.0;
const FIELD_H: f32 = 132.0;
const STRIP_H: f32 = 12.0;
const SWATCH: f32 = 20.0;

/// The colours committed last, newest first, as RGBA hex. One list for every
/// picker in the process, kept in a small file under the data directory so
/// it outlives a restart.
static RECENT: Mutex<Option<Vec<u32>>> = Mutex::new(None);

fn recent_path() -> std::path::PathBuf {
    chukcut_engine::modules::workspace::paths::data_root().join("recent-colours.txt")
}

pub(crate) fn recent_colors() -> Vec<u32> {
    let mut guard = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(|| {
            std::fs::read_to_string(recent_path())
                .unwrap_or_default()
                .lines()
                .filter_map(|line| u32::from_str_radix(line.trim(), 16).ok())
                .take(RECENT_MAX)
                .collect()
        })
        .clone()
}

fn remember(color: [f32; 4]) {
    let value = to_rgba_u32(color);
    let mut list = recent_colors();
    list.retain(|c| *c != value);
    list.insert(0, value);
    list.truncate(RECENT_MAX);
    let text: String = list.iter().map(|c| format!("{c:08x}\n")).collect();
    // Best effort: a read-only data directory loses the list at exit, which
    // is no reason to fail a colour pick.
    let path = recent_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
    *RECENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(list);
}

// --- colour arithmetic ----------------------------------------------------------

/// `[r, g, b, a]` as `0xRRGGBBAA`.
pub(crate) fn to_rgba_u32(color: [f32; 4]) -> u32 {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (c(color[0]) << 24) | (c(color[1]) << 16) | (c(color[2]) << 8) | c(color[3])
}

pub(crate) fn from_rgba_u32(value: u32) -> [f32; 4] {
    let c = |shift: u32| ((value >> shift) & 0xff) as f32 / 255.0;
    [c(24), c(16), c(8), c(0)]
}

/// `0xRRGGBB` and an alpha as a colour.
pub(crate) fn from_rgb_u32(value: u32, alpha: f32) -> [f32; 4] {
    let mut color = from_rgba_u32(value << 8);
    color[3] = alpha;
    color
}

/// `#RRGGBB`, with `AA` appended when the colour is not opaque.
pub(crate) fn to_hex(color: [f32; 4]) -> String {
    let value = to_rgba_u32(color);
    if value & 0xff == 0xff {
        format!("#{:06X}", value >> 8)
    } else {
        format!("#{value:08X}")
    }
}

/// `#RGB`, `#RRGGBB` or `#RRGGBBAA`, with or without the `#`. A colour
/// without alpha keeps `alpha`.
pub(crate) fn parse_hex(text: &str, alpha: f32) -> Option<[f32; 4]> {
    let digits = text.trim().trim_start_matches('#');
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok();
    match digits.len() {
        3 => {
            let v = value?;
            let expand = |n: u32| (n & 0xf) * 0x11;
            let rgb = (expand(v >> 8) << 16) | (expand(v >> 4) << 8) | expand(v);
            Some(from_rgb_u32(rgb, alpha))
        }
        6 => Some(from_rgb_u32(value?, alpha)),
        8 => Some(from_rgba_u32(value?)),
        _ => None,
    }
}

/// Hue, saturation and value, each 0..1.
pub(crate) fn rgb_to_hsv(color: [f32; 4]) -> (f32, f32, f32) {
    let [r, g, b, _] = color;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue, saturation, max)
}

pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32, alpha: f32) -> [f32; 4] {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m, alpha]
}

/// One sRGB channel to linear light.
pub(crate) fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// One linear-light channel to sRGB.
pub(crate) fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn same(a: [f32; 4], b: [f32; 4]) -> bool {
    to_rgba_u32(a) == to_rgba_u32(b)
}

fn gpui_color(color: [f32; 4]) -> gpui::Hsla {
    gpui::Rgba {
        r: color[0],
        g: color[1],
        b: color[2],
        a: color[3],
    }
    .into()
}

// --- the view ---------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Area {
    Field,
    Hue,
    Alpha,
}

pub(crate) struct ColorPicker {
    /// Whether the popover is open; [`color_button`] reads it.
    pub(crate) open: bool,
    color: [f32; 4],
    /// Kept apart from `color`, so a grey or black does not forget the hue
    /// the field was showing.
    hue: f32,
    with_alpha: bool,
    hex: Entity<InputState>,
    drag: Option<Area>,
    bounds: [Rc<Cell<Bounds<Pixels>>>; 3],
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ColorEvent> for ColorPicker {}

impl ColorPicker {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hex = cx.new(|cx| InputState::new(window, cx).placeholder("#FFFFFF"));
        let subscription = cx.subscribe_in(
            &hex,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    let text = input.read(cx).value().to_string();
                    match parse_hex(&text, this.color[3]) {
                        Some(color) => this.choose(color, cx),
                        // Unparseable text snaps back to the colour.
                        None => this.show_hex(window, cx),
                    }
                }
                _ => {}
            },
        );
        Self {
            open: false,
            color: [1.0, 1.0, 1.0, 1.0],
            hue: 0.0,
            with_alpha: false,
            hex,
            drag: None,
            bounds: Default::default(),
            _subscriptions: vec![subscription],
        }
    }

    /// Offer an opacity strip. Off by default: a text fill has its own
    /// opacity row, a shadow or a box does not.
    pub(crate) fn with_alpha(mut self, with_alpha: bool) -> Self {
        self.with_alpha = with_alpha;
        self
    }

    pub(crate) fn color(&self) -> [f32; 4] {
        self.color
    }

    /// Show `color`, unless the user is dragging or typing in the picker.
    pub(crate) fn sync(&mut self, color: [f32; 4], window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_some() || same(color, self.color) {
            return;
        }
        self.set(color);
        self.show_hex(window, cx);
    }

    fn set(&mut self, color: [f32; 4]) {
        let (h, s, _) = rgb_to_hsv(color);
        if s > 0.0 {
            self.hue = h;
        }
        self.color = color;
    }

    fn show_hex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = to_hex(self.color);
        let focused = self.hex.read(cx).focus_handle(cx).is_focused(window);
        if !focused && self.hex.read(cx).value().as_ref() != text {
            self.hex
                .update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }

    /// A finished choice: stored by the owner, remembered as recent.
    fn choose(&mut self, color: [f32; 4], cx: &mut Context<Self>) {
        self.set(color);
        remember(color);
        cx.emit(ColorEvent::Commit(color));
        cx.notify();
    }

    fn press(&mut self, area: Area, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.drag = Some(area);
        self.pointer(position, cx);
    }

    fn pointer(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(area) = self.drag else {
            return;
        };
        let bounds = self.bounds[area as usize].get();
        let w = f32::from(bounds.size.width).max(1.0);
        let h = f32::from(bounds.size.height).max(1.0);
        let x = ((f32::from(position.x) - f32::from(bounds.origin.x)) / w).clamp(0.0, 1.0);
        let y = ((f32::from(position.y) - f32::from(bounds.origin.y)) / h).clamp(0.0, 1.0);
        let (_, s, v) = rgb_to_hsv(self.color);
        let alpha = self.color[3];
        let color = match area {
            Area::Field => hsv_to_rgb(self.hue, x, 1.0 - y, alpha),
            Area::Hue => {
                self.hue = x.min(0.9999);
                hsv_to_rgb(self.hue, s, v, alpha)
            }
            Area::Alpha => {
                let mut c = self.color;
                c[3] = (x * 100.0).round() / 100.0;
                c
            }
        };
        self.color = color;
        cx.emit(ColorEvent::Preview(color));
        cx.notify();
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            let color = self.color;
            remember(color);
            cx.emit(ColorEvent::Commit(color));
            cx.notify();
        }
    }

    /// The press, move and release handlers of one draggable area, plus the
    /// canvas that records where it is.
    fn tracker(&self, area: Area, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = Rc::clone(&self.bounds[area as usize]);
        let entity = cx.entity().downgrade();
        canvas(
            move |b, _, _| bounds.set(b),
            move |_, _, window, _| {
                // Registered on every paint, as the curve editor does: a
                // release that arrives before the next frame must still end
                // the drag. Without a drag the handlers do nothing.
                let up = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = entity.update(cx, |this, cx| {
                            if this.drag == Some(area) {
                                this.pointer(event.position, cx)
                            }
                        });
                    }
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = up.update(cx, |this, cx| {
                            if this.drag == Some(area) {
                                this.release(cx)
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    fn area(&self, area: Area, height: f32, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id(SharedString::from(format!("colour-{area:?}")))
            .relative()
            .w(px(WIDTH))
            .h(px(height))
            .rounded(px(R_SM))
            .overflow_hidden()
            .border_1()
            .border_color(rgb(BORDER))
            .cursor_crosshair()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.press(area, event.position, cx)
                }),
            )
    }

    fn render_field(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (_, s, v) = rgb_to_hsv(self.color);
        let white = hsla(0.0, 0.0, 1.0, 1.0);
        let black = hsla(0.0, 0.0, 0.0, 1.0);
        let clear = |c: gpui::Hsla| gpui::Hsla { a: 0.0, ..c };
        let inner_w = WIDTH - 2.0;
        let inner_h = FIELD_H - 2.0;
        self.area(Area::Field, FIELD_H, cx)
            .bg(hsla(self.hue, 1.0, 0.5, 1.0))
            .child(div().absolute().size_full().bg(linear_gradient(
                90.0,
                linear_color_stop(white, 0.0),
                linear_color_stop(clear(white), 1.0),
            )))
            .child(div().absolute().size_full().bg(linear_gradient(
                180.0,
                linear_color_stop(clear(black), 0.0),
                linear_color_stop(black, 1.0),
            )))
            .child(
                div()
                    .absolute()
                    .left(px(s * inner_w - 6.0))
                    .top(px((1.0 - v) * inner_h - 6.0))
                    .size(px(12.0))
                    .rounded_full()
                    .border_2()
                    .border_color(rgb(0xffffff))
                    .shadow_sm(),
            )
            .child(self.tracker(Area::Field, cx))
    }

    fn render_hue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let stops = (0..6).map(|i| {
            let from = hsla(i as f32 / 6.0, 1.0, 0.5, 1.0);
            let to = hsla(((i + 1) as f32 / 6.0).min(0.9999), 1.0, 0.5, 1.0);
            div().flex_1().h_full().bg(linear_gradient(
                90.0,
                linear_color_stop(from, 0.0),
                linear_color_stop(to, 1.0),
            ))
        });
        self.area(Area::Hue, STRIP_H, cx)
            .flex()
            .flex_row()
            .children(stops)
            .child(marker(self.hue * (WIDTH - 2.0)))
            .child(self.tracker(Area::Hue, cx))
    }

    fn render_alpha(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let opaque = gpui_color([self.color[0], self.color[1], self.color[2], 1.0]);
        let clear = gpui::Hsla { a: 0.0, ..opaque };
        self.area(Area::Alpha, STRIP_H, cx)
            .child(checker(STRIP_H - 2.0, WIDTH - 2.0))
            .child(div().absolute().size_full().bg(linear_gradient(
                90.0,
                linear_color_stop(clear, 0.0),
                linear_color_stop(opaque, 1.0),
            )))
            .child(marker(self.color[3] * (WIDTH - 2.0)))
            .child(self.tracker(Area::Alpha, cx))
    }

    fn swatch_row(
        &self,
        id: &'static str,
        colors: Vec<[f32; 4]>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div().flex().flex_row().flex_wrap().gap(px(5.0)).children(
            colors.into_iter().enumerate().map(|(i, color)| {
                let chosen = same(color, self.color);
                div()
                    .id((id, i))
                    .size(px(SWATCH))
                    .rounded(px(R_XS))
                    .overflow_hidden()
                    .relative()
                    .border_1()
                    .border_color(rgb(if chosen { ACCENT } else { BORDER }))
                    .cursor_pointer()
                    .hover(|s| s.border_color(rgb(TEXT_DIM)))
                    .when(color[3] < 1.0, |s| {
                        s.child(checker(SWATCH - 2.0, SWATCH - 2.0))
                    })
                    .child(div().absolute().size_full().bg(gpui_color(color)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose(color, cx);
                        this.show_hex(window, cx);
                    }))
            }),
        )
    }
}

/// The thin handle on a strip.
fn marker(x: f32) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(px((x - 2.0).max(0.0)))
        .w(px(4.0))
        .rounded(px(2.0))
        .border_1()
        .border_color(rgb(0x000000))
        .bg(rgb(0xffffff))
}

/// A two-row checkerboard, the backdrop of anything see-through.
pub(crate) fn checker(height: f32, width: f32) -> impl IntoElement {
    let cell = (height / 2.0).max(2.0);
    let columns = (width / cell).ceil() as usize;
    let row = move |offset: usize| {
        div().flex().flex_row().children((0..columns).map(move |i| {
            div()
                .size(px(cell))
                .flex_none()
                .bg(rgb(if (i + offset).is_multiple_of(2) {
                    0x9a9a9a
                } else {
                    0xdcdcdc
                }))
        }))
    };
    div()
        .absolute()
        .top_0()
        .left_0()
        .flex()
        .flex_col()
        .child(row(0))
        .child(row(1))
}

impl Render for ColorPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.hex.read(cx).focus_handle(cx).is_focused(window) {
            self.show_hex(window, cx);
        }
        let label = |text: &'static str| {
            div()
                .text_size(px(TEXT_CAPTION))
                .text_color(rgb(TEXT_MUTED))
                .child(text)
        };
        let recent: Vec<[f32; 4]> = recent_colors().into_iter().map(from_rgba_u32).collect();
        let presets: Vec<[f32; 4]> = PRESETS.iter().map(|&c| from_rgb_u32(c, 1.0)).collect();
        div()
            .id("colour-picker")
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(self.render_field(cx))
            .child(self.render_hue(cx))
            .when(self.with_alpha, |this| this.child(self.render_alpha(cx)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .relative()
                            .size(px(26.0))
                            .flex_none()
                            .rounded(px(R_SM))
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(BORDER))
                            .child(checker(24.0, 24.0))
                            .child(div().absolute().size_full().bg(gpui_color(self.color))),
                    )
                    .child(
                        div().flex_1().child(
                            Input::new(&self.hex)
                                .small()
                                .text_size(px(TEXT_LABEL))
                                .font_family(FONT_MONO)
                                .bg(rgb(WELL))
                                .border_color(rgb(BORDER)),
                        ),
                    )
                    .when(self.with_alpha, |this| {
                        this.child(
                            div()
                                .w(px(40.0))
                                .text_right()
                                .text_size(px(TEXT_LABEL))
                                .font_family(FONT_MONO)
                                .text_color(rgb(TEXT_DIM))
                                .child(format!("{:.0}%", self.color[3] * 100.0)),
                        )
                    }),
            )
            .child(label("Colours"))
            .child(self.swatch_row("colour-preset", presets, cx))
            .when(!recent.is_empty(), |this| {
                this.child(label("Recent"))
                    .child(self.swatch_row("colour-recent", recent, cx))
            })
    }
}

/// The swatch that shows `picker`'s colour and opens it. `label` is drawn
/// after the swatch (the hex value, or "None" for a colour that is off).
pub(crate) fn color_button(
    id: impl Into<SharedString>,
    picker: &Entity<ColorPicker>,
    label: Option<SharedString>,
    enabled: bool,
    cx: &App,
) -> impl IntoElement {
    let id: SharedString = id.into();
    let state = picker.read(cx);
    let open = state.open;
    let color = state.color;
    let text = label.unwrap_or_else(|| to_hex(color).into());
    let content = picker.clone();
    let toggle = picker.clone();
    let swatch = div()
        .relative()
        .size(px(16.0))
        .flex_none()
        .rounded(px(R_XS))
        .overflow_hidden()
        .border_1()
        .border_color(rgb(BORDER_STRONG))
        .child(checker(14.0, 14.0))
        .child(div().absolute().size_full().bg(gpui_color(color)));
    let trigger = gpui::component::button::Button::new(SharedString::from(format!("{id}-button")))
        .small()
        .outline()
        .disabled(!enabled)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(swatch)
                .child(
                    div()
                        .text_size(px(TEXT_LABEL))
                        .font_family(FONT_MONO)
                        .text_color(rgb(if enabled { TEXT } else { TEXT_DISABLED }))
                        .child(text),
                ),
        );
    Popover::new(id)
        .anchor(gpui::Anchor::TopRight)
        .open(open && enabled)
        .on_open_change(move |open, _, cx| {
            toggle.update(cx, |picker, cx| {
                picker.open = *open;
                cx.notify();
            });
            cx.refresh_windows();
        })
        .trigger(trigger)
        .content(move |_, _, _| content.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_reads_every_spelling_and_writes_alpha_only_when_needed() {
        assert_eq!(parse_hex("#ff0000", 1.0), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_hex("0f0", 0.5), Some([0.0, 1.0, 0.0, 0.5]));
        assert_eq!(
            parse_hex("#0000FF80", 1.0).map(to_rgba_u32),
            Some(0x0000ff80)
        );
        assert_eq!(parse_hex("#12345", 1.0), None);
        assert_eq!(parse_hex("#gg0000", 1.0), None);
        assert_eq!(to_hex([1.0, 1.0, 1.0, 1.0]), "#FFFFFF");
        assert_eq!(to_hex([0.0, 0.0, 0.0, 0.5]), "#00000080");
    }

    #[test]
    fn hsv_round_trips_through_rgb() {
        for value in [
            0xff0000u32,
            0x2fd5c8,
            0x123456,
            0x808080,
            0xffd400,
            0x000000,
        ] {
            let color = from_rgb_u32(value, 1.0);
            let (h, s, v) = rgb_to_hsv(color);
            assert_eq!(
                to_rgba_u32(hsv_to_rgb(h, s, v, 1.0)),
                to_rgba_u32(color),
                "{value:06x}"
            );
        }
        let (h, s, v) = rgb_to_hsv(from_rgb_u32(0x00ff00, 1.0));
        assert!((h - 1.0 / 3.0).abs() < 1e-4 && s == 1.0 && v == 1.0);
    }

    #[test]
    fn srgb_and_linear_are_inverse() {
        for i in 0..=20 {
            let v = i as f32 / 20.0;
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-4);
        }
    }
}

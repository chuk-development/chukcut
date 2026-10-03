//! Colours and fixed sizes of the editor.
//!
//! Close to CapCut's dark desktop layout — near-black window, #262626 panels,
//! teal video clips, navy audio clips — with our own accent. Everything the
//! app draws itself takes its colours from here; widgets from GPUI Component
//! take theirs from the component theme, which [`apply`] sets to match.

use std::time::Duration;

use gpui::component::theme::{Theme, ThemeMode};
use gpui::{rgb, App, Hsla};

pub(crate) const BG: u32 = 0x141414;
pub(crate) const PANEL: u32 = 0x262626;
pub(crate) const PANEL_RAISED: u32 = 0x313131;
pub(crate) const BORDER: u32 = 0x3b3b3b;
pub(crate) const TEXT: u32 = 0xe6e6e6;
pub(crate) const TEXT_DIM: u32 = 0x8f8f8f;
pub(crate) const ACCENT: u32 = 0x1fc8d6;
pub(crate) const ACCENT_HOVER: u32 = 0x45d6e1;
pub(crate) const PLAYHEAD: u32 = 0xffffff;
pub(crate) const CLIP_VIDEO: u32 = 0x0b4f55;
pub(crate) const CLIP_VIDEO_TITLE: u32 = 0x10666d;
pub(crate) const TRACK_HEADER: u32 = 0x2a2a2a;
pub(crate) const CLIP_IMAGE: u32 = 0x5b4b9a;
pub(crate) const CLIP_AUDIO: u32 = 0x12335d;
pub(crate) const CLIP_AUDIO_WAVE: u32 = 0x3d6fae;
pub(crate) const CLIP_TEXT: u32 = 0x9a6b2f;
pub(crate) const CLIP_OTHER: u32 = 0x55555c;

pub(crate) const MEDIA_W: f32 = 640.0;
pub(crate) const INSPECTOR_W: f32 = 590.0;

/// How often the view checks the clock and the render thread.
pub(crate) const TICK: Duration = Duration::from_millis(8);

fn hsla(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Dark mode, with the component theme's surfaces and accent set to ours.
pub(crate) fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let theme = Theme::global_mut(cx);
    theme.font_family = "Noto Sans".into();
    let c = &mut theme.colors;
    c.background = hsla(BG);
    c.foreground = hsla(TEXT);
    c.muted_foreground = hsla(TEXT_DIM);
    c.border = hsla(BORDER);
    c.primary = hsla(ACCENT);
    c.primary_hover = hsla(ACCENT_HOVER);
    c.primary_active = hsla(ACCENT);
    c.primary_foreground = hsla(0x0b1214);
    c.accent = hsla(PANEL_RAISED);
    c.accent_foreground = hsla(TEXT);
    c.secondary = hsla(PANEL_RAISED);
    c.secondary_hover = hsla(BORDER);
    c.secondary_foreground = hsla(TEXT);
    c.input = hsla(BORDER);
    c.ring = hsla(ACCENT);
    c.popover = hsla(PANEL_RAISED);
    c.popover_foreground = hsla(TEXT);
    c.slider_bar = hsla(ACCENT);
    c.slider_thumb = hsla(TEXT);
    c.switch = hsla(BORDER);
    c.tab_bar = hsla(PANEL);
    c.tab_active = hsla(PANEL_RAISED);
    c.tab_active_foreground = hsla(ACCENT);
    c.tab_foreground = hsla(TEXT_DIM);
    c.title_bar = hsla(BG);
    c.title_bar_border = hsla(BG);
    c.list = hsla(PANEL);
    c.list_hover = hsla(PANEL_RAISED);
    c.list_active = hsla(PANEL_RAISED);
    c.list_active_border = hsla(ACCENT);
    c.scrollbar = hsla(PANEL);
    c.scrollbar_thumb = hsla(BORDER);
    c.window_border = hsla(BG);
}

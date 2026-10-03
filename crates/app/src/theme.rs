//! The design tokens of the editor: colours, type, spacing, radii and fixed
//! sizes. `docs/design/language.md` says what each one is for.
//!
//! CapCut's placement and dark mood, with our own look: cool graphite
//! surfaces, an aqua accent, hairline borders, softer radii and a mono face
//! for every number. Everything the app draws itself takes its values from
//! here; widgets from GPUI Component take theirs from the component theme,
//! which [`apply`] sets to match.

use std::time::Duration;

use gpui::component::theme::{Theme, ThemeMode};
use gpui::{px, rgb, App, Hsla};

// --- surfaces, by elevation --------------------------------------------------

/// The window and the gutters between panels (elevation 0).
pub(crate) const BG: u32 = 0x0f1013;
/// Panels: assets, player, inspector, timeline (elevation 1).
pub(crate) const PANEL: u32 = 0x1a1b1f;
/// Cards, tiles, hovered and selected rows (elevation 2).
pub(crate) const PANEL_RAISED: u32 = 0x24262b;
/// Menus, popovers, tooltips, dialogs (elevation 3).
pub(crate) const OVERLAY: u32 = 0x2a2c32;
/// Sunken fields: number boxes, search, path fields.
pub(crate) const WELL: u32 = 0x131417;
/// The backdrop behind the canvas in the player.
pub(crate) const VIEWER: u32 = 0x000000;

// --- borders -------------------------------------------------------------------

/// Panel outlines and dividers inside a panel.
pub(crate) const HAIRLINE: u32 = 0x26282d;
/// Control outlines, a tile's outline on hover.
pub(crate) const BORDER: u32 = 0x33363d;
/// Outlined buttons.
pub(crate) const BORDER_STRONG: u32 = 0x464a52;

// --- text ----------------------------------------------------------------------

pub(crate) const TEXT: u32 = 0xe9eaee;
/// Labels, inactive tabs.
pub(crate) const TEXT_DIM: u32 = 0x9a9ea8;
/// Hints, placeholders, units.
pub(crate) const TEXT_MUTED: u32 = 0x6b6f79;
pub(crate) const TEXT_DISABLED: u32 = 0x4a4d55;

// --- accent --------------------------------------------------------------------

pub(crate) const ACCENT: u32 = 0x2fd5c8;
pub(crate) const ACCENT_HOVER: u32 = 0x5fe2d7;
pub(crate) const ACCENT_PRESSED: u32 = 0x22b5aa;
/// Text and icons on an accent fill.
pub(crate) const ON_ACCENT: u32 = 0x062220;

/// The accent as a faint fill: selected rows, toggled icon buttons.
pub(crate) fn accent_soft() -> Hsla {
    with_alpha(ACCENT, 0.14)
}

/// The accent as a drop target's fill.
pub(crate) fn accent_drop() -> Hsla {
    with_alpha(ACCENT, 0.08)
}

/// Black at `alpha`, for scrims behind badges on pictures.
pub(crate) fn scrim(alpha: f32) -> Hsla {
    gpui::hsla(0.0, 0.0, 0.0, alpha)
}

// --- semantic ------------------------------------------------------------------

pub(crate) const SUCCESS: u32 = 0x3ecf8e;
pub(crate) const WARNING: u32 = 0xf2b440;
pub(crate) const DANGER: u32 = 0xf2555a;
pub(crate) const INFO: u32 = 0x5aa8ff;

// --- the timeline --------------------------------------------------------------

pub(crate) const PLAYHEAD: u32 = 0xffffff;
pub(crate) const TRACK_HEADER: u32 = 0x1f2125;
pub(crate) const CLIP_VIDEO: u32 = 0x0d4f54;
pub(crate) const CLIP_VIDEO_TITLE: u32 = 0x11666c;
pub(crate) const CLIP_IMAGE: u32 = 0x463b86;
pub(crate) const CLIP_AUDIO: u32 = 0x14345c;
pub(crate) const CLIP_AUDIO_WAVE: u32 = 0x4f86c9;
pub(crate) const CLIP_TEXT: u32 = 0x7d5419;
/// Effect and sticker clips, once the timeline draws them.
#[allow(dead_code)]
pub(crate) const CLIP_EFFECT: u32 = 0x6b2b60;
pub(crate) const CLIP_OTHER: u32 = 0x4a4c55;

// --- type ----------------------------------------------------------------------

pub(crate) const FONT: &str = "Noto Sans";
/// Timecodes, durations, sizes, every number readout: fixed width, so a
/// running clock does not jitter.
pub(crate) const FONT_MONO: &str = "Noto Sans Mono";

/// Dialog titles, empty-state titles.
pub(crate) const TEXT_DISPLAY: f32 = 15.0;
/// Panel titles and body text.
pub(crate) const TEXT_BODY: f32 = 13.0;
/// Property labels and values.
pub(crate) const TEXT_LABEL: f32 = 12.0;
/// Captions, tile names, hints.
pub(crate) const TEXT_CAPTION: f32 = 11.0;
/// Badges.
pub(crate) const TEXT_BADGE: f32 = 10.0;

// --- spacing, size, radius -----------------------------------------------------

/// The gutter between panels.
pub(crate) const GUTTER: f32 = 6.0;
/// Inner padding of a panel.
pub(crate) const PAD: f32 = 12.0;

pub(crate) const TITLE_BAR_H: f32 = 40.0;
pub(crate) const PANEL_HEADER_H: f32 = 40.0;
pub(crate) const ROW_H: f32 = 28.0;
pub(crate) const CONTROL_H: f32 = 26.0;

/// Checkbox, badge.
pub(crate) const R_XS: f32 = 3.0;
/// Controls, inputs, tiles.
pub(crate) const R_SM: f32 = 5.0;
/// Panels, cards.
pub(crate) const R_MD: f32 = 8.0;
/// Dialogs.
pub(crate) const R_LG: f32 = 12.0;

pub(crate) const MEDIA_W: f32 = 640.0;
pub(crate) const INSPECTOR_W: f32 = 590.0;

// --- motion --------------------------------------------------------------------

/// Collapse and expand.
#[allow(dead_code)]
pub(crate) const MOTION_FAST: Duration = Duration::from_millis(120);

/// How often the view checks the clock and the render thread.
pub(crate) const TICK: Duration = Duration::from_millis(8);

pub(crate) fn hsla(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub(crate) fn with_alpha(hex: u32, alpha: f32) -> Hsla {
    let mut color = hsla(hex);
    color.a = alpha;
    color
}

/// Dark mode, with the component theme's surfaces and accent set to ours.
pub(crate) fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    // `update`, not `global_mut`: widgets read the theme's resolved tokens,
    // which only follow `colors` when the edit goes through `update`. Edited
    // in place, the colours below never reached a Button.
    Theme::update(cx, apply_colors);
}

fn apply_colors(theme: &mut Theme) {
    theme.font_family = FONT.into();
    theme.mono_font_family = FONT_MONO.into();
    // `font_size` stays at 16: GPUI Component makes it the window's rem, and
    // every `p_2`, `gap_3` and `text_sm` in the app is measured in rems. The
    // kit sets its text sizes in pixels instead.
    theme.radius = px(R_SM);
    theme.radius_lg = px(R_LG);
    // A ring outside the border is clipped by every scrolling panel; the
    // tinted border alone is our focus state.
    theme.focus_ring = false;
    let c = &mut theme.colors;
    c.background = hsla(BG);
    c.foreground = hsla(TEXT);
    c.muted = hsla(PANEL_RAISED);
    c.muted_foreground = hsla(TEXT_DIM);
    c.border = hsla(BORDER);
    c.primary = hsla(ACCENT);
    c.primary_hover = hsla(ACCENT_HOVER);
    c.primary_active = hsla(ACCENT_PRESSED);
    c.primary_foreground = hsla(ON_ACCENT);
    c.button = hsla(PANEL_RAISED);
    c.button_hover = hsla(OVERLAY);
    c.button_active = hsla(BORDER);
    c.button_foreground = hsla(TEXT);
    c.button_primary = hsla(ACCENT);
    c.button_primary_hover = hsla(ACCENT_HOVER);
    c.button_primary_active = hsla(ACCENT_PRESSED);
    c.button_primary_foreground = hsla(ON_ACCENT);
    c.button_secondary = hsla(PANEL_RAISED);
    c.button_secondary_hover = hsla(OVERLAY);
    c.button_secondary_active = hsla(BORDER);
    c.button_secondary_foreground = hsla(TEXT);
    c.accent = hsla(PANEL_RAISED);
    c.accent_foreground = hsla(TEXT);
    c.secondary = hsla(PANEL_RAISED);
    c.secondary_hover = hsla(OVERLAY);
    c.secondary_active = hsla(BORDER);
    c.secondary_foreground = hsla(TEXT);
    c.input = hsla(BORDER);
    c.ring = hsla(ACCENT);
    c.caret = hsla(ACCENT);
    c.selection = with_alpha(ACCENT, 0.3);
    c.popover = hsla(OVERLAY);
    c.popover_foreground = hsla(TEXT);
    c.overlay = scrim(0.55);
    c.slider_bar = hsla(ACCENT);
    c.slider_thumb = hsla(TEXT);
    c.switch = hsla(BORDER_STRONG);
    c.switch_thumb = hsla(TEXT);
    c.progress_bar = hsla(ACCENT);
    c.tab = hsla(PANEL);
    c.tab_bar = hsla(PANEL);
    c.tab_bar_segmented = hsla(WELL);
    c.tab_active = hsla(PANEL_RAISED);
    c.tab_active_foreground = hsla(ACCENT);
    c.tab_foreground = hsla(TEXT_DIM);
    c.title_bar = hsla(BG);
    c.title_bar_border = hsla(BG);
    c.list = hsla(PANEL);
    c.list_hover = hsla(PANEL_RAISED);
    c.list_active = accent_soft();
    c.list_active_border = hsla(ACCENT);
    c.scrollbar = gpui::transparent_black();
    c.scrollbar_thumb = hsla(BORDER_STRONG);
    c.scrollbar_thumb_hover = hsla(TEXT_MUTED);
    c.window_border = hsla(BG);
    c.success = hsla(SUCCESS);
    c.warning = hsla(WARNING);
    c.danger = hsla(DANGER);
    c.info = hsla(INFO);
}

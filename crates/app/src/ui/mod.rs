//! chukcut's component kit: the pieces every panel is built from, in the
//! design language of `docs/design/language.md`, with its values from
//! `crate::theme`.
//!
//! The components are plain `RenderOnce` elements that take callbacks, so
//! they work inside any view (the editor, a dialog) without knowing it.
//! Where GPUI Component has the widget (text input, checkbox, tooltip, key
//! hint), it is used underneath; the component theme is set to match in
//! `theme::apply`.
//!
//! | Component | For |
//! |---|---|
//! | [`Panel`], [`PanelHeader`] | every region's surface, its title or tabs, its actions |
//! | [`IconButton`] | toolbars and headers; toggled state; tooltip with shortcut |
//! | [`SegmentedTabs`], [`RailTab`] | sub-tabs in a well; the asset panel's icon rail |
//! | [`SectionHeader`], [`Section`] | collapsible groups with an enable box and reset |
//! | [`PropertyRow`], [`KeyframeSlot`] | label · control · reset · keyframe, aligned |
//! | [`NumberField`] | a mono value in a well with unit and stepper |
//! | [`Badge`] | durations on tiles, states, counts |
//! | [`ColorPicker`], [`color_button`] | a colour: field, hue, opacity, hex, presets, recent |
//! | [`EmptyState`] | what a region says before it has content; drop zones |

// Parts of the kit wait for the timeline and inspector to adopt them.
#![allow(dead_code)]

/// A click handler a component keeps until it renders.
pub(crate) type OnClick = std::rc::Rc<dyn Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App)>;

mod badge;
mod button;
pub(crate) mod color;
mod empty;
pub(crate) mod icons;
mod number;
mod panel;
mod property;
mod section;
mod tabs;

#[allow(unused_imports)]
pub(crate) use badge::{Badge, Tone};
#[allow(unused_imports)]
pub(crate) use button::{IconButton, IconSize};
#[allow(unused_imports)]
pub(crate) use color::{color_button, ColorEvent, ColorPicker};
#[allow(unused_imports)]
pub(crate) use empty::EmptyState;
#[allow(unused_imports)]
pub(crate) use icons::{Glyph, IconSrc};
#[allow(unused_imports)]
pub(crate) use number::NumberField;
#[allow(unused_imports)]
pub(crate) use panel::{Panel, PanelHeader};
#[allow(unused_imports)]
pub(crate) use property::{row_actions, KeyMark, KeyframeSlot, PropertyRow, LABEL_W};
#[allow(unused_imports)]
pub(crate) use section::{Section, SectionHeader};
#[allow(unused_imports)]
pub(crate) use tabs::{RailTab, SegmentedTabs};

//! What the frontend needs in order to offer transitions to a user.
//!
//! The list of kinds is in Rust rather than in TypeScript for the reason every
//! other enumerable thing in this project is: two copies of a list drift, and
//! the copy that drifts is always the one nobody compiles. A kind added to
//! `TransitionKind` appears in the picker without the frontend changing.
//!
//! `directional` and `has_color` say which controls a parameter panel should
//! show. They are properties of the shader — a dissolve has no direction and
//! never will — so they belong next to it.

use serde::Serialize;

use super::library::{presets, Family, GlParam};
use crate::modules::project::document::{Micros, TransitionKind, DEFAULT_TRANSITION_DURATION};

#[derive(Debug, Clone, Serialize)]
pub struct TransitionDescriptor {
    pub kind: TransitionKind,
    /// Shown in the picker. English, sentence case, no trailing period.
    pub label: &'static str,
    /// One line the UI can use as a tooltip.
    pub description: &'static str,
    /// Whether `direction` does anything for this kind.
    pub directional: bool,
    /// Whether `color` does anything for this kind.
    pub has_color: bool,
    /// Whether `softness` does anything for this kind.
    pub has_softness: bool,
    /// Whether `zoom` does anything for this kind.
    pub has_zoom: bool,
    pub default_duration: Micros,
    /// Library transitions only: the preset id the material stores, its
    /// family, its parameters, and who wrote it under which licence.
    pub preset: Option<&'static str>,
    pub family: Option<Family>,
    pub params: &'static [GlParam],
    pub author: &'static str,
    pub license: &'static str,
}

pub fn catalog() -> Vec<TransitionDescriptor> {
    let base = TransitionDescriptor {
        kind: TransitionKind::Dissolve,
        label: "",
        description: "",
        directional: false,
        has_color: false,
        has_softness: false,
        has_zoom: false,
        default_duration: DEFAULT_TRANSITION_DURATION,
        preset: None,
        family: None,
        params: &[],
        author: "chukcut",
        license: "GPL-3.0-or-later",
    };
    let library = presets().iter().map(|p| TransitionDescriptor {
        kind: TransitionKind::Library,
        label: p.label,
        description: match p.family {
            Family::Seamless => "Moves both clips as one, hidden by motion blur.",
            Family::Gl => "From the gl-transitions collection.",
        },
        directional: p.directional,
        preset: Some(p.id.as_str()),
        family: Some(p.family),
        params: p.params,
        author: p.author,
        license: p.license,
        ..base.clone()
    });
    let mut all = vec![
        TransitionDescriptor {
            kind: TransitionKind::Dissolve,
            label: "Cross dissolve",
            description: "One clip fades into the next.",
            ..base.clone()
        },
        TransitionDescriptor {
            kind: TransitionKind::DipToColor,
            label: "Dip to colour",
            description: "Out to a colour, then in from it. Black by default.",
            has_color: true,
            ..base.clone()
        },
        TransitionDescriptor {
            kind: TransitionKind::Wipe,
            label: "Wipe",
            description: "An edge sweeps across the frame, revealing the next clip.",
            directional: true,
            has_softness: true,
            ..base.clone()
        },
        TransitionDescriptor {
            kind: TransitionKind::Slide,
            label: "Slide",
            description: "The next clip pushes the current one out of frame.",
            directional: true,
            ..base.clone()
        },
        TransitionDescriptor {
            kind: TransitionKind::Zoom,
            label: "Zoom",
            description: "The current clip pushes towards the viewer as the next settles back.",
            has_zoom: true,
            ..base.clone()
        },
    ];
    all.extend(library);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_covers_every_kind_exactly_once() {
        // A kind added to the enum and forgotten here is a transition the user
        // cannot reach, which is the failure this test exists to catch.
        let kinds = [
            TransitionKind::Dissolve,
            TransitionKind::DipToColor,
            TransitionKind::Wipe,
            TransitionKind::Slide,
            TransitionKind::Zoom,
        ];
        let listed: Vec<_> = catalog()
            .into_iter()
            .filter(|d| d.preset.is_none())
            .map(|d| d.kind)
            .collect();
        assert_eq!(listed.len(), kinds.len());
        for kind in kinds {
            assert_eq!(listed.iter().filter(|k| **k == kind).count(), 1, "{kind:?}");
        }
    }

    #[test]
    fn every_library_preset_is_listed_once() {
        let listed: Vec<_> = catalog().into_iter().filter_map(|d| d.preset).collect();
        assert_eq!(listed.len(), presets().len());
        for preset in presets() {
            assert!(listed.contains(&preset.id.as_str()), "{}", preset.id);
        }
    }
}

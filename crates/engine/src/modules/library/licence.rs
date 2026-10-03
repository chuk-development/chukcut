//! Licence ids to licence facts, and what the panel does with them.
//!
//! Sources name their licences by SPDX id (Fontsource, Iconify) or we know
//! them (a pack). [`spdx`] turns an id into the `provenance::Licence` the
//! export reads; [`policy`] is the layer between a source and the panel the
//! research asks for: show, show with a warning badge, or hide. NonCommercial
//! is hidden unless the user asks for it; NoDerivatives audio is never shown,
//! because music under a picture is always an adaptation; copyleft software
//! licences (GPL) are hidden for pictures, whose terms in a video nobody can
//! state; share-alike is shown with a warning, because the video takes the
//! licence.

use crate::modules::cloud::provenance::{Commercial, Licence};

/// What an item is, for the licence policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    /// Music or a sound effect under a video.
    Audio,
    /// A sticker, an icon, an emoji on top of a video.
    Picture,
    /// A typeface a title is drawn in.
    Font,
}

/// What the panel does with an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Show,
    /// Show with a badge: share-alike, or terms chukcut cannot check.
    Warn,
    Hide,
}

/// The licence named by an SPDX id (case does not matter). Unknown ids come
/// back with `Commercial::Unknown`, which the policy hides.
pub fn spdx(id: &str) -> Licence {
    let lower = id.trim().to_ascii_lowercase();
    let free = |id: &str, name: &str, url: &str| Licence {
        id: id.into(),
        name: name.into(),
        url: url.into(),
        commercial: Commercial::Yes,
        ..Licence::default()
    };
    match lower.as_str() {
        "mit" => free("MIT", "MIT", "https://opensource.org/license/mit"),
        "isc" => free(
            "ISC",
            "ISC",
            "https://opensource.org/license/isc-license-txt",
        ),
        "apache-2.0" => free(
            "Apache-2.0",
            "Apache 2.0",
            "https://www.apache.org/licenses/LICENSE-2.0",
        ),
        "bsd-2-clause" => free(
            "BSD-2-Clause",
            "BSD",
            "https://opensource.org/license/bsd-2-clause",
        ),
        "bsd-3-clause" => free(
            "BSD-3-Clause",
            "BSD",
            "https://opensource.org/license/bsd-3-clause",
        ),
        "cc0-1.0" | "cc0" => free(
            "CC0-1.0",
            "CC0",
            "https://creativecommons.org/publicdomain/zero/1.0/",
        ),
        "unlicense" => free("Unlicense", "Unlicense", "https://unlicense.org/"),
        "ofl-1.1" | "ofl" => free(
            "OFL-1.1",
            "SIL Open Font License 1.1",
            "https://openfontlicense.org",
        ),
        "ufl-1.0" => free(
            "UFL-1.0",
            "Ubuntu Font Licence 1.0",
            "https://ubuntu.com/legal/font-licence",
        ),
        _ if lower.starts_with("cc-by") => {
            // CC-BY-4.0, CC-BY-SA-3.0, CC-BY-NC-ND-4.0, …
            let rest = lower.trim_start_matches("cc-");
            let (terms, version) = match rest.rsplit_once('-') {
                Some((t, v)) if v.chars().next().is_some_and(|c| c.is_ascii_digit()) => (t, v),
                _ => (rest, "4.0"),
            };
            let mut licence = Licence::from_cc_url(&format!(
                "https://creativecommons.org/licenses/{terms}/{version}/"
            ));
            if terms.contains("nd") {
                licence.note = "NoDerivatives: may not be edited or set to a picture".into();
            }
            licence
        }
        _ if lower.starts_with("gpl") || lower.starts_with("lgpl") || lower.starts_with("agpl") => {
            Licence {
                id: id.into(),
                name: id.into(),
                url: "https://www.gnu.org/licenses/".into(),
                commercial: Commercial::Yes,
                share_alike: true,
                note: "a software licence; its terms for a picture in a video are unclear".into(),
                ..Licence::default()
            }
        }
        _ => Licence {
            id: if id.is_empty() {
                "LicenseRef-Unknown".into()
            } else {
                format!("LicenseRef-{id}")
            },
            name: if id.is_empty() {
                "Unknown licence".into()
            } else {
                id.into()
            },
            commercial: Commercial::Unknown,
            ..Licence::default()
        },
    }
}

/// Whether the licence forbids adaptations (CC ND).
pub fn no_derivatives(licence: &Licence) -> bool {
    let id = licence.id.to_ascii_uppercase();
    id.starts_with("CC-") && id.contains("-ND")
}

fn is_software_copyleft(licence: &Licence) -> bool {
    let id = licence.id.to_ascii_uppercase();
    id.starts_with("GPL") || id.starts_with("LGPL") || id.starts_with("AGPL")
}

/// The research's policy layer. `include_nc` is the user's "show
/// non-commercial" filter; it never brings back ND audio.
pub fn policy(licence: &Licence, kind: Use, include_nc: bool) -> Policy {
    if kind == Use::Audio && no_derivatives(licence) {
        return Policy::Hide;
    }
    if kind == Use::Picture && is_software_copyleft(licence) {
        return Policy::Hide;
    }
    match licence.commercial {
        Commercial::No if !include_nc => Policy::Hide,
        Commercial::No => Policy::Warn,
        Commercial::Unknown => Policy::Hide,
        Commercial::Yes if licence.share_alike => Policy::Warn,
        Commercial::Yes => Policy::Show,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spdx_ids_become_licence_facts() {
        let mit = spdx("MIT");
        assert_eq!(mit.commercial, Commercial::Yes);
        assert!(!mit.attribution_required);
        let by = spdx("CC-BY-4.0");
        assert_eq!(by.id, "CC-BY-4.0");
        assert!(by.attribution_required);
        let nc = spdx("CC-BY-NC-SA-4.0");
        assert_eq!(nc.commercial, Commercial::No);
        assert!(nc.share_alike);
        assert_eq!(spdx("ofl-1.1").id, "OFL-1.1");
        assert_eq!(spdx("CC0-1.0").name, "CC0");
        assert_eq!(spdx("WTFPL-ish").commercial, Commercial::Unknown);
    }

    #[test]
    fn the_policy_hides_what_a_creator_cannot_use() {
        assert_eq!(policy(&spdx("MIT"), Use::Picture, false), Policy::Show);
        assert_eq!(
            policy(&spdx("CC-BY-SA-4.0"), Use::Picture, false),
            Policy::Warn
        );
        assert_eq!(
            policy(&spdx("CC-BY-NC-4.0"), Use::Picture, false),
            Policy::Hide
        );
        assert_eq!(
            policy(&spdx("CC-BY-NC-4.0"), Use::Picture, true),
            Policy::Warn
        );
        assert_eq!(policy(&spdx("GPL-3.0"), Use::Picture, true), Policy::Hide);
        assert_eq!(
            policy(&spdx("CC-BY-ND-4.0"), Use::Audio, true),
            Policy::Hide
        );
        assert_eq!(policy(&spdx("CC-BY-4.0"), Use::Audio, false), Policy::Show);
        assert_eq!(policy(&spdx("Proprietary"), Use::Font, true), Policy::Hide);
    }
}

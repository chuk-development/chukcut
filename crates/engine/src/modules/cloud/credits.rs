//! What the licences of the media on the timeline ask of the export.
//!
//! The export dialog shows [`LicenceSummary`] before the export starts — the
//! place where a licence problem costs a click, not a takedown — and after a
//! good export [`write_credits`] puts `<name>.credits.txt` next to the video
//! when any item needs credit (`docs/research/open-assets.md`, "Credits file
//! on export").

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::provenance::{provider_name, Commercial, Origin, OriginKind};
use crate::modules::project::{MaterialKind, Project};

/// One material on the timeline that has a provenance record.
#[derive(Debug, Clone, Serialize)]
pub struct LicenceItem {
    pub material_id: String,
    pub kind: MaterialKind,
    /// The file name, for the list.
    pub name: String,
    pub origin: Origin,
}

/// The licence picture of an export.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LicenceSummary {
    pub items: Vec<LicenceItem>,
    /// Items whose licence requires a credit.
    pub needs_credit: usize,
    /// Names of items that must not be in a monetised video.
    pub non_commercial: Vec<String>,
    /// Names of items whose commercial status is not known.
    pub unknown: Vec<String>,
    /// Names of share-alike items: the video takes their licence.
    pub share_alike: Vec<String>,
}

impl LicenceSummary {
    /// Whether the export dialog has anything to say.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether a credits file is due.
    pub fn wants_credits_file(&self) -> bool {
        self.needs_credit > 0
    }

    /// Whether the user should look before exporting.
    pub fn has_warnings(&self) -> bool {
        !self.non_commercial.is_empty() || !self.share_alike.is_empty()
    }

    /// The summary as short lines for the dialog, worst first.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let list = |names: &[String]| {
            let mut text = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            if names.len() > 3 {
                text.push_str(&format!(" and {} more", names.len() - 3));
            }
            text
        };
        if !self.non_commercial.is_empty() {
            lines.push(format!(
                "{} not for commercial use: do not monetise this video ({})",
                count(self.non_commercial.len()),
                list(&self.non_commercial)
            ));
        }
        if !self.share_alike.is_empty() {
            lines.push(format!(
                "{} share-alike: the video must carry the same licence ({})",
                count(self.share_alike.len()),
                list(&self.share_alike)
            ));
        }
        if self.needs_credit > 0 {
            lines.push(format!(
                "{} need{} credit; a credits file is written next to the video",
                count(self.needs_credit),
                if self.needs_credit == 1 { "s" } else { "" }
            ));
        }
        if !self.unknown.is_empty() {
            lines.push(format!(
                "{} with terms chukcut cannot check; read the provider's terms ({})",
                count(self.unknown.len()),
                list(&self.unknown)
            ));
        }
        if lines.is_empty() && !self.items.is_empty() {
            lines.push(format!(
                "{} from online sources, all free for commercial use",
                count(self.items.len())
            ));
        }
        lines
    }
}

fn count(n: usize) -> String {
    if n == 1 {
        "1 item".to_string()
    } else {
        format!("{n} items")
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Every material on a visible lane that has an origin, once each.
pub fn summary(project: &Project) -> LicenceSummary {
    let mut seen = BTreeSet::new();
    let mut out = LicenceSummary::default();
    for track in project.tracks.iter().filter(|t| !t.hidden) {
        for segment in &track.segments {
            let id = &segment.material_id;
            let Some(origin) = project.materials.origins.get(id) else {
                continue;
            };
            if !seen.insert(id.clone()) {
                continue;
            }
            let pool = &project.materials;
            let (kind, path) = if let Some(m) = pool.video(id) {
                (MaterialKind::Video, m.path.clone())
            } else if let Some(m) = pool.audio(id) {
                (MaterialKind::Audio, m.path.clone())
            } else if let Some(m) = pool.image(id) {
                (MaterialKind::Image, m.path.clone())
            } else {
                continue;
            };
            let name = if origin.title.is_empty() {
                file_name(&path)
            } else {
                origin.title.clone()
            };
            if origin.licence.attribution_required {
                out.needs_credit += 1;
            }
            match origin.licence.commercial {
                Commercial::No => out.non_commercial.push(name.clone()),
                Commercial::Unknown => out.unknown.push(name.clone()),
                Commercial::Yes => {}
            }
            if origin.licence.share_alike {
                out.share_alike.push(name.clone());
            }
            out.items.push(LicenceItem {
                material_id: id.clone(),
                kind,
                name,
                origin: origin.clone(),
            });
        }
    }
    out
}

/// The credits text: every item, grouped, in the format its source asks
/// for, then the providers that ask to be named.
pub fn credits_text(summary: &LicenceSummary, title: &str) -> String {
    let mut out = format!("Credits for \"{title}\"\n");
    type Belongs = fn(&LicenceItem) -> bool;
    let groups: [(&str, Belongs); 5] = [
        ("Sound and music", |i| {
            i.kind == MaterialKind::Audio && i.origin.kind != OriginKind::Generated
        }),
        ("Footage", |i| {
            i.kind == MaterialKind::Video && i.origin.kind != OriginKind::Generated
        }),
        ("Images", |i| {
            i.kind == MaterialKind::Image && i.origin.kind == OriginKind::Stock
        }),
        // Library pictures are stickers, emoji and icons.
        ("Stickers", |i| {
            i.kind == MaterialKind::Image && i.origin.kind == OriginKind::Library
        }),
        ("Generated", |i| i.origin.kind == OriginKind::Generated),
    ];
    for (heading, belongs) in groups {
        let lines: Vec<String> = summary
            .items
            .iter()
            .filter(|i| belongs(i))
            .map(|i| {
                let mut line = format!("- {}", i.origin.credit_line());
                if i.origin.licence.commercial == Commercial::No {
                    line.push_str(" [non-commercial]");
                }
                if !i.origin.licence.url.is_empty()
                    && !line.contains(&i.origin.licence.url)
                    && i.origin.licence.attribution_required
                {
                    line.push_str(&format!(" <{}>", i.origin.licence.url));
                }
                line
            })
            .collect();
        if !lines.is_empty() {
            out.push_str(&format!("\n{heading}\n"));
            for line in lines {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    let providers: BTreeSet<&str> = summary
        .items
        .iter()
        .filter(|i| i.origin.kind == OriginKind::Stock)
        .map(|i| i.origin.provider.as_str())
        .collect();
    if !providers.is_empty() {
        let names: Vec<&str> = providers.iter().map(|p| provider_name(p)).collect();
        out.push_str(&format!("\nStock media from {}.\n", names.join(", ")));
    }
    out
}

/// `clip.mp4` → `clip.credits.txt`, beside it.
pub fn credits_path(video: &Path) -> PathBuf {
    let stem = video
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "export".into());
    video.with_file_name(format!("{stem}.credits.txt"))
}

/// Write the credits file next to `video` when any item needs credit.
/// `Ok(None)` when none does.
pub fn write_credits(project: &Project, video: &Path) -> Result<Option<PathBuf>, String> {
    let summary = summary(project);
    if !summary.wants_credits_file() {
        return Ok(None);
    }
    let path = credits_path(video);
    std::fs::write(&path, credits_text(&summary, &project.name))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::super::provenance::Licence;
    use super::*;
    use crate::modules::captions::edit::caption_segment;
    use crate::modules::project::{AudioMaterial, CanvasConfig, Track, TrackKind};

    fn origin(provider: &str, licence: Licence) -> Origin {
        Origin {
            title: format!("{provider} item"),
            creator: "Someone".into(),
            source_url: format!("https://{provider}.example/1"),
            licence,
            ..Origin::new(OriginKind::Stock, provider)
        }
    }

    fn project_with(origins: Vec<Origin>) -> Project {
        let mut project = Project::new("Trip", CanvasConfig::default(), 30.0);
        let mut track = Track::new(TrackKind::Audio, "Audio");
        for (i, origin) in origins.into_iter().enumerate() {
            let id = format!("m{i}");
            project.materials.audios.push(AudioMaterial {
                id: id.clone(),
                path: format!("/x/{id}.mp3"),
                duration: 1_000_000,
                sample_rate: 48_000,
                channels: 2,
            });
            project.materials.origins.insert(id.clone(), origin);
            track
                .segments
                .push(caption_segment(&id, i as i64 * 1_000_000, 1_000_000));
        }
        project.tracks.push(track);
        project
    }

    #[test]
    fn the_summary_names_what_needs_care() {
        let by = Licence::from_cc_url("https://creativecommons.org/licenses/by/4.0/");
        let nc = Licence::from_cc_url("https://creativecommons.org/licenses/by-nc/4.0/");
        let mut eleven = Origin::new(OriginKind::Generated, "elevenlabs");
        eleven.title = "Voiceover".into();
        eleven.account_tier = Some("free".into());
        eleven.licence.commercial = Commercial::No;
        let project = project_with(vec![
            origin("freesound", by),
            origin("freesound", nc),
            eleven,
        ]);
        let summary = summary(&project);
        assert_eq!(summary.items.len(), 3);
        assert_eq!(summary.needs_credit, 2);
        assert_eq!(summary.non_commercial.len(), 2);
        let lines = summary.lines();
        assert!(
            lines[0].starts_with("2 items not for commercial use"),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("2 items need credit")));

        let text = credits_text(&summary, "Trip");
        assert!(text.contains("Sound and music\n- \"freesound item\" by Someone on Freesound"));
        assert!(text.contains("[non-commercial]"));
        assert!(text.contains("Generated\n- Generated with ElevenLabs, free plan"));
        assert!(text.contains("Stock media from Freesound."));
    }

    #[test]
    fn a_credits_file_is_written_only_when_one_is_due() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch/cloud/credits");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("Trip.mp4");

        let free = project_with(vec![origin(
            "pexels",
            Licence::free("LicenseRef-Pexels", "Pexels License", ""),
        )]);
        assert_eq!(write_credits(&free, &video).unwrap(), None);
        assert_eq!(
            summary(&free).lines(),
            ["1 item from online sources, all free for commercial use"]
        );

        let by = project_with(vec![origin(
            "freesound",
            Licence::from_cc_url("https://creativecommons.org/licenses/by/4.0/"),
        )]);
        let path = write_credits(&by, &video).unwrap().unwrap();
        assert_eq!(path, dir.join("Trip.credits.txt"));
        assert!(std::fs::read_to_string(path).unwrap().contains("CC BY 4.0"));
    }

    #[test]
    fn library_items_are_credited_as_music_and_stickers() {
        let track = crate::modules::library::sounds::curated(Some("Funny"))[0].origin();
        let mut project = project_with(vec![track]);
        let mut sticker = Origin::new(OriginKind::Library, "fluent-emoji");
        sticker.title = "thumbs up".into();
        sticker.credit = "\"thumbs up\" from Fluent Emoji by Microsoft, MIT License".into();
        sticker.licence = crate::modules::library::licence::spdx("MIT");
        project
            .materials
            .images
            .push(crate::modules::project::ImageMaterial {
                id: "img".into(),
                path: "/x/s.png".into(),
                width: 512,
                height: 512,
            });
        project.materials.origins.insert("img".into(), sticker);
        project.tracks[0]
            .segments
            .push(caption_segment("img", 5_000_000, 1_000_000));
        let summary = summary(&project);
        assert_eq!(summary.needs_credit, 1, "the track, not the MIT sticker");
        let text = credits_text(&summary, "Trip");
        assert!(
            text.contains("Sound and music\n- \"Sneaky Snitch\" Kevin MacLeod (incompetech.com)"),
            "{text}"
        );
        assert!(
            text.contains("Stickers\n- \"thumbs up\" from Fluent Emoji"),
            "{text}"
        );
        assert!(
            !text.contains("Stock media from"),
            "library items are not stock: {text}"
        );
    }
}

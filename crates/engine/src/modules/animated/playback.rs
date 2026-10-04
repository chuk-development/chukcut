//! Whether an animated sticker loops or plays once.
//!
//! A per-clip setting, stored like frame blending (`speed::blend`): an entry
//! `{ "sticker_playback": { "mode": "once" } }` in `MaterialPool::extras`,
//! referenced from the clip's `extras`. Looping is the default and stores
//! nothing.

use serde::{Deserialize, Serialize};

use crate::modules::project::document::{new_id, MaterialKind, Micros, Project, Segment};
use crate::modules::project::MaterialPool;
use crate::modules::timeline::ops::EditCommand;

pub const KEY: &str = "sticker_playback";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Playback {
    /// Start again from the first frame whenever it ends.
    #[default]
    Loop,
    /// Play once and hold the last frame.
    Once,
}

impl Playback {
    pub fn name(self) -> &'static str {
        match self {
            Playback::Loop => "loop",
            Playback::Once => "once",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Playback::Loop => "Loop",
            Playback::Once => "Play once",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "loop" => Ok(Playback::Loop),
            "once" | "play_once" | "play-once" => Ok(Playback::Once),
            other => Err(format!("unknown playback {other:?}: use loop or once")),
        }
    }
}

/// The time inside an animation of length `duration` that a clip shows at
/// `source_time`.
pub fn animation_time(playback: Playback, source_time: Micros, duration: Micros) -> Micros {
    if duration <= 0 {
        return 0;
    }
    match playback {
        Playback::Loop => source_time.rem_euclid(duration),
        Playback::Once => source_time.clamp(0, duration - 1),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Block {
    mode: Playback,
}

fn entry(materials: &MaterialPool, id: &str) -> Option<Playback> {
    let value = materials.extras.get(id)?.get(KEY)?;
    serde_json::from_value::<Block>(value.clone())
        .ok()
        .map(|b| b.mode)
}

fn is_entry(materials: &MaterialPool, id: &str) -> bool {
    materials
        .extras
        .get(id)
        .is_some_and(|value| value.get(KEY).is_some())
}

/// The clip's playback; [`Playback::Loop`] when it has no block.
pub fn playback_of(materials: &MaterialPool, segment: &Segment) -> Playback {
    segment
        .extras
        .iter()
        .find_map(|id| entry(materials, id))
        .unwrap_or_default()
}

/// The edit that sets `segment_id`'s playback, with the extras entry to put
/// into the pool first (`None` for looping, the default).
pub fn set_playback_command(
    project: &Project,
    segment_id: &str,
    playback: Playback,
) -> Result<(Option<(String, serde_json::Value)>, EditCommand), String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let materials = &project.materials;
    let animated = (materials.kind_of(&segment.material_id) == Some(MaterialKind::Image))
        .then(|| materials.image(&segment.material_id))
        .flatten()
        .is_some_and(|image| super::animation(std::path::Path::new(&image.path)).is_some());
    if !animated {
        return Err("only an animated sticker (Lottie, GIF, WebP) loops or plays once".into());
    }
    if playback_of(materials, segment) == playback {
        return Err(format!("the sticker already plays {}", playback.name()));
    }
    let entry = (playback != Playback::Loop).then(|| {
        (
            new_id(),
            serde_json::json!({ KEY: Block { mode: playback } }),
        )
    });
    let slot = segment.extras.iter().position(|id| is_entry(materials, id));
    let new_ref = entry.as_ref().map(|(id, _)| id.clone());
    let label = match playback {
        Playback::Loop => "Loop sticker",
        Playback::Once => "Play sticker once",
    };
    let command =
        crate::modules::inspector::edit::replace_segment(project, segment_id, label, |segment| {
            segment.extras.retain(|id| !is_entry(materials, id));
            if let Some(id) = new_ref {
                let at = slot
                    .unwrap_or(segment.extras.len())
                    .min(segment.extras.len());
                segment.extras.insert(at, id);
            }
        })?;
    Ok((entry, command))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looping_wraps_and_playing_once_holds_the_last_frame() {
        assert_eq!(
            animation_time(Playback::Loop, 2_500_000, 1_000_000),
            500_000
        );
        assert_eq!(animation_time(Playback::Loop, 999_999, 1_000_000), 999_999);
        assert_eq!(
            animation_time(Playback::Once, 2_500_000, 1_000_000),
            999_999
        );
        assert_eq!(animation_time(Playback::Once, 300_000, 1_000_000), 300_000);
        assert_eq!(animation_time(Playback::Loop, 5, 0), 0);
    }

    #[test]
    fn names_parse() {
        assert_eq!(Playback::parse("Once"), Ok(Playback::Once));
        assert_eq!(Playback::parse("loop"), Ok(Playback::Loop));
        assert!(Playback::parse("bounce").is_err());
    }
}

//! Turning what a person types into the ids the engine wants.
//!
//! Every clip, lane and material has a UUID. Scripts that just made something
//! get its id back and can use it as is; a person at a shell would rather type
//! a little less. So each reference accepts the full id, a unique prefix of it
//! (at least four characters), and a positional form:
//!
//! - clip: `T:N`, the N-th clip (0-based, in time order) on lane T (0-based,
//!   bottom to top as `info` lists them)
//! - lane: its index, or its name (`"Video 1"`, case-insensitive)
//! - material: its file name or path, as imported
//! - effect: its index in the clip's stack, or its kind (`"gaussian_blur"`)
//!   when the clip has only one of that kind

use chukcut_engine::modules::project::{Project, Segment};

use crate::error::{CliError, CliResult};

const MIN_PREFIX: usize = 4;

fn by_prefix<'a>(
    what: &str,
    reference: &str,
    ids: impl Iterator<Item = &'a str>,
) -> CliResult<Option<String>> {
    if reference.len() < MIN_PREFIX {
        return Ok(None);
    }
    let matches: Vec<&str> = ids.filter(|id| id.starts_with(reference)).collect();
    match matches.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one.to_string())),
        many => Err(CliError::usage(format!(
            "{reference} matches {} {what}s; give more of the id",
            many.len()
        ))),
    }
}

/// The id of the clip `reference` names.
pub fn clip(project: &Project, reference: &str) -> CliResult<String> {
    let reference = reference.trim();
    if project.segment(reference).is_some() {
        return Ok(reference.to_string());
    }
    if let Some((t, n)) = reference.split_once(':') {
        if let (Ok(t), Ok(n)) = (t.trim().parse::<usize>(), n.trim().parse::<usize>()) {
            let track = project.tracks.get(t).ok_or_else(|| {
                CliError::usage(format!(
                    "there is no lane {t}; the project has {} (0 to {})",
                    project.tracks.len(),
                    project.tracks.len().saturating_sub(1)
                ))
            })?;
            let mut segments: Vec<&Segment> = track.segments.iter().collect();
            segments.sort_by_key(|s| s.target_range.start);
            return segments.get(n).map(|s| s.id.clone()).ok_or_else(|| {
                CliError::usage(format!(
                    "lane {t} ({}) has {} clip(s), so there is no clip {n}",
                    track.name,
                    segments.len()
                ))
            });
        }
    }
    let ids = project
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter().map(|s| s.id.as_str()));
    by_prefix("clip", reference, ids)?.ok_or_else(|| {
        CliError::usage(format!(
            "there is no clip {reference}; `chukcut-cli info` lists them"
        ))
    })
}

/// The id of the lane `reference` names.
pub fn track(project: &Project, reference: &str) -> CliResult<String> {
    let reference = reference.trim();
    if project.track(reference).is_some() {
        return Ok(reference.to_string());
    }
    if let Ok(index) = reference.parse::<usize>() {
        return project
            .tracks
            .get(index)
            .map(|t| t.id.clone())
            .ok_or_else(|| CliError::usage(format!("there is no lane {index}")));
    }
    let named: Vec<&str> = project
        .tracks
        .iter()
        .filter(|t| t.name.eq_ignore_ascii_case(reference))
        .map(|t| t.id.as_str())
        .collect();
    match named.as_slice() {
        [one] => return Ok(one.to_string()),
        [] => {}
        _ => {
            return Err(CliError::usage(format!(
                "more than one lane is called {reference}; use its index or id"
            )))
        }
    }
    by_prefix(
        "lane",
        reference,
        project.tracks.iter().map(|t| t.id.as_str()),
    )?
    .ok_or_else(|| CliError::usage(format!("there is no lane {reference}")))
}

/// The id of the imported material `reference` names: an id, a prefix, a
/// path, or a file name.
pub fn material(project: &Project, reference: &str) -> CliResult<String> {
    let reference = reference.trim();
    let pool = &project.materials;
    let all: Vec<(&str, &str)> = pool
        .videos
        .iter()
        .map(|m| (m.id.as_str(), m.path.as_str()))
        .chain(pool.images.iter().map(|m| (m.id.as_str(), m.path.as_str())))
        .chain(pool.audios.iter().map(|m| (m.id.as_str(), m.path.as_str())))
        .collect();
    if all.iter().any(|(id, _)| *id == reference) {
        return Ok(reference.to_string());
    }
    let absolute = crate::session::absolute(std::path::Path::new(reference));
    if let Some((id, _)) = all
        .iter()
        .find(|(_, path)| std::path::Path::new(path) == absolute)
    {
        return Ok(id.to_string());
    }
    let named: Vec<&str> = all
        .iter()
        .filter(|(_, path)| {
            std::path::Path::new(path)
                .file_name()
                .is_some_and(|n| n.to_string_lossy() == reference)
        })
        .map(|(id, _)| *id)
        .collect();
    match named.as_slice() {
        [one] => return Ok(one.to_string()),
        [] => {}
        _ => {
            return Err(CliError::usage(format!(
                "more than one imported file is called {reference}; use its id"
            )))
        }
    }
    by_prefix("material", reference, all.iter().map(|(id, _)| *id))?.ok_or_else(|| {
        CliError::usage(format!(
            "there is no imported material {reference}; import it first, or see `info`"
        ))
    })
}

/// The id of the effect `reference` names in `segment_id`'s stack.
pub fn effect(project: &Project, segment_id: &str, reference: &str) -> CliResult<String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| CliError::usage(format!("there is no clip {segment_id}")))?;
    let stack = project.materials.effects_of(segment);
    if stack.is_empty() {
        return Err(CliError::usage("the clip has no effects"));
    }
    let reference = reference.trim();
    if let Some(found) = stack.iter().find(|e| e.id == reference) {
        return Ok(found.id.clone());
    }
    if let Ok(index) = reference.parse::<usize>() {
        return stack.get(index).map(|e| e.id.clone()).ok_or_else(|| {
            CliError::usage(format!(
                "the clip has {} effect(s), so there is no effect {index}",
                stack.len()
            ))
        });
    }
    let of_kind: Vec<&str> = stack
        .iter()
        .filter(|e| e.kind == reference)
        .map(|e| e.id.as_str())
        .collect();
    match of_kind.as_slice() {
        [one] => return Ok(one.to_string()),
        [] => {}
        _ => {
            return Err(CliError::usage(format!(
                "the clip has more than one {reference}; use its index"
            )))
        }
    }
    by_prefix("effect", reference, stack.iter().map(|e| e.id.as_str()))?
        .ok_or_else(|| CliError::usage(format!("the clip has no effect {reference}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chukcut_engine::modules::project::{
        new_id, CanvasConfig, TimeRange, Track, TrackKind, Transform,
    };

    fn segment(start: i64) -> Segment {
        Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(start, 1_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    #[test]
    fn clips_by_id_prefix_and_position() {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        let mut lane = Track::new(TrackKind::Video, "Video 1");
        let (late, early) = (segment(5_000_000), segment(0));
        lane.segments = vec![early.clone(), late.clone()];
        p.tracks.push(lane);

        assert_eq!(clip(&p, &late.id).unwrap(), late.id);
        assert_eq!(clip(&p, &late.id[..8]).unwrap(), late.id);
        assert_eq!(clip(&p, "0:0").unwrap(), early.id);
        assert_eq!(clip(&p, "0:1").unwrap(), late.id);
        assert!(clip(&p, "0:2").is_err());
        assert!(clip(&p, "1:0").is_err());
        assert!(clip(&p, "ab").is_err(), "too short to be a prefix");
        assert_eq!(track(&p, "video 1").unwrap(), p.tracks[0].id);
        assert_eq!(track(&p, "0").unwrap(), p.tracks[0].id);
    }
}

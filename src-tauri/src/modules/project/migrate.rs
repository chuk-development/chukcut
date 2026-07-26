//! Reading a project file that was not written by this build.
//!
//! Two different problems live here, and they are both about refusing to
//! guess.
//!
//! **Version.** `schema_version` gates loading. A file from an older format is
//! walked up the ladder in [`STEPS`] before it is deserialized; a file from a
//! *newer* one is refused outright, because a document the app half-understands
//! is worse than one it will not open — the half it dropped is saved back over
//! the user's file the first time they press Ctrl+S.
//! `docs/architecture/project-format.md` is explicit about this.
//!
//! **Damage.** `serde_json` cannot write a NaN or an infinity; it writes
//! `null`. So a project that once held a non-finite number — until this commit
//! nothing stopped one from getting in — saved successfully and then failed to
//! load forever after with "invalid type: null, expected f32". Those files
//! exist on users' disks already, so loading repairs them rather than refusing:
//! [`repair_non_finite`] drops the damaged keys, the document's `serde`
//! defaults fill them back in, and the caller is told exactly which values were
//! reset.

use serde::Deserialize;
use serde_json::Value;

use super::document::{Project, SCHEMA_VERSION};

/// A project and everything the user should be told about how it was read.
#[derive(Debug)]
pub struct Loaded {
    pub project: Project,
    /// Prose, one line per thing that was not as expected. Empty for a file
    /// this build wrote itself.
    pub warnings: Vec<String>,
}

/// One rung of the ladder from an older format to the current one.
///
/// A step rewrites the raw JSON — not the typed document, which by definition
/// cannot represent the old shape — so that the *next* step, and finally
/// `Project`'s own `Deserialize`, can read it.
#[allow(dead_code)]
struct Step {
    /// The version this step reads.
    from: u32,
    /// Rewrite `value` in place so that it is a version `from + 1` document.
    apply: fn(&mut Value) -> Result<(), String>,
}

/// The ladder, in order.
///
/// Empty on purpose: the format has only ever been version 1. Adding version 2
/// means bumping [`SCHEMA_VERSION`], appending `Step { from: 1, apply: v1_to_v2 }`
/// here, and writing `v1_to_v2` next to it. Nothing else has to change — the
/// loop below walks whatever is in this table.
const STEPS: &[Step] = &[];

/// Read a project file's text.
///
/// The one entry point for turning bytes on disk into a `Project`. Nothing else
/// may call `serde_json::from_str::<Project>` — that is what let an unknown
/// schema version through in the first place.
pub fn load(raw: &str) -> Result<Loaded, String> {
    let mut value: Value =
        serde_json::from_str(raw).map_err(|e| format!("this file is not valid JSON: {e}"))?;

    let mut warnings = Vec::new();
    let version = declared_version(&value)?;

    if version > SCHEMA_VERSION {
        return Err(format!(
            "this project was saved by a newer version of chukcut (project format {version}; \
             this build understands format {SCHEMA_VERSION}). Update chukcut and open it again — \
             opening it here would silently drop everything the newer format added."
        ));
    }

    if version < SCHEMA_VERSION {
        migrate(&mut value, version, &mut warnings)?;
    }

    let project = match Project::deserialize(&value) {
        Ok(project) => project,
        Err(first) => {
            // Every value `serde_json` could not have written from a live
            // document gets one chance to be repaired before the file is
            // declared unreadable.
            let mut repaired = Vec::new();
            repair_non_finite(&mut value, String::new(), &mut repaired);
            if repaired.is_empty() {
                return Err(format!("cannot read this project: {first}"));
            }
            let project = Project::deserialize(&value)
                .map_err(|e| format!("cannot read this project: {e}"))?;
            warnings.push(format!(
                "{} value{} in this project were not finite numbers — a save wrote them as null — \
                 and have been reset to their defaults: {}",
                repaired.len(),
                if repaired.len() == 1 { "" } else { "s" },
                repaired.join(", ")
            ));
            project
        }
    };

    Ok(Loaded { project, warnings })
}

/// The version the file claims to be, refusing anything that does not claim one.
fn declared_version(value: &Value) -> Result<u32, String> {
    match value.get("schema_version") {
        Some(Value::Number(n)) => n
            .as_u64()
            .filter(|v| *v <= u32::MAX as u64)
            .map(|v| v as u32)
            .ok_or_else(|| format!("this project's schema_version is not a version number: {n}")),
        Some(other) => Err(format!(
            "this project's schema_version is not a version number: {other}"
        )),
        None => Err(
            "this file has no schema_version, so it is not a chukcut project — or it is one that \
             was damaged in transit."
                .into(),
        ),
    }
}

/// Walk an older document up to [`SCHEMA_VERSION`].
fn migrate(value: &mut Value, from: u32, warnings: &mut Vec<String>) -> Result<(), String> {
    let mut version = from;
    while version < SCHEMA_VERSION {
        let step = STEPS.iter().find(|s| s.from == version).ok_or_else(|| {
            format!(
                "this project is saved in format {version}, and this build knows no way to bring \
                 it up to format {SCHEMA_VERSION}. Open it with the version of chukcut that wrote \
                 it and save it again."
            )
        })?;
        (step.apply)(value)?;
        version += 1;
        if let Some(object) = value.as_object_mut() {
            object.insert("schema_version".into(), Value::from(version));
        }
    }
    if version != from {
        warnings.push(format!(
            "this project was saved in format {from} and has been brought up to format {version}; \
             saving it will write the new format."
        ));
    }
    Ok(())
}

/// Replace the `null`s a non-finite number left behind, reporting where each
/// one was.
///
/// Two shapes, because they need different treatment:
///
/// - a `null` at a key is *removed*, so the field's documented `serde` default
///   applies — an opacity comes back at 1.0, not at 0.0, which is the
///   difference between a clip the user can see and one they cannot;
/// - a `null` inside an array (a position, a colour) cannot be removed without
///   shortening the array, so it becomes `0.0`.
///
/// `materials.extras` is skipped entirely: it holds opaque blobs whose nulls
/// are the user's own data and are never the reason a load failed.
pub fn repair_non_finite(value: &mut Value, path: String, repaired: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            let damaged: Vec<String> = map
                .iter()
                .filter(|(key, value)| value.is_null() && key.as_str() != "extras")
                .map(|(key, _)| key.clone())
                .collect();
            for key in damaged {
                repaired.push(join(&path, &key));
                map.remove(&key);
            }
            for (key, child) in map.iter_mut() {
                if key == "extras" {
                    continue;
                }
                repair_non_finite(child, join(&path, key), repaired);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                let path = format!("{path}[{index}]");
                if item.is_null() {
                    repaired.push(path);
                    *item = Value::from(0.0);
                } else {
                    repair_non_finite(item, path, repaired);
                }
            }
        }
        _ => {}
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, Severity, Track, TrackKind};

    fn saved(project: &Project) -> String {
        serde_json::to_string_pretty(project).expect("a project always serializes")
    }

    fn sample() -> Project {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.tracks.push(Track::new(TrackKind::Video, "V1"));
        project
    }

    #[test]
    fn a_file_this_build_wrote_loads_without_a_word() {
        let loaded = load(&saved(&sample())).expect("our own file loads");
        assert_eq!(loaded.project.schema_version, SCHEMA_VERSION);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    #[test]
    fn a_newer_schema_version_is_refused_with_something_to_act_on() {
        let mut project = sample();
        project.schema_version = SCHEMA_VERSION + 98;
        let error = load(&saved(&project)).expect_err("a newer format must not load");

        // The message is the whole point of the check: a refusal the user
        // cannot act on is no better than a crash.
        assert!(error.contains("newer version of chukcut"), "{error}");
        assert!(error.contains("Update chukcut"), "{error}");
        assert!(
            error.contains(&(SCHEMA_VERSION + 98).to_string()),
            "the message names the version it found: {error}"
        );
    }

    #[test]
    fn an_older_schema_version_with_no_migration_is_refused_rather_than_guessed() {
        let mut value: Value = serde_json::from_str(&saved(&sample())).unwrap();
        value["schema_version"] = Value::from(0);
        let error = load(&value.to_string()).expect_err("format 0 has no ladder");
        assert!(error.contains("format 0"), "{error}");
    }

    #[test]
    fn a_file_that_is_not_a_project_says_so() {
        let error = load(r#"{"hello": "world"}"#).expect_err("not a project");
        assert!(error.contains("schema_version"), "{error}");
        assert!(load("not json at all").is_err());
    }

    #[test]
    fn a_project_whose_floats_were_written_as_null_still_opens() {
        // Exactly the file `serde_json` produces for a document that held a
        // NaN: `null` where a number belongs.
        let mut value: Value = serde_json::from_str(&saved(&sample())).unwrap();
        value["fps"] = Value::Null;
        value["canvas"]["background"][2] = Value::Null;

        let loaded = load(&value.to_string()).expect("a damaged project still opens");
        assert_eq!(loaded.project.fps, 30.0, "the documented default, not zero");
        assert_eq!(loaded.project.canvas.background[2], 0.0);
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains("fps"), "{:?}", loaded.warnings);
        assert!(
            loaded.warnings[0].contains("canvas.background[2]"),
            "the warning names the value that was lost: {:?}",
            loaded.warnings
        );

        // And what comes out is a document that saves and reloads cleanly,
        // which is the property the original file lost.
        let again = load(&saved(&loaded.project)).expect("the repaired project round trips");
        assert!(again.warnings.is_empty());
        assert!(!again
            .project
            .validate()
            .iter()
            .any(|i| i.severity == Severity::Error));
    }

    #[test]
    fn a_null_opacity_comes_back_visible() {
        // Dropping the key rather than zeroing it is the difference between a
        // clip the user can still see and one that vanished.
        let mut project = sample();
        let mut segment = crate::modules::project::document::Segment {
            id: "s1".into(),
            material_id: "m1".into(),
            target_range: crate::modules::project::document::TimeRange::new(0, 1_000_000),
            source_range: crate::modules::project::document::TimeRange::new(0, 1_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Default::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        segment.transform.opacity = f32::NAN;
        project.tracks[0].segments.push(segment);

        let raw = saved(&project);
        assert!(raw.contains("null"), "serde_json writes NaN as null");
        assert!(
            serde_json::from_str::<Project>(&raw).is_err(),
            "the bare parse is what used to lose the file"
        );

        let loaded = load(&raw).expect("but the load path repairs it");
        assert_eq!(loaded.project.tracks[0].segments[0].transform.opacity, 1.0);
    }

    #[test]
    fn repair_leaves_an_extras_blob_alone() {
        // Nulls inside an opaque effect blob are the user's data, not damage.
        let mut project = sample();
        project.materials.extras.insert(
            "effect-0".into(),
            serde_json::json!({ "keep": null, "nested": [null, 1] }),
        );
        let mut value: Value = serde_json::from_str(&saved(&project)).unwrap();
        value["fps"] = Value::Null;

        let loaded = load(&value.to_string()).expect("loads");
        assert_eq!(
            loaded.project.materials.extras["effect-0"],
            serde_json::json!({ "keep": null, "nested": [null, 1] })
        );
        assert!(!loaded.warnings[0].contains("extras"), "{:?}", loaded.warnings);
    }
}

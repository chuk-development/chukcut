//! The command catalog: every operation as one entry with an id, a label,
//! its group, its parameters and what it does to the world.
//!
//! This is what `command_list` answers and what `command_run` runs, in the
//! MCP server and over the app's control socket alike. The shape is
//! FilmCraft's (storytold/filmcraft, `command_list` / `command_run`): an
//! agent learns a handful of tools and finds every capability in one
//! catalog, instead of reading a hundred and fifty tool schemas before its
//! first call. A command's id is the operation's batch name (`split`,
//! `grade`), so a batch file, the CLI manual and the catalog use one word for
//! one thing.

use serde_json::{json, Map, Value};

use crate::error::{CliError, CliResult};
use crate::ops::ml::MlArgs;
use crate::ops::project::{CatalogArgs, NewArgs};
use crate::ops::template::{TemplateApplyArgs, TemplateDeleteArgs, TemplateListArgs};
use crate::ops::{self, Ctx, Operation, Outcome, ToolSpec};
use crate::session::Session;

/// The commands whose operation only reads: MCP's `readOnlyHint`.
pub const READ_ONLY: &[&str] = &[
    "info",
    "template_list",
    "template_slots",
    "validate",
    "captions_list",
    "silence_detect",
    "loudness",
    "catalog",
    "marker_list",
    "analysis",
    "stock_kinds",
    "stock_search",
    "presets",
    "estimate",
    "timeline_list",
    "audio_effects",
    // Reads the clip's faces; writes only the landmark cache.
    "face_landmarks",
    // Reads the clip's people; writes only the landmark cache.
    "body_landmarks",
];

/// The commands that talk to a service outside this machine: MCP's
/// `openWorldHint`.
pub const OPEN_WORLD: &[&str] = &[
    "captions_transcribe",
    "translate_captions",
    "tts",
    "stock_kinds",
    "stock_search",
    "stock_download",
    "ml",
    // Downloads the model and ONNX Runtime on first use.
    "remove_background",
    "select_object",
    "apply_to",
    // Download RIFE on first use (`--mode flow`).
    "frame_blend",
    "smooth_slow_mo",
    "speed_effect",
    // Download HTDemucs or the face mesh on first use.
    "isolate_voice",
    "face_landmarks",
    "retouch",
    "follow_face",
    // Download RTMPose and the person detector on first use.
    "body_landmarks",
    "follow_body",
    // Download LaMa or Real-ESRGAN on first use.
    "remove_object",
    "enhance_quality",
    "sound",
    "fal",
    "sticker",
    "music",
    "sfx",
    "title_font",
    "catalog",
];

/// Commands that work without an open project.
pub const NO_PROJECT: &[&str] = &["catalog", "ml", "template_list", "template_delete"];

/// Commands that write a new project file at `project` instead of editing
/// one: there is no open document for them in the running app.
pub const CREATES_PROJECT: &[&str] = &["new_project", "template_apply"];

/// Every command as a tool spec: the operations on a project, then the
/// commands that make a project or need none.
pub fn specs() -> Vec<ToolSpec> {
    let mut out = vec![ops::tool_spec::<NewArgs>("new_project")];
    out.extend(ops::tools());
    let mut apply = ops::tool_spec::<TemplateApplyArgs>("template_apply");
    apply.description = format!(
        "{} \"project\" is the new project file to write, or with \"into\" the existing project.",
        apply.description
    );
    out.push(apply);
    out.push(ops::tool_spec::<TemplateListArgs>("template_list"));
    out.push(ops::tool_spec::<TemplateDeleteArgs>("template_delete"));
    out.push(ops::tool_spec::<CatalogArgs>("catalog"));
    out.push(ops::tool_spec::<MlArgs>("ml"));
    out
}

/// Every command id.
pub fn ids() -> Vec<&'static str> {
    specs().into_iter().map(|s| s.name).collect()
}

pub fn is_command(id: &str) -> bool {
    ids().contains(&id)
}

/// The first sentence of a description: the label a list shows.
fn label(description: &str) -> String {
    let first = description.split("\n\n").next().unwrap_or(description);
    let end = first
        .char_indices()
        .find(|&(i, c)| c == '.' && first[i + 1..].starts_with([' ', '\n']))
        .map(|(i, _)| i)
        .unwrap_or_else(|| first.trim_end_matches('.').len());
    first[..end].replace('\n', " ").trim().to_string()
}

fn group(id: &str) -> &'static str {
    match id {
        "new_project" | "catalog" => "project",
        "template_apply" | "template_list" | "template_delete" => "template",
        "ml" => "ml",
        _ => ops::groups()
            .into_iter()
            .find(|(name, _)| *name == id)
            .map(|(_, group)| group)
            .unwrap_or("ops"),
    }
}

/// One catalog entry. `has_project` says whether a project is there to run
/// against, which is what `enabled` reports.
fn entry(spec: &ToolSpec, has_project: bool) -> Value {
    let needs_project = !NO_PROJECT.contains(&spec.name) && !CREATES_PROJECT.contains(&spec.name);
    json!({
        "id": spec.name,
        "label": label(&spec.description),
        "group": group(spec.name),
        "description": spec.description,
        "params": spec.schema,
        "readOnly": READ_ONLY.contains(&spec.name),
        "openWorld": OPEN_WORLD.contains(&spec.name),
        "needsProject": needs_project,
        "enabled": !needs_project || has_project,
    })
}

/// The catalog, filtered: `filter` matches the id or the label, any case;
/// `enabled_only` keeps the commands that can run right now.
pub fn list(filter: Option<&str>, enabled_only: bool, has_project: bool) -> Value {
    let needle = filter.unwrap_or_default().to_lowercase();
    Value::Array(
        specs()
            .iter()
            .map(|spec| entry(spec, has_project))
            .filter(|e| {
                let id = e["id"].as_str().unwrap_or_default().to_lowercase();
                let label = e["label"].as_str().unwrap_or_default().to_lowercase();
                (needle.is_empty() || id.contains(&needle) || label.contains(&needle))
                    && (!enabled_only || e["enabled"] == true)
            })
            .collect(),
    )
}

fn args_of<T: serde::de::DeserializeOwned>(id: &str, params: Value) -> CliResult<T> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).map_err(|e| CliError::usage(format!("{id}: {e}")))
}

/// Run a command that needs no project; `None` when `id` needs one.
pub fn run_without_project(id: &str, params: Value) -> Option<CliResult<Outcome>> {
    Some(match id {
        "catalog" => args_of::<CatalogArgs>(id, params).and_then(CatalogArgs::run),
        "ml" => args_of::<MlArgs>(id, params).and_then(MlArgs::run),
        "template_list" => args_of::<TemplateListArgs>(id, params).and_then(TemplateListArgs::run),
        "template_delete" => {
            args_of::<TemplateDeleteArgs>(id, params).and_then(TemplateDeleteArgs::run)
        }
        _ => return None,
    })
}

/// Run command `id` against an open project. `template_apply` with `into`
/// is the operation on this project; the commands that write a new file
/// are not, and say so.
pub fn run_on(id: &str, mut params: Value, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
    if let Some(result) = run_without_project(id, params.clone()) {
        return result;
    }
    if params.is_null() {
        params = json!({});
    }
    let into = params.get("into").and_then(Value::as_bool) == Some(true);
    match id {
        "template_apply" if into => {
            if let Some(map) = params.as_object_mut() {
                map.remove("into");
            }
            let args: TemplateApplyArgs = args_of(id, params)?;
            Operation::run(args.into_args(), session, ctx)
        }
        "new_project" | "template_apply" => Err(CliError::usage(format!(
            "{id} writes a new project file; give it the file as \"project\" (or use \
             template_apply with \"into\": true for this project)"
        ))),
        _ => ops::run_named(id, params, session, ctx),
    }
}

/// The first argument `params` has that command `id` does not take, as a
/// message naming the accepted ones.
pub fn unknown_argument(id: &str, params: &Value) -> Option<String> {
    if let Some(message) = ops::unknown_argument(id, params) {
        return Some(message);
    }
    let spec = specs().into_iter().find(|s| s.name == id)?;
    let props = spec.schema.get("properties")?.as_object()?;
    let map = params.as_object()?;
    let bad = map
        .keys()
        .find(|k| !props.contains_key(*k) && k.as_str() != "project")?;
    let mut names: Vec<&str> = props.keys().map(String::as_str).collect();
    names.sort_unstable();
    Some(format!(
        "{id}: unknown argument {bad:?}; the arguments are: {}",
        names.join(", ")
    ))
}

/// The input schema of `id` as a standalone object, for a tool.
pub fn schema(id: &str) -> Option<Value> {
    specs().into_iter().find(|s| s.name == id).map(|s| s.schema)
}

/// An outcome as the JSON the MCP server and the control socket answer.
pub fn outcome_json(outcome: &Outcome) -> Value {
    let mut map = Map::new();
    map.insert("message".into(), json!(outcome.message));
    map.insert("data".into(), outcome.data.clone());
    map.insert("changed".into(), json!(outcome.mutated));
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_is_the_first_sentence() {
        assert_eq!(
            label("Split clips at a time. More text."),
            "Split clips at a time"
        );
        assert_eq!(label("Version 2.5 of it\n\nmore"), "Version 2.5 of it");
        assert_eq!(label("No full stop"), "No full stop");
    }

    #[test]
    fn every_command_is_listed_once_with_a_group_and_params() {
        let list = list(None, false, true);
        let entries = list.as_array().unwrap();
        let mut ids: Vec<&str> = entries.iter().map(|e| e["id"].as_str().unwrap()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "ids are unique");
        for e in entries {
            assert_eq!(e["params"]["type"], "object", "{}", e["id"]);
            assert!(!e["label"].as_str().unwrap().is_empty(), "{}", e["id"]);
        }
        let split = entries.iter().find(|e| e["id"] == "split").unwrap();
        assert_eq!(split["group"], "timeline");
        assert_eq!(split["readOnly"], false);
        let info = entries.iter().find(|e| e["id"] == "info").unwrap();
        assert_eq!(info["readOnly"], true);
        for name in READ_ONLY.iter().chain(OPEN_WORLD).chain(NO_PROJECT) {
            assert!(ids.contains(name), "{name} in a hint list is a command");
        }
    }

    #[test]
    fn the_filmcraft_ports_are_commands() {
        let list = list(None, false, true);
        for (id, group) in [
            ("export_timeline", "render"),
            ("export_markers", "render"),
            ("relink", "project"),
            ("sync", "analysis"),
        ] {
            let e = list
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == id)
                .unwrap_or_else(|| panic!("{id} is in the catalog"));
            assert_eq!(e["group"], group, "{id}");
            assert_eq!(e["needsProject"], true, "{id}");
        }
        // Their arguments are checked like every command's.
        let message = unknown_argument("export_timeline", &json!({"fromat": "edl"})).unwrap();
        assert!(message.contains("format"), "{message}");
    }

    #[test]
    fn without_a_project_only_the_projectless_commands_are_enabled() {
        let enabled = list(None, true, false);
        let ids: Vec<&str> = enabled
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"catalog") && ids.contains(&"new_project"));
        assert!(!ids.contains(&"split"));
        let found = list(Some("SPLIT"), false, true);
        assert!(found.as_array().unwrap().iter().any(|e| e["id"] == "split"));
    }

    #[test]
    fn an_unknown_argument_is_named_with_the_accepted_ones() {
        let message = unknown_argument("split", &json!({"att": 2})).unwrap();
        assert!(
            message.contains("\"att\"") && message.contains("at"),
            "{message}"
        );
        assert!(unknown_argument("split", &json!({"at": 2})).is_none());
        let message = unknown_argument("catalog", &json!({"knd": "effects"})).unwrap();
        assert!(message.contains("kind"), "{message}");
    }
}

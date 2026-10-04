//! Every command-layer function is reachable from the CLI and the MCP
//! server, or is on a list here with the reason it is not.
//!
//! CLAUDE.md: "A user-visible capability is a function in a
//! `modules/<name>/commands.rs` ... The app calls it; a CLI and an MCP
//! server will call the same function." This test holds the CLI to that. It
//! reads the source rather than a registry, because there is no registry of
//! engine commands: a new `pub fn` in a `commands.rs` fails here until an
//! operation calls it or the allowlist says why none should.
//!
//! A function counts as reachable when its name appears in `crates/cli/src`
//! outside a comment. That is enough because the CLI only calls the engine
//! from operations: every `impl Operation` is in the `operations!` list (an
//! MCP tool and a batch op) and has a subcommand (`On<...>`), which the two
//! other tests here check. `catalog` is in both the CLI and the MCP server.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Functions no operation calls, and why. Kept short and honest: "the app
/// needs it" is not a reason, "it only means something inside a running
/// app" is.
const ALLOWLIST: &[(&str, &str)] = &[
    // Jobs and cancellation inside one process. A CLI operation waits for its
    // own work and exits; there is nothing left for a second call to see.
    ("analysis::analysis_jobs", "lists the app's background analysis jobs; a CLI analysis waits for its job"),
    ("analysis::analysis_cancel", "cancels a background job of this process; a CLI analysis runs to its end"),
    ("analysis::analysis_wait", "the CLI's analysis operations poll analysis_status with progress instead"),
    ("export::export_queue_add", "the app's queue; export_queue adds whole projects with export_queue_add_project"),
    ("export::export_queue_cancel", "cancels a queued export of this process; export_queue runs its list to the end"),
    ("export::export_queue_remove", "edits the app's queue panel; the CLI's queue lives for one call"),
    ("export::export_queue_move", "reorders the app's queue panel; the CLI's queue lives for one call"),
    ("export::export_queue_clear_finished", "tidies the app's queue panel"),
    ("export::export_queue_wait_idle", "export_queue follows its jobs through export_queue_subscribe"),
    ("export::export_cancel", "cancels a running export of this process; the CLI exports in the foreground"),
    ("export::cancel_all_exports", "the app's quit path"),
    ("media::media_thumbnails_cancel", "cancels the app's thumbnail job"),
    ("media::cancel_all_thumbnails", "the app's quit path"),
    ("proxy::proxy_cancel", "cancels a proxy encode of this process; the CLI turns proxies off"),
    ("proxy::cancel_all_proxies", "the app's quit path"),
    ("tracking::tracking_cancel", "cancels the app's background tracking job; track runs to its end"),
    ("matting::matting_cancel", "cancels the app's background bake; remove_background runs to its end"),
    ("matting::matting_coverage", "the inspector's progress; remove_background bakes until nothing is missing"),
    ("matting::matting_ensure", "the export's own step; export calls it before rendering"),
    ("matting::job_for", "a helper the bake commands and tests share, not an operation"),
    ("matting::current_model", "the model version this build writes; remove_background uses it through matting_remove_background"),
    ("matting::setting_for", "the setting a mode writes; remove_background --model uses it through matting_remove_background_with"),
    ("matting::matting_remove_background", "the app's people-only toggle; remove_background calls matting_remove_background_with, the same edit with a model"),
    ("matting::matting_queue_missing", "the app's re-bake after an edit made a clip longer; a CLI export bakes what is missing (matting_ensure) and remove_background bakes its clip"),
    ("matting::matting_running", "the app's status line for background bakes"),
    ("matting::matting_cache_clear", "Settings, Storage in the app; a CLI run must not delete what a running app shows"),
    ("speed::speed_flow_status", "the app's progress for an optical-flow bake; frame_blend and smooth_slow_mo wait with speed_flow_wait"),
    ("speed::speed_flow_running", "the app's status line for optical-flow bakes"),
    ("speed::speed_flow_queue_missing", "the app's re-bake after an edit; a CLI export bakes what is missing (speed_flow_ensure) and frame_blend --mode flow bakes its clip"),
    ("landmarks::landmarks_queue_missing", "the app's background analysis after an edit; retouch and face_landmarks analyse their clip, an export calls landmarks_ensure"),
    ("landmarks::landmarks_ensure", "the export's own step; export calls it before rendering"),
    ("landmarks::needing", "a helper of the export step and the app's queue, not an operation"),
    ("landmarks::face_samples", "the pose samples landmarks_follow_face builds; a public helper for its tests"),
    ("body::part_samples", "the pose samples body_follow builds; a public helper for its tests"),
    ("voice::voice_isolation_missing", "the app's re-render after a cache was cleared; an export renders what is missing (denoise::ensure_rendered) and isolate_voice renders its clip"),
    ("speed::speed_flow_ensure", "the export's own step; export calls it before rendering"),
    ("enhance::enhance_status", "the app's progress for a remade-frame bake; remove_object and enhance_quality wait with enhance_wait"),
    ("enhance::enhance_running", "the app's status line for remade-frame bakes"),
    ("enhance::enhance_queue_missing", "the app's re-bake after an edit; a CLI export bakes what is missing (enhance_ensure) and remove_object and enhance_quality bake their clip"),
    ("enhance::enhance_ensure", "the export's own step; export calls it before rendering"),
    ("enhance::enhance_bake", "remove_object and enhance_quality start the bake through enhance_set_removal and enhance_set_upscale, the same call"),
    ("enhance::enhance_select_object", "the app's click on the player; remove_object --point records the same selection with enhance_prompt_at and enhance_set_removal"),
    ("enhance::enhance_paint", "a brush on the player in the app; remove_object --stroke takes the stroke in source fractions"),
    ("enhance::enhance_cache_info", "Settings, AI acceleration in the app"),
    ("enhance::enhance_cache_clear", "Settings, AI acceleration in the app; a CLI run must not delete what a running app shows"),
    ("prepare::prepare_start", "the app's open path, baking what an opened project lacks in the background; a CLI export bakes what it needs first (matting_ensure, speed_flow_ensure, enhance_ensure)"),
    ("prepare::prepare_status", "the app's status line for prepare_start"),
    ("prepare::prepare_stop", "the Stop on the app's status line for prepare_start"),
    ("prepare::prepare_missing", "the count behind prepare_status, for tests and the app; a CLI export bakes what it needs first"),
    ("compositing::compositing_set_background", "the setting alone; remove_background also bakes the matte"),
    ("ml::gpu_vendors", "part of ml status, which reports it"),
    // The live preview, playback and the app's own windows.
    ("preview::preview_start", "the app's live preview server; render_frame and view_frame render a frame"),
    ("preview::preview_seek", "the live preview"),
    ("preview::preview_play", "the live preview"),
    ("preview::preview_pause", "the live preview"),
    ("preview::preview_stop", "the live preview"),
    ("preview::preview_viewport", "the live preview's window size"),
    ("preview::preview_state", "the live preview"),
    ("audio::audio_status", "the live audio output device"),
    ("audio::audio_set_volume", "the live audio output's master volume, not a document value"),
    ("audiofx::audiofx_render", "fills the player's audio cache; export renders processed sound itself"),
    // Proxies are preview media. The CLI switches them off (main.rs, init_engine).
    ("proxy::proxy_consider", "proxies are preview media; the CLI sets the policy to Off"),
    ("proxy::proxy_request", "proxies are preview media"),
    ("proxy::proxy_request_media", "proxies are preview media"),
    ("proxy::proxy_policy", "proxies are preview media"),
    ("proxy::proxy_generation", "the player's cache key for proxy swaps"),
    ("proxy::proxy_watch", "progress events for the app's proxy badge"),
    ("proxy::proxy_queue_status", "the app's proxy badge"),
    ("proxy::proxy_state", "the app's per-file proxy badge"),
    ("proxy::proxy_cache_clear", "Settings, Storage in the app; a CLI run must not delete what a running app plays"),
    // Pictures for the app's panels.
    ("media::media_thumbnails", "filmstrip pictures for the timeline"),
    ("media::media_waveform", "drawing data for the timeline's audio lanes; loudness measures sound for scripts"),
    ("fx::fx_tile", "the Effects tab's preview tile"),
    ("fx::fx_transition_tile", "the Transitions tab's preview tile"),
    ("fx::fx_split_tile", "the layout picker's preview tile"),
    ("fx::fx_pip_tile", "the layout picker's preview tile"),
    ("text::text_style_tile", "the Text tab's preview tile"),
    ("text::text_template_tile", "the Text tab's preview tile"),
    ("library::library_look_tile", "the Filters tab's preview tile"),
    ("grading::grading_preset_tile", "the Filters tab's preview tile for a grade preset"),
    ("library::library_font_preview", "the font picker's preview tile"),
    ("library::library_font_system_preview", "the font picker's preview tile"),
    ("library::library_sticker_thumb", "the Stickers tab's preview tile"),
    ("library::library_icon_thumb", "the Stickers tab's preview tile"),
    ("library::library_animated_preview", "the Stickers tab's moving preview tile"),
    ("animated::animated_preview", "a moving preview tile for the Stickers tab and the media panel"),
    ("library::library_set_settings", "only chooses where font preview tiles come from (a privacy choice in the app); catalog library_settings shows it"),
    // The app's session, crash recovery and start screen.
    ("project::project_get", "the app's copy of the open document; a Session holds its own"),
    ("project::project_path", "the app's title bar; a Session knows its file"),
    ("project::project_close", "the app's close path; a CLI process ends instead"),
    ("project::project_recovery_claim", "crash recovery of the app's working copy, which the CLI never writes"),
    ("project::project_recovery_pending", "crash recovery of the app's working copy"),
    ("project::project_recovery_restore", "crash recovery of the app's working copy"),
    ("project::project_recovery_discard", "crash recovery of the app's working copy"),
    ("workspace::workspace_recent_list", "catalog recent lists workspace_recent_entries, the same list with posters"),
    ("workspace::workspace_recent_record", "the start screen's list; a CLI edit is not the user opening a project"),
    ("workspace::workspace_recent_clear", "the start screen's list"),
    ("workspace::workspace_recent_forget", "the start screen's list"),
    ("workspace::poster_of", "the start screen's poster picture"),
    ("workspace::workspace_settings_set", "the app's Settings page; the CLI reads them (catalog settings) and never rewrites the user's settings file"),
    ("workspace::workspace_settings_apply", "applies settings to a running app"),
    ("workspace::workspace_trim_cache", "the app's cache limit at start-up; a CLI run must not delete what a running app plays"),
    ("workspace::workspace_cache_clear", "Settings, Storage in the app; a CLI run must not delete what a running app plays"),
    ("workspace::workspace_cache_in_use", "tells the app's cache trimmer what the open project uses"),
    ("workspace::workspace_log_path", "the app's log file; the CLI logs to stderr (-v, RUST_LOG)"),
    // Accounts hold API keys. A key typed on a command line ends up in shell
    // history and in an MCP client's logs; the app's Settings, Accounts page
    // is the one place to add, change or test one (docs/cli.md, Cloud).
    ("cloud::cloud_account_set", "accounts and their keys are managed in the app"),
    ("cloud::cloud_account_new", "accounts and their keys are managed in the app"),
    ("cloud::cloud_account_new_of", "accounts and their keys are managed in the app"),
    ("cloud::cloud_account_remove", "accounts and their keys are managed in the app"),
    ("cloud::cloud_account_test", "accounts and their keys are managed in the app; a cloud operation reports a bad key itself"),
    // Reached another way.
    ("inspector::inspector_set_color", "the four-slider grade of the first UI; grade sets the same fields through inspector_set_grade"),
    ("inspector::inspector_set_grade_control", "grade --set name=value sets any control through inspector_set_grade, several in one step"),
    ("inspector::inspector_set_wheel", "grade --set wheel_x:gain=... sets the wheels through inspector_set_grade"),
    ("compositing::compositing_set_mask", "the player's drag commit; mask --set reaches every field, with keyframes, through compositing_set_mask_value"),
    ("text::text_add", "title add calls text_add_on, the same add with a lane"),
    ("text::text_add_style", "title style calls text_add_style_for, the same add with a length"),
    ("text::text_add_template", "title template calls text_add_template_for, the same add with a length"),
    ("text::text_set", "title set builds the same SetTextMaterial edit and folds it with the clip's own changes into one undo step"),
    ("media::media_probe", "import probes every file it adds, and info shows what it found"),
    ("media::media_missing_files", "validate reports missing media from the document"),
    ("export::estimate_sampled_for", "estimate measures the open project through export_estimate_sampled"),
    ("export::export_memory_recall", "the export dialog's remembered settings; export takes its settings or a preset explicitly"),
    ("export::export_memory_remember", "the export dialog's remembered settings"),
    // Helpers that are public for the engine's own tests and callers.
    ("analysis::stabilise_covers", "a predicate the renderer and the inspector ask; analysis shows the stabilisation"),
    ("compositing::key_probe_project", "the document the eyedropper renders; chroma_key --pick uses compositing_pick_key_color"),
    ("compositing::pick_from", "the eyedropper's pick on any source provider, for tests; chroma_key --pick uses compositing_pick_key_color"),
    ("project::import_material", "the pure half of project_import_media, which import calls"),
    ("speech::transcribe_chunked", "the chunking inside speech_transcribe, which captions transcribe calls"),
    ("template::template_build_project", "the build half of template_new_project, which template apply calls"),
    ("template::template_open_project", "the open half of template_new_project, which template apply calls"),
    ("template::template_info", "one entry of template_list, which template list prints whole"),
    ("template::template_thumbnail", "the app gallery's preview tile; render_frame renders any frame of an applied template"),
    // Keyboard shortcuts belong to the app window; a CLI has no keys to bind.
    ("keymap::keymap_get", "the app's keyboard shortcuts"),
    ("keymap::keymap_bindings", "the app's keyboard shortcuts"),
    ("keymap::keymap_conflicts", "the app's keyboard shortcuts"),
    ("keymap::keymap_set", "the app's keyboard shortcuts"),
    ("keymap::keymap_reset", "the app's keyboard shortcuts"),
    ("keymap::keymap_reset_all", "the app's keyboard shortcuts"),
    ("keymap::keymap_set_preset", "the app's keyboard shortcuts"),
    ("effects::effects_describe", "describes a CapCut effect package the user points at; the package runtime is not a product feature yet (build-out plan: not a priority)"),
];

/// Functions another branch is exposing right now. Not failures, and not
/// checked for staleness, so that branch's merge does not break this test;
/// delete an entry once its operation exists.
const PENDING: &[(&str, &str)] = &[];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `module::function` for every `pub fn` in every `commands.rs`, above its
/// tests.
fn engine_commands() -> BTreeSet<String> {
    let modules = workspace().join("crates/engine/src/modules");
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(&modules).expect("the engine's modules") {
        let dir = entry.expect("a module").path();
        let file = dir.join("commands.rs");
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let module = dir.file_name().unwrap().to_string_lossy().into_owned();
        let source = source.split("#[cfg(test)]").next().unwrap_or_default();
        for line in source.lines() {
            let line = line.trim_start();
            let rest = line
                .strip_prefix("pub fn ")
                .or_else(|| line.strip_prefix("pub async fn "));
            if let Some(rest) = rest {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                out.insert(format!("{module}::{name}"));
            }
        }
    }
    assert!(out.len() > 100, "found only {} commands", out.len());
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a source directory") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The CLI's source with line comments taken out, so a function named in
/// prose does not count as called.
fn cli_source() -> String {
    let mut files = Vec::new();
    rust_files(&workspace().join("crates/cli/src"), &mut files);
    files.sort();
    let mut out = String::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("a source file");
        for line in text.lines() {
            let code = match line.find("//") {
                // A URL in a string is not a comment; neither is anything
                // after it, which only ever is the rest of the URL.
                Some(i) if !line[..i].ends_with(':') => &line[..i],
                _ => line,
            };
            out.push_str(code);
            out.push('\n');
        }
    }
    out
}

fn mentions(source: &str, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    source.match_indices(name).any(|(i, _)| {
        let before = source[..i].chars().next_back();
        let after = source[i + name.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

fn function(qualified: &str) -> &str {
    qualified.rsplit("::").next().unwrap_or(qualified)
}

#[test]
fn every_command_is_reachable_or_allowlisted_with_a_reason() {
    let commands = engine_commands();
    let source = cli_source();
    let allowed: BTreeSet<&str> = ALLOWLIST.iter().map(|(n, _)| *n).collect();
    let pending: BTreeSet<&str> = PENDING.iter().map(|(n, _)| *n).collect();

    let mut unreachable = Vec::new();
    let mut stale = Vec::new();
    for command in &commands {
        let reached = mentions(&source, function(command));
        let listed = allowed.contains(command.as_str());
        if !reached && !listed && !pending.contains(command.as_str()) {
            unreachable.push(command.clone());
        }
        if reached && listed {
            stale.push(command.clone());
        }
    }
    let gone: Vec<&str> = allowed
        .iter()
        .chain(pending.iter())
        .filter(|n| !commands.contains(**n))
        .copied()
        .collect();
    for (name, reason) in ALLOWLIST {
        assert!(
            reason.len() > 10,
            "{name} needs a real reason to be allowlisted"
        );
    }
    for name in &pending {
        if mentions(&source, function(name)) {
            eprintln!("{name} is reachable now; take it off PENDING");
        }
    }

    assert!(
        unreachable.is_empty(),
        "these engine commands have no CLI/MCP operation. Add one (crates/cli/src/ops, \
         docs/cli.md), or put them on ALLOWLIST in this file with the reason:\n  {}",
        unreachable.join("\n  ")
    );
    assert!(
        stale.is_empty(),
        "these allowlisted commands are reachable now; take them off ALLOWLIST:\n  {}",
        stale.join("\n  ")
    );
    assert!(
        gone.is_empty(),
        "these listed commands no longer exist in the engine:\n  {}",
        gone.join("\n  ")
    );
}

/// The types between `operations!(` and its closing `);` in ops/mod.rs.
fn registered_operations() -> BTreeSet<String> {
    let text = std::fs::read_to_string(workspace().join("crates/cli/src/ops/mod.rs")).unwrap();
    let start = text.find("operations!(").expect("the operations! list");
    let body = &text[start + "operations!(".len()..];
    let body = &body[..body.find(");").expect("the end of the list")];
    body.split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| t.rsplit("::").next().unwrap().to_string())
        .collect()
}

#[test]
fn every_operation_is_an_mcp_tool() {
    let registered = registered_operations();
    let mut files = Vec::new();
    rust_files(&workspace().join("crates/cli/src/ops"), &mut files);
    let mut missing = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for (i, _) in text.match_indices("impl Operation for ") {
            let name: String = text[i + "impl Operation for ".len()..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !registered.contains(&name) {
                missing.push(format!("{name} ({})", file.display()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "these operations are not in the operations! list in ops/mod.rs, so neither batch nor \
         the MCP server can run them:\n  {}",
        missing.join("\n  ")
    );
}

/// Operations that only mean something inside one session's history.
const SESSION_ONLY: &[(&str, &str)] = &[
    (
        "UndoArgs",
        "a one-shot command has no history; undo is for batch and MCP sessions",
    ),
    (
        "RedoArgs",
        "a one-shot command has no history; redo is for batch and MCP sessions",
    ),
];

#[test]
fn every_operation_has_a_subcommand() {
    let source = cli_source();
    let missing: Vec<String> = registered_operations()
        .into_iter()
        .filter(|op| !SESSION_ONLY.iter().any(|(n, _)| n == op))
        .filter(|op| !source.contains(&format!("On<{op}>")))
        .collect();
    assert!(
        missing.is_empty(),
        "these operations have no subcommand (an `On<...>` variant in main.rs or an ops \
         subcommand enum):\n  {}",
        missing.join("\n  ")
    );
}

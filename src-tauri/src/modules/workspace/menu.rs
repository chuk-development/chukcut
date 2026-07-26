//! The native menu bar.
//!
//! One rule holds this module together: **an item you can click does
//! something.** Everything the bar offers is either wired to working code or
//! visibly greyed out, and which of the two it is has to be derivable without
//! opening the app. That is what [`ITEMS`] is for — a table of every item, its
//! accelerator, and the one thing that decides whether it is clickable. Adding
//! an item without deciding that is not possible, because [`Gate`] has no
//! default.
//!
//! ## Where the enabled state comes from
//!
//! The frontend's Zustand stores are the only authority on document state —
//! whether anything is open, whether it is dirty, whether the undo stack has
//! anything on it. Rust does not keep a second copy and does not infer one. The
//! webview pushes a [`MenuState`] through `workspace_menu_sync` whenever those
//! facts change, and [`enablement`] maps that state onto the bar. The mapping
//! is a pure function so it can be tested without a GUI, which is the whole
//! reason it is a table and not a pile of `if` statements in the builder.
//!
//! ## Accelerators, and the two we deliberately do not register
//!
//! On GTK a menu accelerator is an entry in the window's `GtkAccelGroup`, and
//! `gtk_window_key_press_event` consults the accel group *before* it propagates
//! the key to the focused widget. A bare-letter accelerator therefore shadows
//! typing: registering `C` for Split Clip would mean the letter `c` never
//! reaches a text field again, and `Delete` would stop deleting characters.
//! Both of those keys are already bound by the timeline, so the menu shows them
//! in its label — `Split Clip (C)` — and does not claim them. The binding that
//! already exists wins; the menu only advertises it.
//!
//! The clipboard items are the same story one step further on. While they were
//! permanently disabled they could safely carry `Ctrl+X`/`C`/`V`/`A`, because
//! GTK's `gtk_widget_can_activate_accel` refuses to activate an insensitive
//! widget and reports the key as unhandled — so it fell through to the webview
//! and text editing kept working. **Now that they are clickable, that is no
//! longer true**: an enabled item with `Ctrl+C` in the accel group takes the key
//! away from every text field in the app, and "copy" stops working in the
//! project name box the moment a clip is selected. So the four of them are shown
//! and not claimed, exactly like `Del` and `C`: the timeline binds them in the
//! webview, where the handler already declines to act when the focus is in a
//! field, and the menu only advertises the key in its label.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use super::paths;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

pub mod ids {
    pub const FILE_NEW: &str = "file.new";
    pub const FILE_OPEN: &str = "file.open";
    pub const FILE_SAVE: &str = "file.save";
    pub const FILE_SAVE_AS: &str = "file.save_as";
    pub const FILE_IMPORT: &str = "file.import";
    pub const FILE_EXPORT: &str = "file.export";
    pub const FILE_QUIT: &str = "file.quit";

    pub const EDIT_UNDO: &str = "edit.undo";
    pub const EDIT_REDO: &str = "edit.redo";
    pub const EDIT_CUT: &str = "edit.cut";
    pub const EDIT_COPY: &str = "edit.copy";
    pub const EDIT_PASTE: &str = "edit.paste";
    pub const EDIT_DUPLICATE: &str = "edit.duplicate";
    pub const EDIT_DELETE: &str = "edit.delete";
    pub const EDIT_SELECT_ALL: &str = "edit.select_all";
    pub const EDIT_SPLIT: &str = "edit.split";

    pub const VIEW_ZOOM_IN: &str = "view.zoom_in";
    pub const VIEW_ZOOM_OUT: &str = "view.zoom_out";
    pub const VIEW_ZOOM_FIT: &str = "view.zoom_fit";
    pub const VIEW_FULLSCREEN: &str = "view.fullscreen";
    pub const VIEW_LOGS: &str = "view.logs";

    pub const HELP_ABOUT: &str = "help.about";
    pub const HELP_SHORTCUTS: &str = "help.shortcuts";
    pub const HELP_DOCS: &str = "help.docs";
}

/// The event the webview listens on. One payload field, the item id.
pub const MENU_ACTION_EVENT: &str = "menu://action";

/// Raised for both File → Quit and the window's close button, because they are
/// the same action and must be guarded the same way.
pub const CLOSE_REQUESTED_EVENT: &str = "menu://close-requested";

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// What the frontend knows that the menu needs.
///
/// Deliberately facts and not decisions: "there is a clip under the playhead",
/// not "Split should be enabled". The decision is [`enablement`], on this side,
/// so there is one place to read when an item is greyed out unexpectedly.
///
/// snake_case on the wire, like the project document and `EditCommand`. See
/// `src/test/ipc-contract.test.ts` for why the boundary has two conventions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct MenuState {
    /// A document is open. Everything that edits or exports needs this.
    pub has_project: bool,
    /// It has edits that are not on disk.
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// At least one clip is selected, so there is something to delete, copy or
    /// link. How *many* is deliberately not here: no item in the bar changes
    /// its enabled state between one clip and four, and a count would be a
    /// second thing to keep in step for no gain.
    pub has_selection: bool,
    /// The playhead is inside a clip, so there is something to cut.
    pub can_split: bool,
    /// The timeline is on screen and has a span worth fitting to.
    pub can_fit: bool,
    /// Something has been cut or copied in this session, so Paste has material.
    ///
    /// The clipboard itself lives in the webview and is not part of the
    /// document — see `src/modules/timeline/lib/clipboard.ts` — so this is the
    /// only thing Rust knows about it, which is all the bar needs.
    pub has_clipboard: bool,
    /// There is at least one clip anywhere on the timeline, so Select All would
    /// select something.
    pub has_clips: bool,
}

/// What this *machine* offers, as opposed to what the document does.
///
/// Probed rather than assumed, and re-probed on every sync: file logging is
/// being added separately, and an item that is grey until the first log file
/// exists and then quietly becomes clickable is the honest behaviour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// A log directory exists and can be shown.
    pub logs: bool,
    /// The repository's `docs/` tree is reachable from here.
    pub docs: bool,
}

impl Capabilities {
    pub fn probe() -> Self {
        Self {
            logs: log_directory().is_some(),
            docs: docs_directory().is_some(),
        }
    }
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// The one thing that decides whether an item can be clicked.
///
/// No `Default`, on purpose: every item in [`ITEMS`] has to answer this
/// question explicitly, and "I forgot" is not one of the answers.
#[derive(Clone, Copy)]
pub enum Gate {
    /// Always clickable. Reserved for things that cannot fail to be meaningful
    /// — quitting, the about box.
    Always,
    /// Clickable when the document is in the right state.
    State(fn(&MenuState) -> bool),
    /// Clickable when the machine can do it at all.
    Capability(fn(&Capabilities) -> bool),
    /// Never clickable, because the functionality does not exist. The string is
    /// why, and it is the text that goes in the release notes when someone asks.
    Unimplemented(&'static str),
}

pub struct Item {
    pub id: &'static str,
    pub label: &'static str,
    /// muda accelerator syntax. `None` where the key is bare and would shadow
    /// typing — see the module docs.
    pub accelerator: Option<&'static str>,
    pub gate: Gate,
}

pub enum Entry {
    Item(Item),
    Separator,
}

/// A whole submenu, in the order it is drawn.
pub struct MenuSection {
    pub title: &'static str,
    pub entries: &'static [Entry],
}

const fn item(id: &'static str, label: &'static str, accel: Option<&'static str>, gate: Gate) -> Entry {
    Entry::Item(Item {
        id,
        label,
        accelerator: accel,
        gate,
    })
}

/// The whole bar. Read top to bottom, this is the answer to "what does chukcut
/// claim it can do".
pub const ITEMS: &[MenuSection] = &[
    MenuSection {
        title: "File",
        entries: &[
            item(ids::FILE_NEW, "New Project", Some("CmdOrCtrl+N"), Gate::Always),
            item(ids::FILE_OPEN, "Open Project…", Some("CmdOrCtrl+O"), Gate::Always),
            Entry::Separator,
            // Greyed when there is nothing to write. A Save that is a no-op
            // still teaches the user that Save sometimes does nothing.
            item(ids::FILE_SAVE, "Save", Some("CmdOrCtrl+S"), Gate::State(|s| s.has_project && s.dirty)),
            item(ids::FILE_SAVE_AS, "Save As…", Some("CmdOrCtrl+Shift+S"), Gate::State(|s| s.has_project)),
            Entry::Separator,
            item(ids::FILE_IMPORT, "Import Media…", Some("CmdOrCtrl+I"), Gate::State(|s| s.has_project)),
            item(ids::FILE_EXPORT, "Export…", Some("CmdOrCtrl+E"), Gate::State(|s| s.has_project)),
            Entry::Separator,
            item(ids::FILE_QUIT, "Quit", Some("CmdOrCtrl+Q"), Gate::Always),
        ],
    },
    MenuSection {
        title: "Edit",
        entries: &[
            item(ids::EDIT_UNDO, "Undo", Some("CmdOrCtrl+Z"), Gate::State(|s| s.can_undo)),
            item(ids::EDIT_REDO, "Redo", Some("CmdOrCtrl+Shift+Z"), Gate::State(|s| s.can_redo)),
            Entry::Separator,
            // The clipboard holds detached clips and lives in the webview, so
            // Cut and Copy need something selected and Paste needs something
            // copied. None of the four claims its accelerator: an enabled GTK
            // item would take Ctrl+C away from every text field in the app. The
            // timeline binds them and the label advertises them; see the
            // module docs.
            item(ids::EDIT_CUT, "Cut (Ctrl+X)", None, Gate::State(|s| s.has_selection)),
            item(ids::EDIT_COPY, "Copy (Ctrl+C)", None, Gate::State(|s| s.has_selection)),
            item(ids::EDIT_PASTE, "Paste (Ctrl+V)", None, Gate::State(|s| s.has_project && s.has_clipboard)),
            item(ids::EDIT_DUPLICATE, "Duplicate (Ctrl+D)", None, Gate::State(|s| s.has_selection)),
            Entry::Separator,
            // Del and C are the timeline's own bindings and are shown, not
            // claimed. Registering them would stop both keys reaching a text
            // field. See the module docs.
            item(ids::EDIT_DELETE, "Delete Clip (Del)", None, Gate::State(|s| s.has_selection)),
            item(
                ids::EDIT_SELECT_ALL,
                "Select All (Ctrl+A)",
                None,
                // Not `has_project` alone: selecting everything on an empty
                // timeline lights up Delete and Copy for a selection of nothing.
                Gate::State(|s| s.has_project && s.has_clips),
            ),
            item(ids::EDIT_SPLIT, "Split Clip (C)", None, Gate::State(|s| s.can_split)),
        ],
    },
    MenuSection {
        title: "View",
        entries: &[
            item(ids::VIEW_ZOOM_IN, "Zoom In", Some("CmdOrCtrl+="), Gate::State(|s| s.has_project)),
            item(ids::VIEW_ZOOM_OUT, "Zoom Out", Some("CmdOrCtrl+-"), Gate::State(|s| s.has_project)),
            item(ids::VIEW_ZOOM_FIT, "Fit Timeline", Some("CmdOrCtrl+0"), Gate::State(|s| s.can_fit)),
            Entry::Separator,
            // F is the preview's binding and is shown rather than claimed; the
            // action itself is real window fullscreen either way.
            item(ids::VIEW_FULLSCREEN, "Toggle Fullscreen (F)", None, Gate::Always),
            item(ids::VIEW_LOGS, "Show Log Directory", None, Gate::Capability(|c| c.logs)),
        ],
    },
    MenuSection {
        title: "Help",
        entries: &[
            item(ids::HELP_ABOUT, "About chukcut", None, Gate::Always),
            item(ids::HELP_SHORTCUTS, "Keyboard Shortcuts", None, Gate::Always),
            // The docs are markdown in the repository. There is no published
            // copy and nothing is bundled into the app, so this can only be
            // offered from a source checkout.
            item(ids::HELP_DOCS, "Documentation", None, Gate::Capability(|c| c.docs)),
        ],
    },
];

/// Every item in the bar, in order, flattened out of the sections.
pub fn all_items() -> impl Iterator<Item = &'static Item> {
    ITEMS.iter().flat_map(|section| {
        section.entries.iter().filter_map(|entry| match entry {
            Entry::Item(item) => Some(item),
            Entry::Separator => None,
        })
    })
}

/// Why an item can never be clicked, for the items where the answer is "the
/// feature does not exist". `None` for everything that is merely unavailable
/// right now.
pub fn unavailable_reason(id: &str) -> Option<&'static str> {
    all_items()
        .find(|item| item.id == id)
        .and_then(|item| match item.gate {
            Gate::Unimplemented(reason) => Some(reason),
            _ => None,
        })
}

/// Which items are clickable, given what the document and the machine can do.
///
/// The decision, in one pure function, so that "why is Save grey" is answered
/// by reading twenty lines rather than by running the app.
pub fn enablement(state: &MenuState, capabilities: &Capabilities) -> BTreeMap<&'static str, bool> {
    all_items()
        .map(|item| {
            let enabled = match item.gate {
                Gate::Always => true,
                Gate::State(decide) => decide(state),
                Gate::Capability(decide) => decide(capabilities),
                Gate::Unimplemented(_) => false,
            };
            (item.id, enabled)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Building and updating
// ---------------------------------------------------------------------------

/// The live items, kept so an update is one call per item rather than a
/// recursive walk of the bar. `Menu::get` only looks at the top level, and
/// every one of our items is a level below that.
pub struct MenuHandles<R: Runtime> {
    items: BTreeMap<&'static str, MenuItem<R>>,
}

impl<R: Runtime> MenuHandles<R> {
    fn set_enabled(&self, id: &str, enabled: bool) {
        if let Some(item) = self.items.get(id) {
            if let Err(error) = item.set_enabled(enabled) {
                tracing::warn!(%error, id, "could not update a menu item");
            }
        }
    }
}

/// Build the bar and register the handles for later updates.
///
/// The initial state is "nothing is open", which is true at launch: the
/// frontend has not asked Rust what it has yet. The first
/// `workspace_menu_sync` corrects it a few milliseconds later.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let capabilities = Capabilities::probe();
    let initial = MenuState::default();
    let enabled = enablement(&initial, &capabilities);

    let mut handles: BTreeMap<&'static str, MenuItem<R>> = BTreeMap::new();
    let menu = Menu::new(app)?;

    for section in ITEMS {
        let submenu = Submenu::new(app, section.title, true)?;
        for entry in section.entries {
            match entry {
                Entry::Separator => submenu.append(&PredefinedMenuItem::separator(app)?)?,
                Entry::Item(spec) => {
                    let built = MenuItem::with_id(
                        app,
                        spec.id,
                        spec.label,
                        enabled.get(spec.id).copied().unwrap_or(false),
                        spec.accelerator,
                    )?;
                    submenu.append(&built)?;
                    handles.insert(spec.id, built);
                }
            }
        }
        menu.append(&submenu)?;
    }

    // `manage` answers whether this was the first one. A second call would mean
    // the bar was built twice, which nothing does.
    let _ = app.manage(MenuHandles { items: handles });
    Ok(menu)
}

/// Push the frontend's view of the world onto the bar.
pub fn apply<R: Runtime>(app: &AppHandle<R>, state: &MenuState) {
    let Some(handles) = app.try_state::<MenuHandles<R>>() else {
        // No menu was built — a headless test, or a platform where the bar was
        // not installed. Not an error, and not worth a message.
        return;
    };
    for (id, enabled) in enablement(state, &Capabilities::probe()) {
        handles.set_enabled(id, enabled);
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// What the webview is told when an item is clicked.
#[derive(Debug, Clone, Serialize)]
pub struct MenuAction {
    pub id: String,
}

/// Route a click.
///
/// Most items belong to the frontend, because that is where the document, the
/// dialogs and the file pickers live. The handful Rust owns are the ones that
/// are about the machine rather than the document.
pub fn handle_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id().0.as_str();

    match id {
        ids::FILE_QUIT => request_close(app),
        ids::VIEW_LOGS => reveal(log_directory(), "log directory"),
        ids::HELP_DOCS => reveal(docs_directory(), "documentation"),
        ids::HELP_ABOUT => show_about(app),
        _ => {
            if let Err(error) = app.emit(MENU_ACTION_EVENT, MenuAction { id: id.to_string() }) {
                tracing::warn!(%error, id, "could not deliver a menu action to the webview");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Quitting
// ---------------------------------------------------------------------------

/// Whether a quit is already waiting on the user's answer.
///
/// Without this, clicking the close button twice while the prompt is up raises
/// a second prompt behind the first, and answering one strands the other.
static CLOSE_PENDING: AtomicBool = AtomicBool::new(false);

/// Rust's half of the quit handshake, as a value so it can be tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Ask the webview to run its unsaved-changes guard.
    Ask,
    /// A question is already on screen. Do nothing and let it be answered.
    AlreadyAsking,
}

/// Claim the right to ask. Returns [`CloseAction::AlreadyAsking`] when someone
/// else already has it.
pub fn begin_close() -> CloseAction {
    if CLOSE_PENDING.swap(true, Ordering::SeqCst) {
        CloseAction::AlreadyAsking
    } else {
        CloseAction::Ask
    }
}

/// Release the claim, so a later close request asks again. Called when the user
/// cancels — and only then, since a confirmed close destroys the window.
pub fn end_close() {
    CLOSE_PENDING.store(false, Ordering::SeqCst);
}

/// Ask the webview whether it is safe to go.
///
/// Nothing closes here. The window stays up until the frontend has run
/// `guardUnsaved` and answered through `workspace_close_answer`, which is the
/// only arrangement in which Cancel genuinely aborts: a `close()` that has
/// already started cannot be taken back.
pub fn request_close<R: Runtime, M: Manager<R>>(manager: &M) {
    if begin_close() == CloseAction::AlreadyAsking {
        return;
    }
    let app = manager.app_handle();
    if let Err(error) = app.emit(CLOSE_REQUESTED_EVENT, ()) {
        // The webview cannot be reached, so nothing will ever answer. Closing
        // is then the lesser evil — the alternative is a window that cannot be
        // shut at all.
        tracing::error!(%error, "could not ask the webview about unsaved changes; closing anyway");
        end_close();
        app.exit(0);
    }
}

// ---------------------------------------------------------------------------
// The things Rust does itself
// ---------------------------------------------------------------------------

fn reveal(path: Option<PathBuf>, what: &str) {
    let Some(path) = path else {
        // The item is greyed whenever this is `None`, so arriving here means
        // the directory went away between the last sync and the click.
        tracing::warn!(what, "nothing to show: the directory is gone");
        return;
    };
    if let Err(error) = tauri_plugin_opener::reveal_item_in_dir(&path) {
        tracing::warn!(%error, path = %path.display(), "could not open the file manager");
    }
}

fn show_about<R: Runtime>(app: &AppHandle<R>) {
    use tauri_plugin_dialog::DialogExt;

    app.dialog()
        .message(format!(
            "chukcut {}\n\nA video editor with a Rust engine and a webview UI.\n\nRust owns the machine, the webview owns the pixels.",
            env!("CARGO_PKG_VERSION")
        ))
        .title("About chukcut")
        .show(|_| {});
}

/// What "Show Log Directory" would reveal, or `None` when there is nothing to
/// show yet.
///
/// The current log *file* is preferred over its directory because revealing a
/// file selects it, which is strictly more useful than dropping the user in a
/// folder of seven dailies. When file logging could not start — a read-only
/// home, most likely — `logging::log_file` is `None` and the directory itself
/// only counts if it exists. Both being absent is exactly the case the item
/// must be grey for.
pub fn log_directory() -> Option<PathBuf> {
    if let Some(file) = super::logging::log_file() {
        if file.exists() {
            return Some(file);
        }
    }
    let directory = paths::logs_dir();
    directory.is_dir().then_some(directory)
}

/// The repository's `docs/` tree, when the app is running out of a checkout.
///
/// Nothing under `docs/` is bundled into a build, so in a shipped app this is
/// `None` and the Help item is grey. That is the honest answer: there is no
/// published documentation to point at.
pub fn docs_directory() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(PathBuf::from));
    }
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().map(PathBuf::from));
    }

    roots
        .into_iter()
        .map(|dir| dir.join("docs"))
        // A directory called `docs` is not enough — the marker is a file we
        // know we wrote, so a `docs` folder belonging to something else on the
        // path above the executable cannot be mistaken for ours.
        .find(|docs| docs.join("architecture").join("overview.md").is_file())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A document with everything available, so a test only has to say what is
    /// *missing* from it.
    fn everything() -> MenuState {
        MenuState {
            has_project: true,
            dirty: true,
            can_undo: true,
            can_redo: true,
            has_selection: true,
            can_split: true,
            can_fit: true,
            has_clipboard: true,
            has_clips: true,
        }
    }

    /// Every state the nine booleans can be in. Used by the checks that are
    /// about what may *never* happen, where one example is not evidence.
    fn every_state() -> impl Iterator<Item = MenuState> {
        (0u16..512).map(|bits| MenuState {
            has_project: bits & 1 != 0,
            dirty: bits & 2 != 0,
            can_undo: bits & 4 != 0,
            can_redo: bits & 8 != 0,
            has_selection: bits & 16 != 0,
            can_split: bits & 32 != 0,
            can_fit: bits & 64 != 0,
            has_clipboard: bits & 128 != 0,
            has_clips: bits & 256 != 0,
        })
    }

    fn is_enabled(state: &MenuState, id: &str) -> bool {
        enablement(state, &Capabilities::default())[id]
    }

    #[test]
    fn ids_are_unique() {
        let mut seen = BTreeMap::new();
        for item in all_items() {
            assert!(
                seen.insert(item.id, item.label).is_none(),
                "{} appears twice in the bar",
                item.id
            );
        }
    }

    #[test]
    fn every_item_gets_a_decision() {
        let decided = enablement(&everything(), &Capabilities::default());
        for item in all_items() {
            assert!(
                decided.contains_key(item.id),
                "{} is in the bar but nothing decides whether it is clickable",
                item.id
            );
        }
        assert_eq!(decided.len(), all_items().count());
    }

    #[test]
    fn nothing_is_clickable_with_no_document_except_what_does_not_need_one() {
        let empty = MenuState::default();
        // New and Open are how you get a document, so they cannot depend on
        // having one; Quit and About are about the app, not the work.
        for id in [ids::FILE_NEW, ids::FILE_OPEN, ids::FILE_QUIT, ids::HELP_ABOUT] {
            assert!(is_enabled(&empty, id), "{id} must work with nothing open");
        }
        for id in [
            ids::FILE_SAVE,
            ids::FILE_SAVE_AS,
            ids::FILE_IMPORT,
            ids::FILE_EXPORT,
            ids::EDIT_UNDO,
            ids::EDIT_REDO,
            ids::EDIT_DELETE,
            ids::EDIT_SPLIT,
            ids::VIEW_ZOOM_IN,
            ids::VIEW_ZOOM_OUT,
            ids::VIEW_ZOOM_FIT,
        ] {
            assert!(!is_enabled(&empty, id), "{id} must be grey with nothing open");
        }
    }

    #[test]
    fn save_is_grey_until_there_is_something_to_write() {
        let clean = MenuState {
            dirty: false,
            ..everything()
        };
        assert!(!is_enabled(&clean, ids::FILE_SAVE));
        // Save As is a different question: writing a copy of an unchanged
        // document is a perfectly ordinary thing to want.
        assert!(is_enabled(&clean, ids::FILE_SAVE_AS));
        assert!(is_enabled(&everything(), ids::FILE_SAVE));
    }

    #[test]
    fn undo_and_redo_follow_the_history_rather_than_the_document() {
        let no_history = MenuState {
            can_undo: false,
            can_redo: false,
            ..everything()
        };
        assert!(!is_enabled(&no_history, ids::EDIT_UNDO));
        assert!(!is_enabled(&no_history, ids::EDIT_REDO));

        let undo_only = MenuState {
            can_redo: false,
            ..everything()
        };
        assert!(is_enabled(&undo_only, ids::EDIT_UNDO));
        assert!(!is_enabled(&undo_only, ids::EDIT_REDO));
    }

    #[test]
    fn delete_needs_a_selection_and_split_needs_a_clip_under_the_playhead() {
        let nothing_selected = MenuState {
            has_selection: false,
            can_split: false,
            ..everything()
        };
        assert!(!is_enabled(&nothing_selected, ids::EDIT_DELETE));
        assert!(!is_enabled(&nothing_selected, ids::EDIT_SPLIT));

        // The two are independent: the playhead can sit inside a clip that is
        // not the selected one.
        let over_a_clip = MenuState {
            has_selection: false,
            ..everything()
        };
        assert!(!is_enabled(&over_a_clip, ids::EDIT_DELETE));
        assert!(is_enabled(&over_a_clip, ids::EDIT_SPLIT));
    }

    #[test]
    fn nothing_in_the_bar_is_permanently_unimplemented() {
        // The four clipboard items were `Gate::Unimplemented` until the
        // timeline learned multi-selection and grew a clipboard. Nothing in the
        // bar claims a feature that does not exist any more, and this is what
        // notices if something is added that does.
        let stubs: Vec<&str> = all_items()
            .filter(|item| unavailable_reason(item.id).is_some())
            .map(|item| item.id)
            .collect();
        assert!(stubs.is_empty(), "these items are wired to nothing: {stubs:?}");
    }

    #[test]
    fn the_clipboard_items_follow_the_selection_and_the_clipboard() {
        // Cut and Copy take what is selected; Paste puts back what was taken.
        // The two are independent — copying in one project and pasting into
        // another is the whole reason the clipboard outlives a document.
        let nothing_selected = MenuState {
            has_selection: false,
            ..everything()
        };
        assert!(!is_enabled(&nothing_selected, ids::EDIT_CUT));
        assert!(!is_enabled(&nothing_selected, ids::EDIT_COPY));
        assert!(!is_enabled(&nothing_selected, ids::EDIT_DUPLICATE));
        assert!(is_enabled(&nothing_selected, ids::EDIT_PASTE));

        let nothing_copied = MenuState {
            has_clipboard: false,
            ..everything()
        };
        assert!(is_enabled(&nothing_copied, ids::EDIT_CUT));
        assert!(!is_enabled(&nothing_copied, ids::EDIT_PASTE));
    }

    #[test]
    fn select_all_needs_something_to_select() {
        let empty_timeline = MenuState {
            has_clips: false,
            ..everything()
        };
        // A Select All that selects nothing would then light up Delete, Copy
        // and Link for a selection of no clips.
        assert!(!is_enabled(&empty_timeline, ids::EDIT_SELECT_ALL));
        assert!(is_enabled(&everything(), ids::EDIT_SELECT_ALL));
    }

    #[test]
    fn nothing_that_edits_a_document_is_clickable_without_one() {
        // Exhaustive over the nine booleans: the frontend describes a closed
        // document as all-false, but a stale flag arriving with `has_project`
        // false must not be able to light up an editing item either.
        for state in every_state().filter(|state| !state.has_project) {
            for capabilities in [
                Capabilities::default(),
                Capabilities {
                    logs: true,
                    docs: true,
                },
            ] {
                let decided = enablement(&state, &capabilities);
                for id in [ids::EDIT_PASTE, ids::EDIT_SELECT_ALL, ids::FILE_EXPORT] {
                    assert!(!decided[id], "{id} must be grey with nothing open");
                }
            }
        }
    }

    #[test]
    fn the_machine_decides_the_log_and_docs_items_and_the_document_does_not() {
        let present = Capabilities {
            logs: true,
            docs: true,
        };
        let absent = Capabilities::default();

        // Not a document question: a log directory is just as showable with
        // nothing open as with a full timeline.
        for state in [MenuState::default(), everything()] {
            assert!(enablement(&state, &present)[ids::VIEW_LOGS]);
            assert!(enablement(&state, &present)[ids::HELP_DOCS]);
            assert!(!enablement(&state, &absent)[ids::VIEW_LOGS]);
            assert!(!enablement(&state, &absent)[ids::HELP_DOCS]);
        }
    }

    #[test]
    fn fullscreen_does_not_need_a_document() {
        assert!(is_enabled(&MenuState::default(), ids::VIEW_FULLSCREEN));
    }

    #[test]
    fn a_second_close_request_does_not_raise_a_second_prompt() {
        end_close();
        assert_eq!(begin_close(), CloseAction::Ask);
        assert_eq!(begin_close(), CloseAction::AlreadyAsking);
        assert_eq!(begin_close(), CloseAction::AlreadyAsking);

        // Cancelling puts the app back where it was, so the next attempt asks.
        end_close();
        assert_eq!(begin_close(), CloseAction::Ask);
        end_close();
    }

    #[test]
    fn keys_the_app_already_binds_are_shown_and_not_claimed() {
        // Registering these as accelerators would take the key away from every
        // text field in the app; see the module docs. That is obvious for the
        // bare keys and easy to get wrong for the clipboard four, which carried
        // Ctrl+X/C/V/A safely for exactly as long as they were disabled. The
        // guard is that the label says what the key is and the accelerator
        // stays empty.
        for id in [
            ids::EDIT_DELETE,
            ids::EDIT_SPLIT,
            ids::VIEW_FULLSCREEN,
            ids::EDIT_CUT,
            ids::EDIT_COPY,
            ids::EDIT_PASTE,
            ids::EDIT_DUPLICATE,
            ids::EDIT_SELECT_ALL,
        ] {
            let item = all_items().find(|item| item.id == id).unwrap();
            assert!(
                item.accelerator.is_none(),
                "{id} must not register a bare-key accelerator"
            );
            assert!(
                item.label.contains('('),
                "{id} must show the key it does not claim"
            );
        }
    }

    #[test]
    fn every_accelerator_carries_a_modifier() {
        for item in all_items() {
            let Some(accelerator) = item.accelerator else {
                continue;
            };
            assert!(
                accelerator.contains('+'),
                "{} binds {accelerator}, which would shadow typing",
                item.id
            );
        }
    }
}

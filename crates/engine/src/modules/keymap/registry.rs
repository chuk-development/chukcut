//! Every action the app can bind to a key, with its group, its name, and its
//! keys in each preset.
//!
//! The ids are the app's action names without the `chukcut::` namespace
//! (`crates/app`'s `actions!` lists). The app maps each id to its action and
//! a test there checks that every id here has one; nothing else in the app
//! lists keys any more.

use serde::Serialize;

/// The presets, in the order the settings offer them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Ours: CapCut's layout plus J/K/L, I/O and the usual Ctrl keys.
    #[default]
    Chukcut,
    /// As close to CapCut desktop's defaults as our actions allow.
    Capcut,
    /// As close to Premiere Pro's defaults as our actions allow.
    Premiere,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Chukcut, Preset::Capcut, Preset::Premiere];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Chukcut => "chukcut",
            Preset::Capcut => "CapCut-like",
            Preset::Premiere => "Premiere-like",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Preset::Chukcut => "chukcut",
            Preset::Capcut => "capcut",
            Preset::Premiere => "premiere",
        }
    }

    pub fn from_id(id: &str) -> Option<Preset> {
        Preset::ALL
            .into_iter()
            .find(|p| p.id().eq_ignore_ascii_case(id.trim()))
    }
}

/// The groups, in the order the sheet and the editor show them.
pub const GROUPS: [&str; 7] = [
    "Playback", "Editing", "Timeline", "Clip", "File", "App", "Analysis",
];

/// One bindable action.
#[derive(Debug, Clone, Serialize)]
pub struct ActionSpec {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    /// The keys stay out of text fields even with a modifier: Ctrl+C in a
    /// field copies its text, not the selected clips. A plain key is always
    /// kept out of text fields, whatever this says.
    pub typing_off: bool,
    pub chukcut: &'static [&'static str],
    pub capcut: &'static [&'static str],
    pub premiere: &'static [&'static str],
}

impl ActionSpec {
    pub fn defaults(&self, preset: Preset) -> &'static [&'static str] {
        match preset {
            Preset::Chukcut => self.chukcut,
            Preset::Capcut => self.capcut,
            Preset::Premiere => self.premiere,
        }
    }
}

/// `a!(id, group, label, typing_off, [chukcut], [capcut], [premiere])`.
macro_rules! a {
    ($id:literal, $group:literal, $label:literal, $off:literal,
     [$($c:literal),*], [$($k:literal),*], [$($p:literal),*]) => {
        ActionSpec {
            id: $id,
            group: $group,
            label: $label,
            typing_off: $off,
            chukcut: &[$($c),*],
            capcut: &[$($k),*],
            premiere: &[$($p),*],
        }
    };
}

/// Every action, in the order the editor lists them within a group.
pub const ACTIONS: &[ActionSpec] = &[
    // --- Playback ---------------------------------------------------------
    a!(
        "PlayPause",
        "Playback",
        "Play / pause",
        false,
        ["space"],
        ["space"],
        ["space"]
    ),
    a!(
        "ShuttleForward",
        "Playback",
        "Shuttle forward (again: 2\u{d7}, 4\u{d7}, 8\u{d7})",
        false,
        ["l"],
        ["l"],
        ["l"]
    ),
    a!(
        "ShuttleBack",
        "Playback",
        "Shuttle backward",
        false,
        ["j"],
        ["j"],
        ["j"]
    ),
    a!(
        "ShuttleStop",
        "Playback",
        "Stop",
        false,
        ["k"],
        ["k"],
        ["k"]
    ),
    a!(
        "StepBack",
        "Playback",
        "Previous frame",
        false,
        ["left"],
        ["left"],
        ["left"]
    ),
    a!(
        "StepForward",
        "Playback",
        "Next frame",
        false,
        ["right"],
        ["right"],
        ["right"]
    ),
    a!(
        "StepBack10",
        "Playback",
        "Back 10 frames",
        true,
        ["shift-left"],
        ["shift-left"],
        ["shift-left"]
    ),
    a!(
        "StepForward10",
        "Playback",
        "Forward 10 frames",
        true,
        ["shift-right"],
        ["shift-right"],
        ["shift-right"]
    ),
    a!(
        "GoToStart",
        "Playback",
        "Go to start",
        false,
        ["home"],
        ["home"],
        ["home"]
    ),
    a!(
        "GoToEnd",
        "Playback",
        "Go to end",
        false,
        ["end"],
        ["end"],
        ["end"]
    ),
    a!("MarkIn", "Playback", "Mark in", false, ["i"], ["i"], ["i"]),
    a!(
        "MarkOut",
        "Playback",
        "Mark out",
        false,
        ["o"],
        ["o"],
        ["o"]
    ),
    a!(
        "ClearInOut",
        "Playback",
        "Clear in and out",
        true,
        ["alt-x"],
        ["alt-x"],
        ["ctrl-shift-x"]
    ),
    a!(
        "ToggleLoop",
        "Playback",
        "Loop playback of in to out",
        false,
        ["ctrl-l"],
        ["ctrl-l"],
        ["ctrl-l"]
    ),
    // --- Editing ----------------------------------------------------------
    a!(
        "Split",
        "Editing",
        "Split at the playhead",
        false,
        ["s", "ctrl-b"],
        ["ctrl-b"],
        ["ctrl-k"]
    ),
    a!(
        "DeleteSelected",
        "Editing",
        "Delete selection",
        true,
        ["delete", "backspace"],
        ["delete", "backspace"],
        ["delete", "backspace"]
    ),
    a!(
        "DeleteLeft",
        "Editing",
        "Delete left of the playhead",
        false,
        ["q"],
        ["q"],
        ["q"]
    ),
    a!(
        "DeleteRight",
        "Editing",
        "Delete right of the playhead",
        false,
        ["w"],
        ["w"],
        ["w"]
    ),
    a!(
        "Undo",
        "Editing",
        "Undo",
        false,
        ["ctrl-z"],
        ["ctrl-z"],
        ["ctrl-z"]
    ),
    a!(
        "Redo",
        "Editing",
        "Redo",
        false,
        ["ctrl-shift-z", "ctrl-y"],
        ["ctrl-shift-z"],
        ["ctrl-shift-z"]
    ),
    a!(
        "CopyClips",
        "Editing",
        "Copy",
        true,
        ["ctrl-c"],
        ["ctrl-c"],
        ["ctrl-c"]
    ),
    a!(
        "CutClips",
        "Editing",
        "Cut",
        true,
        ["ctrl-x"],
        ["ctrl-x"],
        ["ctrl-x"]
    ),
    a!(
        "PasteClips",
        "Editing",
        "Paste",
        true,
        ["ctrl-v"],
        ["ctrl-v"],
        ["ctrl-v"]
    ),
    a!(
        "DuplicateClips",
        "Editing",
        "Duplicate",
        true,
        ["ctrl-d"],
        ["ctrl-d"],
        ["ctrl-shift-/"]
    ),
    a!(
        "SelectAllClips",
        "Editing",
        "Select all clips",
        true,
        ["ctrl-a"],
        ["ctrl-a"],
        ["ctrl-a"]
    ),
    a!(
        "ClearSelection",
        "Editing",
        "Clear the selection",
        true,
        ["escape"],
        ["escape"],
        ["ctrl-shift-a"]
    ),
    // --- Timeline ---------------------------------------------------------
    a!(
        "ToggleMarker",
        "Timeline",
        "Add or remove a marker",
        false,
        ["m"],
        ["m"],
        ["m"]
    ),
    a!(
        "ToggleMagnet",
        "Timeline",
        "Main track magnet",
        false,
        ["p"],
        ["p"],
        ["alt-p"]
    ),
    a!(
        "ToggleSnapping",
        "Timeline",
        "Snapping",
        false,
        ["n"],
        ["n"],
        ["s"]
    ),
    a!(
        "SelectTool",
        "Timeline",
        "Select tool",
        false,
        ["a"],
        ["a"],
        ["v"]
    ),
    a!(
        "BladeTool",
        "Timeline",
        "Blade tool",
        false,
        ["b"],
        ["b"],
        ["c"]
    ),
    a!(
        "ZoomIn",
        "Timeline",
        "Zoom in",
        false,
        ["ctrl-=", "ctrl-+", "ctrl-shift-="],
        ["ctrl-=", "ctrl-+"],
        ["="]
    ),
    a!(
        "ZoomOut",
        "Timeline",
        "Zoom out",
        false,
        ["ctrl--"],
        ["ctrl--"],
        ["-"]
    ),
    a!(
        "ZoomToFit",
        "Timeline",
        "Zoom to fit",
        false,
        ["shift-z"],
        ["shift-z"],
        ["\\"]
    ),
    // --- Clip -------------------------------------------------------------
    a!("DetachAudio", "Clip", "Detach audio", false, [], [], []),
    a!("LinkClips", "Clip", "Link clips", false, [], [], []),
    a!("UnlinkClips", "Clip", "Unlink clips", false, [], [], []),
    a!("ResetSpeed", "Clip", "Reset speed", false, [], [], []),
    a!(
        "FreezeFrame",
        "Clip",
        "Freeze frame",
        false,
        [],
        [],
        ["shift-f"]
    ),
    a!("ReplaceMedia", "Clip", "Replace media", false, [], [], []),
    a!(
        "DuckUnderSpeech",
        "Clip",
        "Duck music under speech",
        false,
        [],
        [],
        []
    ),
    a!("RemoveDucking", "Clip", "Remove ducking", false, [], [], []),
    a!(
        "ToggleVoiceover",
        "Clip",
        "Record a voiceover",
        false,
        [],
        [],
        []
    ),
    a!(
        "CreateCompound",
        "Clip",
        "Create compound clip",
        true,
        ["alt-g"],
        ["alt-g"],
        ["alt-g"]
    ),
    a!(
        "FlattenCompound",
        "Clip",
        "Flatten compound clip",
        true,
        ["alt-shift-g"],
        ["alt-shift-g"],
        ["alt-shift-g"]
    ),
    a!(
        "OpenCompound",
        "Clip",
        "Open compound clip",
        false,
        [],
        [],
        []
    ),
    a!(
        "CloseCompound",
        "Clip",
        "Close compound clip",
        false,
        [],
        [],
        []
    ),
    a!("NewTimeline", "Timeline", "New timeline", false, [], [], []),
    // --- File -------------------------------------------------------------
    a!(
        "NewProject",
        "File",
        "New project",
        false,
        ["ctrl-n"],
        ["ctrl-n"],
        ["ctrl-alt-n"]
    ),
    a!(
        "Open",
        "File",
        "Open project",
        false,
        ["ctrl-o"],
        ["ctrl-o"],
        ["ctrl-o"]
    ),
    a!(
        "Save",
        "File",
        "Save",
        false,
        ["ctrl-s"],
        ["ctrl-s"],
        ["ctrl-s"]
    ),
    a!(
        "Import",
        "File",
        "Import media",
        false,
        ["ctrl-i"],
        ["ctrl-i"],
        ["ctrl-i"]
    ),
    a!(
        "Export",
        "File",
        "Export",
        false,
        ["ctrl-e"],
        ["ctrl-e"],
        ["ctrl-m"]
    ),
    a!(
        "Quit",
        "File",
        "Quit",
        false,
        ["ctrl-q"],
        ["ctrl-q"],
        ["ctrl-q"]
    ),
    // --- App --------------------------------------------------------------
    a!(
        "OpenSettings",
        "App",
        "Settings",
        false,
        ["ctrl-,"],
        ["ctrl-,"],
        ["ctrl-,"]
    ),
    a!(
        "ShowShortcuts",
        "App",
        "Keyboard shortcuts",
        false,
        ["ctrl-/", "shift-/", "?"],
        ["ctrl-/"],
        ["ctrl-alt-k"]
    ),
    // --- Analysis ---------------------------------------------------------
    a!(
        "DetectScenes",
        "Analysis",
        "Detect scenes",
        false,
        [],
        [],
        []
    ),
    a!(
        "SplitAtScenes",
        "Analysis",
        "Split at scene changes",
        false,
        [],
        [],
        []
    ),
    a!("StabiliseClip", "Analysis", "Stabilise", false, [], [], []),
    a!("DetectBeats", "Analysis", "Detect beats", false, [], [], []),
    a!(
        "AutoCutToBeat",
        "Analysis",
        "Cut to the beat",
        false,
        [],
        [],
        []
    ),
    a!(
        "SnapCutsToBeats",
        "Analysis",
        "Snap cuts to beats",
        false,
        [],
        [],
        []
    ),
    a!("AutoReframe", "Analysis", "Auto reframe", false, [], [], []),
    a!(
        "ReframeVertical",
        "Analysis",
        "Reframe to vertical",
        false,
        [],
        [],
        []
    ),
    a!(
        "ReframeLandscape",
        "Analysis",
        "Reframe to landscape",
        false,
        [],
        [],
        []
    ),
    a!(
        "CancelAnalysis",
        "Analysis",
        "Cancel analysis",
        false,
        [],
        [],
        []
    ),
];

/// The action `id`.
pub fn action(id: &str) -> Option<&'static ActionSpec> {
    ACTIONS.iter().find(|a| a.id == id)
}

//! The keyboard shortcuts sheet (Ctrl+/ or ?), and the app-level actions
//! that open it and the other shell surfaces.
//!
//! The sheet is generated from the live keymap — every binding in the
//! `chukcut` namespace, whoever registered it — so it cannot drift from what
//! the keys actually do. The table below only gives actions a group and a
//! readable name; an action missing from it still appears, under "Other",
//! with a name made from its identifier.

use std::collections::BTreeMap;

use gpui::component::kbd::Kbd;
use gpui::component::WindowExt as _;
use gpui::{AnyElement, AsKeystroke as _, KeyBinding};

use super::*;

actions!(chukcut, [NewProject, OpenSettings, ShowShortcuts]);

pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    const TYPING_OFF: Option<&str> = Some("!Input");
    vec![
        KeyBinding::new("ctrl-n", NewProject, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
        KeyBinding::new("ctrl-/", ShowShortcuts, None),
        KeyBinding::new("shift-/", ShowShortcuts, TYPING_OFF),
        KeyBinding::new("?", ShowShortcuts, TYPING_OFF),
    ]
}

/// Groups in the order the sheet shows them.
const GROUPS: [&str; 6] = ["Playback", "Editing", "Timeline", "File", "App", "Other"];

/// `(action, group, label)` for every action we know.
const NAMES: &[(&str, &str, &str)] = &[
    ("PlayPause", "Playback", "Play / pause"),
    (
        "ShuttleForward",
        "Playback",
        "Shuttle forward (again: 2\u{d7}, 4\u{d7}, 8\u{d7})",
    ),
    ("ShuttleBack", "Playback", "Shuttle backward"),
    ("ShuttleStop", "Playback", "Stop"),
    ("StepBack", "Playback", "Previous frame"),
    ("StepForward", "Playback", "Next frame"),
    ("StepBack10", "Playback", "Back 10 frames"),
    ("StepForward10", "Playback", "Forward 10 frames"),
    ("GoToStart", "Playback", "Go to start"),
    ("GoToEnd", "Playback", "Go to end"),
    ("MarkIn", "Playback", "Mark in"),
    ("MarkOut", "Playback", "Mark out"),
    ("ClearInOut", "Playback", "Clear in and out"),
    ("ToggleLoop", "Playback", "Loop playback of in to out"),
    ("Split", "Editing", "Split at the playhead"),
    ("DeleteSelected", "Editing", "Delete selection"),
    ("DeleteLeft", "Editing", "Delete left of the playhead"),
    ("DeleteRight", "Editing", "Delete right of the playhead"),
    ("Undo", "Editing", "Undo"),
    ("Redo", "Editing", "Redo"),
    ("ToggleMarker", "Timeline", "Add or remove a marker"),
    ("ToggleMagnet", "Timeline", "Main track magnet"),
    ("ToggleSnapping", "Timeline", "Snapping"),
    ("SelectTool", "Timeline", "Select tool"),
    ("BladeTool", "Timeline", "Blade tool"),
    ("ZoomIn", "Timeline", "Zoom in"),
    ("ZoomOut", "Timeline", "Zoom out"),
    ("ZoomToFit", "Timeline", "Zoom to fit"),
    ("NewProject", "File", "New project"),
    ("Open", "File", "Open project"),
    ("Save", "File", "Save"),
    ("Import", "File", "Import media"),
    ("Export", "File", "Export"),
    ("Quit", "File", "Quit"),
    ("OpenSettings", "App", "Settings"),
    ("ShowShortcuts", "App", "Keyboard shortcuts"),
];

/// "chukcut::DeleteLeft" → ("Other", "Delete left") for an action the table
/// does not know; the table's entry when it does.
fn describe(action: &str) -> (&'static str, String) {
    let name = action.rsplit("::").next().unwrap_or(action);
    if let Some((_, group, label)) = NAMES.iter().find(|(known, _, _)| *known == name) {
        return (group, (*label).to_string());
    }
    let mut label = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            label.push(' ');
            label.extend(c.to_lowercase());
        } else {
            label.push(c);
        }
    }
    ("Other", label)
}

/// One action and every key that runs it.
struct Entry {
    label: String,
    keys: Vec<Vec<gpui::Keystroke>>,
}

/// The sheet's content: group → entries, from the keymap as it is now.
fn collect(cx: &App) -> BTreeMap<usize, Vec<Entry>> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let mut by_action: Vec<(String, Vec<Vec<gpui::Keystroke>>)> = Vec::new();
    for binding in keymap.bindings() {
        let action = binding.action().name();
        if !action.starts_with("chukcut::") {
            continue;
        }
        let keys: Vec<gpui::Keystroke> = binding
            .keystrokes()
            .iter()
            .map(|stroke| stroke.as_keystroke().clone())
            .collect();
        match by_action.iter_mut().find(|(known, _)| known == action) {
            Some((_, list)) => {
                if !list.contains(&keys) {
                    list.push(keys);
                }
            }
            None => by_action.push((action.to_string(), vec![keys])),
        }
    }
    let mut groups: BTreeMap<usize, Vec<Entry>> = BTreeMap::new();
    for (action, keys) in by_action {
        let (group, label) = describe(&action);
        let index = GROUPS
            .iter()
            .position(|g| *g == group)
            .unwrap_or(GROUPS.len() - 1);
        groups.entry(index).or_default().push(Entry { label, keys });
    }
    // Table order within a group, so related actions sit together.
    for entries in groups.values_mut() {
        entries.sort_by_key(|entry| {
            NAMES
                .iter()
                .position(|(_, _, label)| *label == entry.label)
                .unwrap_or(usize::MAX)
        });
    }
    groups
}

pub(crate) fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, cx| {
        let groups = collect(cx);
        // Groups in order, each into the shortest of three columns, so the
        // sheet fits the window without scrolling.
        let mut columns: [(usize, Vec<AnyElement>); 3] = Default::default();
        for (index, entries) in groups {
            let height = entries.len() + 2;
            let group = render_group(GROUPS[index], entries);
            let column = columns
                .iter_mut()
                .min_by_key(|(used, _)| *used)
                .expect("three columns");
            column.0 += height;
            column.1.push(group);
        }
        let columns = columns.into_iter().map(|(_, groups)| {
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap_5()
                .children(groups)
        });
        dialog.w(px(1100.0)).title("Keyboard shortcuts").child(
            div()
                .id("shortcuts-scroll")
                .max_h(px(640.0))
                .overflow_y_scroll()
                .flex()
                .flex_row()
                .gap_8()
                .children(columns),
        )
    });
}

fn render_group(title: &str, entries: Vec<Entry>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .pb_1()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(ACCENT))
                .child(title.to_uppercase()),
        )
        .children(entries.into_iter().map(|entry| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .py(px(3.0))
                .child(div().text_sm().text_color(rgb(TEXT)).child(entry.label))
                .child(div().flex().flex_row().gap_1().flex_none().children(
                    entry.keys.into_iter().map(|sequence| {
                        div()
                            .flex()
                            .flex_row()
                            .gap_0p5()
                            .children(sequence.into_iter().map(Kbd::new))
                    }),
                ))
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_action_gets_its_group_and_name() {
        assert_eq!(
            describe("chukcut::PlayPause"),
            ("Playback", "Play / pause".to_string())
        );
    }

    #[test]
    fn an_unknown_action_still_appears_with_a_readable_name() {
        assert_eq!(
            describe("chukcut::PasteAttributes"),
            ("Other", "Paste attributes".to_string())
        );
    }

    #[test]
    fn every_named_group_exists() {
        for (action, group, _) in NAMES {
            assert!(GROUPS.contains(group), "{action} is in an unknown group");
        }
    }
}

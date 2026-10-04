//! The keyboard shortcuts sheet (Ctrl+/ or ?), and the app-level actions
//! that open it and the other shell surfaces.
//!
//! The sheet reads the engine's shortcut registry
//! (`chukcut_engine::modules::keymap`) — the same effective keymap the app
//! binds (`editor/keymap.rs`) — so it shows the user's own keys and cannot
//! drift from what the keys do. Changing them is the editor behind
//! "Edit shortcuts…" in the settings.

use std::collections::BTreeMap;

use chukcut_engine::modules::keymap::commands as keymap_commands;
use chukcut_engine::modules::keymap::GROUPS;
use gpui::component::kbd::Kbd;
use gpui::component::WindowExt as _;
use gpui::{AnyElement, Keystroke};

use super::*;

actions!(chukcut, [NewProject, OpenSettings, ShowShortcuts]);

/// One action and every key that runs it.
struct Entry {
    label: String,
    keys: Vec<Vec<gpui::Keystroke>>,
}

/// The sheet's content: group index → entries, in registry order, actions
/// without a key left out.
fn collect() -> BTreeMap<usize, Vec<Entry>> {
    let mut groups: BTreeMap<usize, Vec<Entry>> = BTreeMap::new();
    for binding in keymap_commands::keymap_bindings() {
        let keys: Vec<Vec<Keystroke>> = binding
            .keys
            .iter()
            .map(|key| {
                key.split_whitespace()
                    .filter_map(|chord| Keystroke::parse(chord).ok())
                    .collect::<Vec<_>>()
            })
            .filter(|strokes| !strokes.is_empty())
            .collect();
        if keys.is_empty() {
            continue;
        }
        let index = GROUPS
            .iter()
            .position(|g| *g == binding.group)
            .unwrap_or(GROUPS.len() - 1);
        groups.entry(index).or_default().push(Entry {
            label: binding.label.to_string(),
            keys,
        });
    }
    groups
}

pub(crate) fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, _| {
        let groups = collect();
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
    fn the_sheet_lists_bound_actions_by_group() {
        let groups = collect();
        let total: usize = groups.values().map(Vec::len).sum();
        assert!(total > 30, "{total}");
        // Play / pause is in the first group, with Space.
        let playback = &groups[&0];
        assert!(playback
            .iter()
            .any(|e| e.label == "Play / pause" && !e.keys.is_empty()));
    }
}

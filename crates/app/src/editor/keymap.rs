//! Key bindings from the engine's shortcut registry
//! (`chukcut_engine::modules::keymap`), and the shortcut editor in the
//! settings.
//!
//! The app binds no key of its own any more: [`install`] reads the effective
//! keymap (preset plus the user's changes) and binds every action in it.
//! GPUI Component's bindings (text fields, dialogs) are kept as they are.

use chukcut_engine::modules::keymap::commands as keymap_commands;
use chukcut_engine::modules::keymap::{keys, Binding, Keymap, Preset, GROUPS};
use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::kbd::Kbd;
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::{Disableable as _, Sizable as _, WindowExt as _};
use gpui::{AnyElement, Entity, KeyBinding, Keystroke, Subscription};

use super::analysis::*;
use super::audio_tools::*;
use super::playback::*;
use super::shortcuts::{NewProject, OpenSettings, ShowShortcuts};
use super::timeline::*;
use super::*;
use crate::ui::IconSrc;

/// `bindings!(Name, …)`: the binding builder for every action by name, and
/// the list of names it knows.
macro_rules! bindings {
    ($($name:ident),* $(,)?) => {
        /// A key binding of the action called `id` (no namespace).
        fn binding(id: &str, key: &str, context: Option<&str>) -> Option<KeyBinding> {
            match id {
                $(stringify!($name) => Some(KeyBinding::new(key, $name, context)),)*
                _ => None,
            }
        }

        /// Every action name [`binding`] knows.
        #[cfg(test)]
        const KNOWN: &[&str] = &[$(stringify!($name)),*];
    };
}

bindings!(
    PlayPause,
    ToggleFullscreen,
    ShuttleForward,
    ShuttleBack,
    ShuttleStop,
    StepBack,
    StepForward,
    StepBack10,
    StepForward10,
    GoToStart,
    GoToEnd,
    MarkIn,
    MarkOut,
    ClearInOut,
    ToggleLoop,
    Split,
    DeleteSelected,
    DeleteLeft,
    DeleteRight,
    Undo,
    Redo,
    CopyClips,
    CutClips,
    PasteClips,
    DuplicateClips,
    SelectAllClips,
    ClearSelection,
    ToggleMarker,
    ToggleMagnet,
    ToggleSnapping,
    SelectTool,
    BladeTool,
    ZoomIn,
    ZoomOut,
    ZoomToFit,
    DetachAudio,
    LinkClips,
    UnlinkClips,
    ResetSpeed,
    FreezeFrame,
    ReplaceMedia,
    CreateCompound,
    FlattenCompound,
    OpenCompound,
    CloseCompound,
    NewTimeline,
    DuckUnderSpeech,
    RemoveDucking,
    ToggleVoiceover,
    NewProject,
    Open,
    Save,
    Import,
    Export,
    Quit,
    OpenSettings,
    ShowShortcuts,
    DetectScenes,
    SplitAtScenes,
    StabiliseClip,
    DetectBeats,
    AutoCutToBeat,
    SnapCutsToBeats,
    AutoReframe,
    ReframeVertical,
    ReframeLandscape,
    CancelAnalysis,
);

/// Whether GPUI can parse every chord of `key`. A key the engine accepts
/// but GPUI does not is skipped with a log line rather than panicking in
/// `KeyBinding::new`.
fn parses(key: &str) -> bool {
    key.split_whitespace()
        .all(|chord| Keystroke::parse(chord).is_ok())
}

/// GPUI key bindings for `bindings`.
fn key_bindings(bindings: &[Binding]) -> Vec<KeyBinding> {
    let mut out = Vec::new();
    for b in bindings {
        for key in &b.keys {
            if !parses(key) {
                tracing::warn!(action = b.action, key, "a shortcut GPUI cannot parse");
                continue;
            }
            match binding(b.action, key, b.context_for(key)) {
                Some(kb) => out.push(kb),
                None => tracing::warn!(action = b.action, "a shortcut for an unknown action"),
            }
        }
    }
    out
}

/// Bind the effective keymap, replacing whatever chukcut bindings there
/// were and keeping everyone else's.
fn rebind(bindings: &[Binding], cx: &mut App) {
    let others: Vec<KeyBinding> = {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        keymap
            .bindings()
            .filter(|b| !b.action().name().starts_with("chukcut::"))
            .cloned()
            .collect()
    };
    cx.clear_key_bindings();
    cx.bind_keys(others);
    cx.bind_keys(key_bindings(bindings));
}

/// Bind the keymap as stored. Called at start and after every change.
pub(crate) fn install(cx: &mut App) {
    rebind(&keymap_commands::keymap_bindings(), cx);
}

// ---------------------------------------------------------------------------
// The editor
// ---------------------------------------------------------------------------

/// Open the shortcut editor.
pub(crate) fn open(window: &mut Window, cx: &mut App) {
    let editor = cx.new(|cx| ShortcutEditor::new(window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(760.0))
            .p_0()
            // Typing into the search field must not confirm the dialog.
            .on_ok(|_, _, _| false)
            .title(div().px_4().pt_3().child("Keyboard shortcuts"))
            .child(editor.clone())
    });
}

/// A key being captured for `action`.
struct Capture {
    action: &'static str,
    /// Add the key to the ones the action has, rather than replace them.
    add: bool,
    /// A key that clashes, waiting for "Take it" or "Cancel".
    pending: Option<String>,
}

pub(crate) struct ShortcutEditor {
    keymap: Keymap,
    search: Entity<InputState>,
    capture: Option<Capture>,
    /// Takes every keystroke before any binding sees it while a key is being
    /// captured, so Ctrl+S is recorded instead of saving the project. Dropped
    /// with the capture, or with the editor when the dialog closes.
    intercept: Option<Subscription>,
    notice: Option<SharedString>,
    _search: Subscription,
}

impl ShortcutEditor {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search actions or keys"));
        let subscription = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            keymap: keymap_commands::keymap_get(),
            search,
            capture: None,
            intercept: None,
            notice: None,
            _search: subscription,
        }
    }

    /// Store a change and bind it at once, or say why not.
    fn apply(&mut self, result: Result<Keymap, String>, cx: &mut Context<Self>) {
        match result {
            Ok(keymap) => {
                self.keymap = keymap;
                self.notice = None;
            }
            Err(error) => self.notice = Some(error.into()),
        }
        install(cx);
        cx.notify();
    }

    fn start_capture(&mut self, action: &'static str, add: bool, cx: &mut Context<Self>) {
        self.capture = Some(Capture {
            action,
            add,
            pending: None,
        });
        self.notice = None;
        let this = cx.entity().downgrade();
        self.intercept = Some(cx.intercept_keystrokes(move |event, _, cx| {
            let stroke = event.keystroke.clone();
            let taken = this
                .update(cx, |editor, cx| editor.on_stroke(&stroke, cx))
                .unwrap_or(false);
            if taken {
                cx.stop_propagation();
            }
        }));
        cx.notify();
    }

    fn stop_capture(&mut self, cx: &mut Context<Self>) {
        self.capture = None;
        self.intercept = None;
        cx.notify();
    }

    /// A keystroke while capturing. Answers whether it was taken; a stroke
    /// while a clash waits for an answer is left alone, so the buttons and
    /// Escape still work.
    fn on_stroke(&mut self, stroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(capture) = &mut self.capture else {
            return false;
        };
        if capture.pending.is_some() {
            return false;
        }
        if matches!(
            stroke.key.as_str(),
            "shift" | "control" | "ctrl" | "alt" | "super" | "platform" | "cmd" | "fn"
        ) || stroke.key.is_empty()
        {
            return true;
        }
        if stroke.key == "escape" && !stroke.modifiers.modified() {
            self.stop_capture(cx);
            return true;
        }
        let key = match keys::normalize(&stroke.unparse()) {
            Ok(key) => key,
            Err(error) => {
                self.notice = Some(error.into());
                cx.notify();
                return true;
            }
        };
        let action = capture.action;
        let mut wanted = if capture.add {
            self.keymap.keys_of(action)
        } else {
            Vec::new()
        };
        if !wanted.contains(&key) {
            wanted.push(key.clone());
        }
        if self.keymap.clashes(action, &wanted).is_empty() {
            self.capture = None;
            self.intercept = None;
            let result = keymap_commands::keymap_set(action, &wanted, false);
            self.apply(result, cx);
        } else {
            capture.pending = Some(key);
            // The clash is answered with the buttons; keys work again.
            self.intercept = None;
            cx.notify();
        }
        true
    }

    /// "Take it": the captured key moves to this action.
    fn take_pending(&mut self, cx: &mut Context<Self>) {
        self.intercept = None;
        let Some(capture) = self.capture.take() else {
            return;
        };
        let Some(key) = capture.pending else {
            return;
        };
        let mut wanted = if capture.add {
            self.keymap.keys_of(capture.action)
        } else {
            Vec::new()
        };
        if !wanted.contains(&key) {
            wanted.push(key);
        }
        let result = keymap_commands::keymap_set(capture.action, &wanted, true);
        self.apply(result, cx);
    }

    fn query(&self, cx: &App) -> Option<String> {
        let text = self.search.read(cx).value().trim().to_lowercase();
        (!text.is_empty()).then_some(text)
    }

    fn render_row(
        &self,
        binding: &Binding,
        conflicted: &[String],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = binding.action;
        let this = cx.entity().downgrade();
        let capturing = self.capture.as_ref().filter(|c| c.action == action);

        let chips: Vec<AnyElement> = binding
            .keys
            .iter()
            .map(|key| {
                let clash = conflicted.contains(key);
                let strokes: Vec<AnyElement> = key
                    .split_whitespace()
                    .filter_map(|chord| Keystroke::parse(chord).ok())
                    .map(|k| Kbd::new(k).into_any_element())
                    .collect();
                div()
                    .flex()
                    .flex_row()
                    .gap_0p5()
                    .px(px(2.0))
                    .rounded(px(R_XS))
                    .when(clash, |d| d.border_1().border_color(rgb(DANGER)))
                    .children(strokes)
                    .into_any_element()
            })
            .collect();

        let keys_cell = match capturing {
            Some(Capture {
                pending: Some(key), ..
            }) => {
                let owner = self
                    .keymap
                    .clashes(action, std::slice::from_ref(key))
                    .first()
                    .and_then(|(_, other)| chukcut_engine::modules::keymap::action(other))
                    .map_or("another action", |s| s.label);
                let take = this.clone();
                let cancel = this.clone();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(WARNING))
                            .child(format!("{} is \u{201c}{owner}\u{201d}", keys::display(key))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("take-{action}")))
                            .label("Take it")
                            .xsmall()
                            .primary()
                            .on_click(move |_, _, cx| {
                                let _ = take.update(cx, |e, cx| e.take_pending(cx));
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("cancel-{action}")))
                            .label("Cancel")
                            .xsmall()
                            .on_click(move |_, _, cx| {
                                let _ = cancel.update(cx, |e, cx| e.stop_capture(cx));
                            }),
                    )
                    .into_any_element()
            }
            Some(_) => div()
                .px_2()
                .py(px(2.0))
                .rounded(px(R_XS))
                .border_1()
                .border_color(rgb(ACCENT))
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(ACCENT))
                .child("Press a key\u{2026} (Esc cancels)")
                .into_any_element(),
            None if chips.is_empty() => div()
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(TEXT_MUTED))
                .child("No key")
                .into_any_element(),
            None => div()
                .flex()
                .flex_row()
                .gap_1()
                .children(chips)
                .into_any_element(),
        };

        let change = {
            let this = this.clone();
            Button::new(SharedString::from(format!("change-{action}")))
                .icon(Lucide::Pencil)
                .xsmall()
                .ghost()
                .tooltip("Change the key")
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |e, cx| e.start_capture(action, false, cx));
                })
        };
        let add = {
            let this = this.clone();
            Button::new(SharedString::from(format!("add-{action}")))
                .icon(Lucide::Plus)
                .xsmall()
                .ghost()
                .tooltip("Add another key")
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |e, cx| e.start_capture(action, true, cx));
                })
        };
        let clear = {
            let this = this.clone();
            Button::new(SharedString::from(format!("clear-{action}")))
                .icon(Lucide::X)
                .xsmall()
                .ghost()
                .tooltip("No key")
                .disabled(binding.keys.is_empty())
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |e, cx| {
                        let result = keymap_commands::keymap_set(action, &[], false);
                        e.apply(result, cx)
                    });
                })
        };
        let reset = {
            let this = this.clone();
            Button::new(SharedString::from(format!("reset-{action}")))
                .icon(Lucide::Undo2)
                .xsmall()
                .ghost()
                .tooltip("Back to the preset's key")
                .disabled(!binding.overridden)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |e, cx| {
                        let result = keymap_commands::keymap_reset(action);
                        e.apply(result, cx)
                    });
                })
        };

        div()
            .id(SharedString::from(format!("shortcut-{action}")))
            .min_h(px(ROW_H + 4.0))
            .px_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(HAIRLINE))
            .when(capturing.is_some(), |row| row.bg(rgb(PANEL_RAISED)))
            .child(
                div()
                    .w(px(260.0))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_size(px(TEXT_BODY))
                    .text_color(rgb(TEXT))
                    .child(binding.label)
                    .when(binding.overridden, |d| {
                        d.child(
                            div()
                                .text_size(px(TEXT_CAPTION))
                                .text_color(rgb(ACCENT))
                                .child("\u{2022}"),
                        )
                    }),
            )
            .child(div().flex_1().min_w(px(0.0)).child(keys_cell))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .gap_0p5()
                    .child(change)
                    .child(add)
                    .child(clear)
                    .child(reset),
            )
            .into_any_element()
    }
}

impl Render for ShortcutEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query(cx);
        let bindings = self.keymap.effective();
        let conflicts = self.keymap.conflicts();
        let conflicted: Vec<String> = conflicts.iter().map(|c| c.key.clone()).collect();

        let mut groups: Vec<AnyElement> = Vec::new();
        for group in GROUPS {
            let rows: Vec<AnyElement> = bindings
                .iter()
                .filter(|b| b.group == group)
                .filter(|b| {
                    query.as_ref().is_none_or(|q| {
                        b.label.to_lowercase().contains(q.as_str())
                            || b.keys.iter().any(|k| {
                                k.contains(q.as_str())
                                    || keys::display(k).to_lowercase().contains(q.as_str())
                            })
                    })
                })
                .map(|b| self.render_row(b, &conflicted, cx))
                .collect();
            if rows.is_empty() {
                continue;
            }
            groups.push(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_3()
                            .pt_3()
                            .pb_1()
                            .text_size(px(TEXT_CAPTION))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ACCENT))
                            .child(group.to_uppercase()),
                    )
                    .children(rows)
                    .into_any_element(),
            );
        }

        let this = cx.entity().downgrade();
        let preset = self.keymap.preset;
        let presets = Button::new("shortcut-preset")
            .label(format!("Preset: {}", preset.label()))
            .small()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                Preset::ALL.iter().fold(menu, |menu, each| {
                    let (this, each) = (this.clone(), *each);
                    menu.item(
                        PopupMenuItem::new(each.label())
                            .checked(each == preset)
                            .on_click(move |_, _, cx| {
                                let _ = this.update(cx, |e, cx| {
                                    let result = keymap_commands::keymap_set_preset(each);
                                    e.apply(result, cx)
                                });
                            }),
                    )
                })
            });
        let reset_all = {
            let this = cx.entity().downgrade();
            Button::new("shortcut-reset-all")
                .label("Reset all")
                .small()
                .disabled(self.keymap.overrides.is_empty())
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |e, cx| {
                        let result = keymap_commands::keymap_reset_all();
                        e.apply(result, cx)
                    });
                })
        };

        let conflict_line = (!conflicts.is_empty()).then(|| {
            let names: Vec<String> = conflicts.iter().map(|c| keys::display(&c.key)).collect();
            div()
                .px_3()
                .py_2()
                .rounded(px(R_SM))
                .bg(rgb(PANEL_RAISED))
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(DANGER))
                .child(format!(
                    "{} key{} run more than one action: {}",
                    conflicts.len(),
                    if conflicts.len() == 1 { "" } else { "s" },
                    names.join(", ")
                ))
        });

        div()
            .id("shortcut-editor")
            .px_4()
            .pb_4()
            .flex()
            .flex_col()
            .gap_3()
            .text_color(rgb(TEXT))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.search)
                                .small()
                                .cleanable(true)
                                .bg(rgb(WELL))
                                .border_color(rgb(BORDER))
                                .prefix(IconSrc::from(Lucide::Search).svg(14.0, rgb(TEXT_MUTED))),
                        ),
                    )
                    .child(presets)
                    .child(reset_all),
            )
            .children(self.notice.clone().map(|notice| {
                div()
                    .px_3()
                    .py_2()
                    .rounded(px(R_SM))
                    .bg(rgb(PANEL_RAISED))
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(WARNING))
                    .child(notice)
            }))
            .children(conflict_line)
            .child(
                div()
                    .id("shortcut-list")
                    .max_h(px(520.0))
                    .overflow_y_scroll()
                    .rounded(px(R_SM))
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(HAIRLINE))
                    .children(groups),
            )
    }
}

/// The settings dialog's "Keyboard shortcuts" section: the preset in use,
/// how many keys the user changed, and the button to the editor.
pub(crate) fn settings_section() -> AnyElement {
    let keymap = keymap_commands::keymap_get();
    let changed = keymap.overrides.len();
    let conflicts = keymap.conflicts().len();
    let mut summary = format!("Preset: {}", keymap.preset.label());
    if changed > 0 {
        summary.push_str(&format!(
            " \u{b7} {changed} change{}",
            if changed == 1 { "" } else { "s" }
        ));
    }
    if conflicts > 0 {
        summary.push_str(&format!(
            " \u{b7} {conflicts} conflict{}",
            if conflicts == 1 { "" } else { "s" }
        ));
    }
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .pb_1()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(ACCENT))
                .child("KEYBOARD SHORTCUTS"),
        )
        .child(
            div()
                .min_h(px(44.0))
                .px_3()
                .py_2()
                .flex()
                .flex_row()
                .items_center()
                .gap_4()
                .rounded_md()
                .bg(rgb(PANEL_RAISED))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().child("Shortcuts"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(if conflicts > 0 { DANGER } else { TEXT_DIM }))
                                .child(summary),
                        ),
                )
                .child(
                    Button::new("settings-edit-shortcuts")
                        .label("Edit shortcuts\u{2026}")
                        .small()
                        .on_click(|_, window, cx| open(window, cx)),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chukcut_engine::modules::keymap::ACTIONS;

    #[test]
    fn every_action_in_the_registry_has_an_app_action() {
        for spec in ACTIONS {
            assert!(
                KNOWN.contains(&spec.id),
                "{} is in the registry but the app cannot bind it",
                spec.id
            );
        }
        for known in KNOWN {
            assert!(
                ACTIONS.iter().any(|a| a.id == *known),
                "{known} can be bound but is not in the registry"
            );
        }
    }

    #[test]
    fn every_preset_key_parses_in_gpui() {
        for preset in Preset::ALL {
            let map = Keymap {
                preset,
                ..Keymap::default()
            };
            for b in map.effective() {
                for key in &b.keys {
                    assert!(parses(key), "{preset:?} {}: {key}", b.action);
                }
            }
        }
    }
}

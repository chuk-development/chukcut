//! The cloud features in the asset panel: text to speech, sound effects and
//! music in the Audio tab, the Stock tab, fal.ai tools on a selected clip in
//! the Media tab, and caption translation in the Captions tab.
//!
//! All of it is presentation over `chukcut_engine::modules::cloud`: the
//! engine talks to the providers, writes each file with its licence record,
//! and builds the edit that puts it on the timeline. Network calls run on
//! their own threads (`shell::spawn_blocking`) and land back here.

mod audio;
mod process;
mod stock;
mod translate;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::captions::edit::PlaceOptions;
use chukcut_engine::modules::captions::{TimedWord, Transcript};
use chukcut_engine::modules::cloud::commands::{self as cloud_commands, AccountView, Generated};
use chukcut_engine::modules::cloud::jobs::JobEvent;
use chukcut_engine::modules::cloud::providers::fal::Estimate;
use chukcut_engine::modules::cloud::stock::{StockKind, StockPage};
use chukcut_engine::modules::cloud::{place, Capability, CloudStore, Voice};
use chukcut_engine::modules::speech::SpeechSettings;
use chukcut_engine::shell::spawn_blocking;
use gpui::component::input::{InputEvent, InputState, TextareaState};
use gpui::component::Sizable as _;
use gpui::{AnyElement, Entity, Subscription};
use parking_lot::Mutex;

use super::*;
use crate::ui::Glyph;

pub(crate) use audio::AUDIO_CATEGORIES;
pub(crate) use stock::STOCK_CATEGORIES;

/// The Stock tab's glyph: a frame with a mountain and a sun, on the kit's
/// 24 grid and stroke.
pub(crate) const STOCK_GLYPH: Glyph = Glyph(
    br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2.5"/><circle cx="9" cy="9.5" r="1.75"/><path d="M3.5 18l5.5-5.5 3.5 3.5 2.5-2.5 5.5 5.5"/></svg>"#,
);

/// Where a finished file goes.
#[derive(Debug, Clone)]
pub(crate) enum Placement {
    /// Into the library only.
    Library,
    /// On the timeline at this time.
    At(Micros),
    /// On a new lane above this clip.
    Beside(String),
    /// In place of this clip.
    Replace(String),
}

/// A voice list being fetched or fetched.
pub(crate) enum Voices {
    Loading,
    Ready(Vec<Voice>),
    Failed(String),
}

/// A running fal job.
pub(crate) struct FalRun {
    pub(crate) event: std::sync::Arc<Mutex<Option<JobEvent>>>,
    pub(crate) cancel: std::sync::Arc<AtomicBool>,
    pub(crate) action: String,
}

/// A stock or preview image: being fetched, on disk, or failed.
pub(crate) enum Picture {
    Loading,
    Ready(PathBuf),
    Failed,
}

/// The cloud panels' state: one field of the asset panel.
pub(crate) struct CloudPanel {
    /// The accounts generation last read (`accounts::ACCOUNTS_CHANGED`).
    seen: u64,
    accounts: Vec<AccountView>,
    /// The account chosen per capability.
    chosen: HashMap<&'static str, String>,

    // Text to speech.
    tts_text: Entity<TextareaState>,
    voice_typed: Entity<InputState>,
    /// ElevenLabs: the chosen model; `None` is the account's default.
    tts_model: Option<String>,
    /// OpenAI-compatible: the speech model typed; empty is the default.
    tts_model_typed: Entity<InputState>,
    voices: HashMap<String, Voices>,
    voice: Option<String>,
    stability: f32,
    similarity: f32,
    speed: f32,
    tts_captions: bool,

    // Sound effects and music.
    sfx_prompt: Entity<InputState>,
    sfx_seconds: f32,
    sfx_loop: bool,
    music_prompt: Entity<InputState>,
    music_seconds: f32,
    instrumental: bool,

    /// The generation running now, by label.
    busy: Option<SharedString>,
    /// What this session generated, newest first.
    recent: Vec<Generated>,

    // Stock.
    stock_query: Entity<InputState>,
    stock_page: Option<Result<StockPage, String>>,
    stock_loading: bool,
    stock_page_no: u32,
    stock_nc: bool,
    stock_kind_seen: Option<StockKind>,
    pictures: HashMap<String, Picture>,
    downloading: HashMap<String, Placement>,

    // fal.
    fal_action: usize,
    fal_estimate: Option<Result<Estimate, String>>,
    fal_estimating: bool,
    fal_estimate_for: Option<(String, usize)>,
    fal_run: Option<FalRun>,
    fal_placement: usize,

    // Translation.
    translate_target: String,
    translate_model: Entity<InputState>,
    translating: bool,

    _subscriptions: Vec<Subscription>,
}

impl CloudPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Editor>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Editor>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let tts_text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(4)
                .placeholder("Type what the voice should say")
        });
        let stock_query = input("Search stock, then press Enter", window, cx);
        let subscriptions = vec![cx.subscribe_in(
            &stock_query,
            window,
            |this: &mut Editor, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.assets.cloud.stock_page_no = 1;
                    this.stock_search(cx);
                }
            },
        )];
        Self {
            seen: u64::MAX,
            accounts: Vec::new(),
            chosen: HashMap::new(),
            tts_text,
            voice_typed: input("Voice name, e.g. alloy or af_bella", window, cx),
            tts_model: None,
            tts_model_typed: input("Model, e.g. gpt-4o-mini-tts or tts-1", window, cx),
            voices: HashMap::new(),
            voice: None,
            stability: 0.5,
            similarity: 0.75,
            speed: 1.0,
            tts_captions: true,
            sfx_prompt: input(
                "Describe the sound, e.g. a door slams in a hallway",
                window,
                cx,
            ),
            sfx_seconds: 0.0,
            sfx_loop: false,
            music_prompt: input("Describe the music, e.g. calm lofi beat, piano", window, cx),
            music_seconds: 30.0,
            instrumental: true,
            busy: None,
            recent: Vec::new(),
            stock_query,
            stock_page: None,
            stock_loading: false,
            stock_page_no: 1,
            stock_nc: false,
            stock_kind_seen: None,
            pictures: HashMap::new(),
            downloading: HashMap::new(),
            fal_action: 0,
            fal_estimate: None,
            fal_estimating: false,
            fal_estimate_for: None,
            fal_run: None,
            fal_placement: 0,
            translate_target: "en".into(),
            translate_model: input("Chat model, e.g. gpt-4o-mini", window, cx),
            translating: false,
            _subscriptions: subscriptions,
        }
    }

    /// Read the accounts again if Settings changed them.
    fn refresh_accounts(&mut self) {
        let generation = super::accounts::accounts_generation();
        if generation != self.seen {
            self.seen = generation;
            self.accounts = cloud_commands::cloud_accounts(&CloudStore::user(), None);
            self.voices.clear();
        }
    }

    /// The accounts that can do `capability`.
    fn able(&self, capability: Capability) -> Vec<AccountView> {
        self.accounts
            .iter()
            .filter(|a| a.account.can(capability))
            .cloned()
            .collect()
    }

    /// The chosen account for `slot` among `able`, defaulting to the first.
    fn chosen_of(&self, slot: &'static str, able: &[AccountView]) -> Option<AccountView> {
        let chosen = self.chosen.get(slot);
        able.iter()
            .find(|a| Some(&a.account.id) == chosen)
            .or_else(|| able.first())
            .cloned()
    }
}

// --- shared drawing ----------------------------------------------------------------

/// `mm:ss`, as on a tile's duration badge.
fn mmss(duration: Micros) -> String {
    let seconds = (duration.max(0) as f64 / 1_000_000.0).round() as i64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn label(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(TEXT_LABEL))
        .text_color(rgb(TEXT_DIM))
        .child(text.into())
}

fn hint(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(TEXT_CAPTION))
        .text_color(rgb(TEXT_MUTED))
        .child(text.into())
}

fn column() -> gpui::Div {
    div().flex().flex_col().gap(px(8.0))
}

fn row() -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(6.0))
}

/// A pill that is either on or off.
fn chip(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(CONTROL_H))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(R_SM))
        .cursor_pointer()
        .text_size(px(TEXT_LABEL))
        .border_1()
        .when(active, |pill| {
            pill.bg(accent_soft())
                .border_color(rgb(ACCENT))
                .text_color(rgb(ACCENT))
        })
        .when(!active, |pill| {
            pill.border_color(rgb(BORDER))
                .text_color(rgb(TEXT_DIM))
                .hover(|style| style.bg(rgb(PANEL_RAISED)).text_color(rgb(TEXT)))
        })
        .on_click(on_click)
        .child(text.into())
}

/// A value with − and + either side, in the kit's mono well.
fn stepper(
    id: &'static str,
    value: String,
    on_step: impl Fn(&mut Editor, f32, &mut Context<Editor>) + 'static,
    cx: &mut Context<Editor>,
) -> impl IntoElement {
    let on_step = std::rc::Rc::new(on_step);
    let (down, up) = (on_step.clone(), on_step);
    let button = |id: String, text: &'static str| {
        div()
            .id(SharedString::from(id))
            .size(px(CONTROL_H))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(R_SM))
            .cursor_pointer()
            .text_color(rgb(TEXT_DIM))
            .hover(|style| style.bg(rgb(PANEL_RAISED)).text_color(rgb(TEXT)))
            .child(text)
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .child(
            button(format!("{id}-down"), "\u{2212}").on_click(cx.listener(
                move |this, _, _, cx| {
                    down(this, -1.0, cx);
                    cx.notify();
                },
            )),
        )
        .child(
            div()
                .min_w(px(56.0))
                .h(px(CONTROL_H))
                .px(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(R_SM))
                .bg(rgb(WELL))
                .border_1()
                .border_color(rgb(BORDER))
                .font_family(FONT_MONO)
                .text_size(px(TEXT_LABEL))
                .child(value),
        )
        .child(
            button(format!("{id}-up"), "+").on_click(cx.listener(move |this, _, _, cx| {
                up(this, 1.0, cx);
                cx.notify();
            })),
        )
}

/// "Connect an account" for a capability nobody can do yet.
fn no_account(
    id: &'static str,
    what: &'static str,
    providers: &'static str,
    cx: &mut Context<Editor>,
) -> AnyElement {
    crate::ui::EmptyState::new(id, crate::ui::icons::PLUS, what)
        .hint(format!(
            "Add a {providers} account with your own API key in Settings, Accounts."
        ))
        .action(
            gpui::component::button::Button::new(SharedString::from(format!("{id}-open")))
                .label("Open Settings")
                .small()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_settings_dialog(window, cx);
                })),
        )
        .into_any_element()
}

/// The account picker for `slot`.
fn account_picker(
    slot: &'static str,
    able: Vec<AccountView>,
    current: &AccountView,
    cx: &mut Context<Editor>,
) -> impl IntoElement {
    use gpui::component::button::Button;
    use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
    let editor = cx.entity().downgrade();
    let current_id = current.account.id.clone();
    Button::new(SharedString::from(format!("cloud-account-{slot}")))
        .label(current.account.name.clone())
        .small()
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _, _| {
            able.iter().fold(menu, |menu, view| {
                let editor = editor.clone();
                let id = view.account.id.clone();
                menu.item(
                    PopupMenuItem::new(view.account.name.clone())
                        .checked(id == current_id)
                        .on_click(move |_, _, cx| {
                            let id = id.clone();
                            let _ = editor.update(cx, |this, cx| {
                                this.assets.cloud.chosen.insert(slot, id);
                                cx.notify();
                            });
                        }),
                )
            })
        })
}

impl Editor {
    /// Run `work` on its own thread and hand the answer to `done` here.
    fn cloud_task<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Editor, T, &mut Context<Editor>) + 'static,
    ) {
        let future = spawn_blocking(work);
        cx.spawn(async move |this, cx| {
            if let Ok(value) = future.await {
                let _ = this.update(cx, |editor, cx| {
                    done(editor, value, cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn open_settings_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        super::settings::open(Some(cx.entity().downgrade()), window, cx);
    }

    /// Import `path` (its licence record comes with it) and put it where
    /// `placement` says. `words`, when given, become captions at the same
    /// place.
    pub(crate) fn cloud_import(
        &mut self,
        path: PathBuf,
        placement: Placement,
        words: Vec<TimedWord>,
        cx: &mut Context<Self>,
    ) {
        let state = Arc::clone(&self.state);
        cx.spawn(async move |this, cx| {
            let path_text = path.to_string_lossy().to_string();
            let imported = project_commands::project_import_media(&state, path_text).await;
            let _ = this.update(cx, |editor, cx| {
                let imported = match imported {
                    Ok(imported) => imported,
                    Err(error) => {
                        editor.report(Err(error), cx);
                        return;
                    }
                };
                let command = match &placement {
                    Placement::Library => None,
                    Placement::At(at) => {
                        Some(place::at_playhead(&editor.project_now(), &imported.id, *at))
                    }
                    Placement::Beside(segment) => {
                        Some(place::beside(&editor.project_now(), segment, &imported.id))
                    }
                    Placement::Replace(segment) => {
                        Some(place::replace(&editor.project_now(), segment, &imported.id))
                    }
                };
                let mut result = match command {
                    Some(command) => command
                        .and_then(|c| timeline_commands::timeline_apply(&editor.state, c))
                        .map(|_| ()),
                    None => Ok(()),
                };
                if result.is_ok() && !words.is_empty() {
                    if let Placement::At(at) = placement {
                        let transcript = Transcript {
                            language: None,
                            words: words.clone(),
                            segments: Vec::new(),
                        }
                        .shifted(at);
                        result =
                            chukcut_engine::modules::captions::commands::captions_from_transcript(
                                &editor.state,
                                &transcript,
                                SpeechSettings::load().mode,
                                None,
                                PlaceOptions::default(),
                            )
                            .map(|_| ());
                    }
                }
                editor.refresh(cx);
                match result {
                    Ok(()) => {
                        editor.status = Some(
                            match placement {
                                Placement::Library => {
                                    format!("Added {} to the library", imported.name)
                                }
                                _ => format!("Added {}", imported.name),
                            }
                            .into(),
                        );
                        cx.notify();
                    }
                    Err(error) => editor.report(Err(error), cx),
                }
            });
        })
        .detach();
    }

    /// The document as the engine holds it now. The view's snapshot can be
    /// one import behind, and an import does not refresh it.
    fn project_now(&self) -> Project {
        self.state
            .project
            .read()
            .clone()
            .unwrap_or_else(|| (*self.project).clone())
    }
}

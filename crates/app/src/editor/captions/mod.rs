//! The Captions tab: auto captions, editing, styling, SRT import and export,
//! and dragging a caption on the player.
//!
//! Everything here is presentation. Grouping, placement, timing, styling and
//! file formats are `chukcut_engine::modules::captions`; transcription is
//! `speech`; the accounts and their keys are `cloud`. This file holds the
//! panel's own state and turns clicks into calls to those commands.

mod overlay;
mod view;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chukcut_engine::modules::captions::commands as caption_commands;
use chukcut_engine::modules::captions::edit::{self as caption_edit, PlaceOptions};
use chukcut_engine::modules::captions::srt::SubtitleFormat;
use chukcut_engine::modules::captions::{CaptionMode, CaptionStyle, Cue, Transcript};
use chukcut_engine::modules::cloud::commands::{self as cloud_commands, AccountView};
use chukcut_engine::modules::cloud::{Capability, CloudStore, TestReport};
use chukcut_engine::modules::speech::commands::{self as speech_commands, SpeechProgress};
use chukcut_engine::modules::speech::SpeechSettings;
use chukcut_engine::modules::timeline::ops::TrackFlags;
use gpui::component::input::{InputEvent, InputState};
use gpui::{Entity, Subscription};
use parking_lot::Mutex;

use super::*;

pub(crate) use overlay::CaptionDrag;

/// The tab's category column, in order.
pub(crate) const CATEGORIES: &[&str] = &[
    "Auto captions",
    "Captions",
    "Style",
    "Import & export",
    "Translate",
];

/// A transcription running on its own thread.
pub(crate) struct Job {
    progress: Arc<Mutex<SpeechProgress>>,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<Transcript, String>>>>,
}

/// The account form, open while adding or editing an account.
pub(crate) struct AccountForm {
    /// `None` while adding.
    id: Option<String>,
}

/// The panel's state, one field on the editor.
pub(crate) struct CaptionsPanel {
    settings: SpeechSettings,
    accounts: Vec<AccountView>,
    /// `accounts::ACCOUNTS_CHANGED` when `accounts` was read.
    accounts_seen: u64,
    form: Option<AccountForm>,
    test: Option<TestReport>,
    testing: bool,
    job: Option<Job>,
    /// The caption whose words are in `text`.
    editing: Option<String>,
    /// Style edits go to every caption, or only to the selected one.
    style_all: bool,
    emoji_open: bool,
    emoji_category: usize,
    fonts: Arc<Mutex<Vec<String>>>,
    pub(crate) drag: Option<CaptionDrag>,

    name: Entity<InputState>,
    url: Entity<InputState>,
    model: Entity<InputState>,
    key: Entity<InputState>,
    text: Entity<InputState>,
    chars: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl CaptionsPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Editor>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Editor>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let name = input("Name, e.g. Groq", window, cx);
        let url = input("https://api.example.com/v1", window, cx);
        let model = input("Model, e.g. whisper-1", window, cx);
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("API key (kept on this computer)")
                .masked(true)
        });
        let text = input("Caption text", window, cx);
        let settings = SpeechSettings::load();
        let chars = cx.new(|cx| {
            InputState::new(window, cx).default_value(match settings.mode {
                CaptionMode::Sentences {
                    max_chars_per_line, ..
                } => max_chars_per_line.to_string(),
                CaptionMode::Words { .. } => "42".to_string(),
            })
        });

        let subscriptions = vec![
            cx.subscribe_in(
                &text,
                window,
                |this: &mut Editor, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        this.commit_caption_text(cx);
                        window.focus(&this.focus, cx);
                    }
                    InputEvent::Blur => this.commit_caption_text(cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &chars,
                window,
                |this: &mut Editor, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.commit_line_length(cx);
                    }
                },
            ),
        ];

        // The font list scans the system once, tens of milliseconds; not on
        // the UI thread.
        let fonts = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&fonts);
        std::thread::spawn(move || {
            *sink.lock() = chukcut_engine::modules::text::TextRenderer::shared().font_families();
        });

        Self {
            accounts: cloud_commands::cloud_accounts(
                &CloudStore::user(),
                Some(Capability::Transcribe),
            ),
            accounts_seen: super::accounts::accounts_generation(),
            settings,
            form: None,
            test: None,
            testing: false,
            job: None,
            editing: None,
            style_all: true,
            emoji_open: false,
            emoji_category: 0,
            fonts,
            drag: None,
            name,
            url,
            model,
            key,
            text,
            chars,
            _subscriptions: subscriptions,
        }
    }

    fn save_settings(&self) -> Result<(), String> {
        self.settings.save()
    }

    /// Read the accounts again when Settings changed them.
    pub(crate) fn sync_accounts(&mut self) {
        let generation = super::accounts::accounts_generation();
        if generation != self.accounts_seen {
            self.accounts_seen = generation;
            self.reload_accounts();
        }
    }

    fn reload_accounts(&mut self) {
        self.accounts =
            cloud_commands::cloud_accounts(&CloudStore::user(), Some(Capability::Transcribe));
        // A deleted account must not stay selected.
        if let Some(id) = &self.settings.account {
            if !self.accounts.iter().any(|a| &a.account.id == id) {
                self.settings.account = None;
            }
        }
        if self.settings.account.is_none() {
            self.settings.account = self.accounts.first().map(|a| a.account.id.clone());
        }
    }
}

/// `text` re-broken into `lines` lines of about equal length — how an edited
/// two-line caption stays two lines without the user typing a line break into
/// a one-line field.
fn rewrap(text: &str, lines: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if lines <= 1 || words.len() < 2 {
        return words.join(" ");
    }
    let total: usize = words.iter().map(|w| w.chars().count() + 1).sum();
    let per_line = total.div_ceil(lines);
    let mut out = String::new();
    let mut used = 0usize;
    let mut breaks = 1usize;
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            // Break before the word whose middle would cross the line's share.
            let middle = used + word.chars().count().div_ceil(2);
            if middle > per_line * breaks && breaks < lines {
                out.push('\n');
                breaks += 1;
            } else {
                out.push(' ');
            }
        }
        out.push_str(word);
        used += word.chars().count() + 1;
    }
    out
}

impl Editor {
    // --- auto captions ------------------------------------------------------

    fn start_transcription(&mut self, cx: &mut Context<Self>) {
        if self.captions.job.is_some() {
            return;
        }
        let _ = self.captions.save_settings();
        let job = Job {
            progress: Arc::new(Mutex::new(SpeechProgress {
                label: "Starting".into(),
                fraction: None,
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(None)),
        };
        let (progress, cancel, result) = (
            Arc::clone(&job.progress),
            Arc::clone(&job.cancel),
            Arc::clone(&job.result),
        );
        let state = Arc::clone(&self.state);
        let settings = self.captions.settings.clone();
        std::thread::Builder::new()
            .name("transcribe".into())
            .spawn(move || {
                let outcome = speech_commands::speech_transcribe(
                    &state,
                    &settings,
                    &|p| *progress.lock() = p,
                    &cancel,
                );
                *result.lock() = Some(outcome);
            })
            .ok();
        self.captions.job = Some(job);
        self.status = None;

        // Redraw for the progress bar, and pick the result up when it lands.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let running = this.update(cx, |editor, cx| {
                cx.notify();
                editor.poll_transcription(cx)
            });
            if !matches!(running, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    /// `false` once the job is over.
    fn poll_transcription(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(job) = &self.captions.job else {
            return false;
        };
        let Some(outcome) = job.result.lock().take() else {
            return true;
        };
        self.captions.job = None;
        let settings = &self.captions.settings;
        let result = outcome.and_then(|transcript| {
            let words_were_estimated = transcript.words.is_empty();
            let added = caption_commands::captions_from_transcript(
                &self.state,
                &transcript,
                settings.mode,
                None,
                PlaceOptions {
                    replace: settings.replace,
                    auto_emoji: settings.auto_emoji,
                },
            )?;
            Ok((added, words_were_estimated, transcript.language))
        });
        self.refresh(cx);
        match result {
            Ok((added, estimated, language)) => {
                let mut message = format!("Added {} captions", added.segment_ids.len());
                if let Some(language) = language {
                    message.push_str(&format!(
                        " ({})",
                        chukcut_engine::modules::speech::language_name(&language)
                    ));
                }
                if estimated {
                    message.push_str(". This provider gave no word timing, so word captions and karaoke are estimated");
                }
                self.selected = added.segment_ids.first().cloned();
                self.status = Some(message.into());
            }
            Err(error) if error == "cancelled" => self.status = Some("Cancelled".into()),
            Err(error) => self.status = Some(error.into()),
        }
        cx.notify();
        false
    }

    fn cancel_transcription(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &self.captions.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        cx.notify();
    }

    fn regroup_captions(&mut self, cx: &mut Context<Self>) {
        let result = caption_commands::captions_regroup(&self.state, self.captions.settings.mode)
            .map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    fn commit_line_length(&mut self, cx: &mut Context<Self>) {
        let text = self.captions.chars.read(cx).value().to_string();
        if let (
            Ok(chars),
            CaptionMode::Sentences {
                max_lines,
                max_duration,
                ..
            },
        ) = (text.trim().parse::<usize>(), self.captions.settings.mode)
        {
            self.captions.settings.mode = CaptionMode::Sentences {
                max_chars_per_line: chars.clamp(8, 120),
                max_lines,
                max_duration,
            };
            let _ = self.captions.save_settings();
            cx.notify();
        }
    }

    // --- accounts -----------------------------------------------------------

    fn open_account_form(
        &mut self,
        id: Option<String>,
        preset: Option<(&str, &str, &str)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = id
            .as_ref()
            .and_then(|id| self.captions.accounts.iter().find(|a| &a.account.id == id))
            .map(|a| a.account.clone());
        let (name, url, model) = match (&existing, preset) {
            (Some(a), _) => (a.name.clone(), a.base_url.clone(), a.default_model.clone()),
            (None, Some((n, u, m))) => (n.to_string(), u.to_string(), m.to_string()),
            (None, None) => (String::new(), String::new(), String::new()),
        };
        self.captions
            .name
            .update(cx, |s, cx| s.set_value(name, window, cx));
        self.captions
            .url
            .update(cx, |s, cx| s.set_value(url, window, cx));
        self.captions
            .model
            .update(cx, |s, cx| s.set_value(model, window, cx));
        self.captions
            .key
            .update(cx, |s, cx| s.set_value(String::new(), window, cx));
        self.captions.form = Some(AccountForm { id });
        self.captions.test = None;
        cx.notify();
    }

    fn save_account(&mut self, test: bool, cx: &mut Context<Self>) {
        let Some(form) = &self.captions.form else {
            return;
        };
        let value =
            |input: &Entity<InputState>, cx: &App| input.read(cx).value().trim().to_string();
        let name = value(&self.captions.name, cx);
        let url = value(&self.captions.url, cx);
        let model = value(&self.captions.model, cx);
        let key = value(&self.captions.key, cx);

        let store = CloudStore::user();
        let mut account = match &form.id {
            Some(id) => match store.account(id) {
                Some(account) => account,
                None => cloud_commands::cloud_account_new(&name, &url, &model),
            },
            None => cloud_commands::cloud_account_new(&name, &url, &model),
        };
        account.name = if name.is_empty() {
            "Transcription".into()
        } else {
            name
        };
        account.base_url = url;
        account.default_model = model;
        // An empty key field while editing means "keep the stored key".
        let key = (!key.is_empty()).then_some(key);
        match cloud_commands::cloud_account_set(&store, account, key.as_deref()) {
            Ok(id) => {
                self.captions.settings.account = Some(id.clone());
                self.captions.reload_accounts();
                let _ = self.captions.save_settings();
                self.captions.form = None;
                self.status = None;
                if test {
                    self.test_account(id, cx);
                }
            }
            Err(error) => self.status = Some(error.into()),
        }
        cx.notify();
    }

    fn remove_account(&mut self, id: String, cx: &mut Context<Self>) {
        let result = cloud_commands::cloud_account_remove(&CloudStore::user(), &id);
        self.captions.reload_accounts();
        let _ = self.captions.save_settings();
        self.captions.form = None;
        self.report(result, cx);
    }

    fn test_account(&mut self, id: String, cx: &mut Context<Self>) {
        self.captions.testing = true;
        self.captions.test = None;
        let task = cx.background_spawn(async move {
            cloud_commands::cloud_account_test(&CloudStore::user(), &id)
        });
        cx.spawn(async move |this, cx| {
            let report = task.await;
            let _ = this.update(cx, |editor, cx| {
                editor.captions.testing = false;
                editor.captions.test = Some(report);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // --- editing ------------------------------------------------------------

    /// The selected clip, when it is a caption.
    fn selected_caption(&self) -> Option<String> {
        let id = self.selected.as_ref()?;
        let (_, segment) = self.project.segment(id)?;
        caption_edit::is_caption_segment(&self.project, segment).then(|| id.clone())
    }

    /// Select a caption, put its words in the text field and the playhead on it.
    fn pick_caption(&mut self, segment_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, segment)) = self.project.segment(&segment_id) else {
            return;
        };
        let start = segment.target_range.start;
        let text = self
            .project
            .materials
            .text(&segment.material_id)
            .map(|m| m.content.replace('\n', " "))
            .unwrap_or_default();
        self.captions
            .text
            .update(cx, |s, cx| s.set_value(text, window, cx));
        self.captions.editing = Some(segment_id.clone());
        self.selected = Some(segment_id);
        self.pause();
        // Just inside, so the frame shown is the caption's and not the one
        // before it.
        self.seek(start + 1);
        cx.notify();
    }

    fn commit_caption_text(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.captions.editing.clone() else {
            return;
        };
        let Some(material) = self
            .project
            .segment(&id)
            .and_then(|(_, s)| self.project.materials.text(&s.material_id))
        else {
            return;
        };
        let typed = self.captions.text.read(cx).value().to_string();
        let lines = material.content.lines().count().max(1);
        let text = rewrap(&typed, lines);
        if text.trim().is_empty() || text == material.content {
            return;
        }
        let result = caption_commands::captions_set_text(&self.state, &id, &text).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    /// Put an emoji into the caption: at the cursor while typing, otherwise at
    /// the end of the selected caption.
    fn insert_emoji(&mut self, emoji: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let typing =
            gpui::Focusable::focus_handle(self.captions.text.read(cx), cx).is_focused(window);
        if typing {
            self.captions
                .text
                .update(cx, |s, cx| s.insert(emoji, window, cx));
            return;
        }
        let Some(id) = self.selected_caption() else {
            self.status = Some("Select a caption first".into());
            cx.notify();
            return;
        };
        let Some(content) = self
            .project
            .segment(&id)
            .and_then(|(_, s)| self.project.materials.text(&s.material_id))
            .map(|m| m.content.clone())
        else {
            return;
        };
        let text = format!("{} {emoji}", content.trim_end());
        let result = caption_commands::captions_set_text(&self.state, &id, &text).map(|_| ());
        self.refresh(cx);
        if self.captions.editing.as_deref() == Some(id.as_str()) {
            self.captions
                .text
                .update(cx, |s, cx| s.set_value(text.replace('\n', " "), window, cx));
        }
        self.report(result, cx);
    }

    fn add_caption_at_playhead(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.clock.position();
        let cue = Cue::new(at, at + 2_000_000, "New caption");
        match caption_commands::captions_add(&self.state, &[cue], None, PlaceOptions::default()) {
            Ok(added) => {
                self.refresh(cx);
                if let Some(id) = added.segment_ids.first().cloned() {
                    self.pick_caption(id, window, cx);
                }
                self.report(Ok(()), cx);
            }
            Err(error) => self.report(Err(error), cx),
        }
    }

    fn split_caption(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_caption() else {
            return;
        };
        let at = self.clock.position();
        match caption_commands::captions_split(&self.state, &id, at) {
            Ok((_, ids)) => {
                self.refresh(cx);
                self.pick_caption(ids[1].clone(), window, cx);
                self.seek(at);
                self.report(Ok(()), cx);
            }
            Err(error) => self.report(Err(error), cx),
        }
    }

    fn merge_caption_with_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_caption() else {
            return;
        };
        let clips = caption_edit::clips(&self.project);
        let Some(index) = clips.iter().position(|c| c.segment_id == id) else {
            return;
        };
        let Some(next) = clips
            .get(index + 1)
            .filter(|n| n.track_id == clips[index].track_id)
        else {
            self.report(Err("there is no caption after this one".into()), cx);
            return;
        };
        match caption_commands::captions_merge(&self.state, &id, &next.segment_id) {
            Ok((_, merged)) => {
                self.refresh(cx);
                self.pick_caption(merged, window, cx);
                self.report(Ok(()), cx);
            }
            Err(error) => self.report(Err(error), cx),
        }
    }

    fn delete_caption(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_caption() else {
            return;
        };
        self.captions.editing = None;
        self.remove_clip(&id, cx);
    }

    fn clear_captions(&mut self, cx: &mut Context<Self>) {
        let result = caption_commands::captions_clear(&self.state).map(|_| ());
        self.captions.editing = None;
        self.refresh(cx);
        self.report(result, cx);
    }

    // --- style ----------------------------------------------------------------

    /// The style the panel shows: the selected caption's, or the first one's.
    fn current_caption_style(&self) -> CaptionStyle {
        let selected = self.selected_caption();
        caption_commands::captions_style_of(&self.state, selected.as_deref())
            .unwrap_or_else(|_| CaptionStyle::default_for(&self.project.canvas))
    }

    /// Apply a change to the current style, to all captions or the selected one.
    fn restyle_captions(&mut self, change: impl FnOnce(&mut CaptionStyle), cx: &mut Context<Self>) {
        let mut style = self.current_caption_style();
        change(&mut style);
        let target = if self.captions.style_all {
            None
        } else {
            match self.selected_caption() {
                Some(id) => Some(id),
                None => {
                    self.report(Err("Select a caption, or style all of them".into()), cx);
                    return;
                }
            }
        };
        let result = caption_commands::captions_set_style(&self.state, target.as_deref(), &style)
            .map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    // --- files ----------------------------------------------------------------

    fn import_subtitles(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import subtitles (.srt, .vtt)".into()),
        });
        cx.spawn(async move |this, cx| match picked.await {
            Ok(Ok(Some(paths))) => {
                let Some(path) = paths.into_iter().next() else {
                    return;
                };
                let _ = this.update(cx, |editor, cx| {
                    let replace = editor.captions.settings.replace;
                    let result = caption_commands::captions_import(
                        &editor.state,
                        &path,
                        None,
                        PlaceOptions {
                            replace,
                            auto_emoji: false,
                        },
                    );
                    editor.refresh(cx);
                    match result {
                        Ok(added) => {
                            editor.status = Some(
                                format!("Imported {} captions", added.segment_ids.len()).into(),
                            );
                            cx.notify();
                        }
                        Err(error) => editor.report(Err(error), cx),
                    }
                });
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |editor, cx| editor.dialog_failed(error, cx));
            }
            _ => {}
        })
        .detach();
    }

    fn export_subtitles(&mut self, format: SubtitleFormat, cx: &mut Context<Self>) {
        let directory = self
            .state
            .project_path
            .read()
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let name = format!("{}.{}", self.project.name, format.extension());
        let picked = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |editor, cx| {
                match caption_commands::captions_export(&editor.state, &path, Some(format)) {
                    Ok(count) => {
                        editor.status =
                            Some(format!("Wrote {count} captions to {}", path.display()).into());
                        cx.notify();
                    }
                    Err(error) => editor.report(Err(error), cx),
                }
            });
        })
        .detach();
    }

    /// Show or hide the caption lanes, which is what decides whether an export
    /// burns them in.
    fn set_burn_in(&mut self, burn: bool, cx: &mut Context<Self>) {
        let commands: Vec<EditCommand> = self
            .project
            .tracks
            .iter()
            .filter(|t| caption_edit::is_caption_track(&self.project, t) && t.hidden == burn)
            .map(|t| {
                let before = TrackFlags::of(t);
                EditCommand::SetTrackFlags {
                    track_id: t.id.clone(),
                    before,
                    after: TrackFlags {
                        hidden: !burn,
                        ..before
                    },
                }
            })
            .collect();
        if commands.is_empty() {
            return;
        }
        let command = EditCommand::Composite {
            label: if burn {
                "Show captions"
            } else {
                "Hide captions"
            }
            .into(),
            commands,
        };
        self.apply(Ok(command), cx);
    }
}

/// After a video export: write the captions next to it, when the user asked
/// for a sidecar file. Called by the export dialog.
pub(crate) fn after_export(state: &Arc<AppState>, video: &std::path::Path) -> Option<String> {
    if !SpeechSettings::load().sidecar {
        return None;
    }
    let path = caption_commands::sidecar_path(video, SubtitleFormat::Srt);
    match caption_commands::captions_export(state, &path, Some(SubtitleFormat::Srt)) {
        Ok(_) => Some(path.display().to_string()),
        // No captions is not an error worth showing after a good export.
        Err(error) => {
            tracing::debug!(%error, "no subtitle sidecar written");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rewrap;

    #[test]
    fn an_edited_caption_keeps_its_line_count() {
        assert_eq!(rewrap("one two three four", 2), "one two\nthree four");
        assert_eq!(rewrap("one two three", 1), "one two three");
        assert_eq!(rewrap("single", 2), "single");
    }
}

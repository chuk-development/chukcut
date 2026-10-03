//! Drawing the Captions tab.

use chukcut_engine::modules::captions::emoji::PICKER;
use chukcut_engine::modules::captions::{CaptionMode, CaptionStyle, Placement};
use chukcut_engine::modules::cloud::registry;
use chukcut_engine::modules::project::{TextAlign, TextShadow};
use chukcut_engine::modules::speech::{self, Backend, LocalModel};
use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::Input;
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::switch::Switch;
use gpui::component::{Disableable as _, Icon, Selectable as _, Sizable as _};
use gpui::{AnyElement, ClickEvent};

use super::*;

/// The swatches every colour row offers.
const COLORS: [[f32; 4]; 10] = [
    [1.0, 1.0, 1.0, 1.0],
    [0.0, 0.0, 0.0, 1.0],
    [1.0, 0.86, 0.0, 1.0],
    [1.0, 0.55, 0.0, 1.0],
    [1.0, 0.25, 0.3, 1.0],
    [1.0, 0.4, 0.75, 1.0],
    [0.6, 0.4, 1.0, 1.0],
    [0.2, 0.6, 1.0, 1.0],
    [0.2, 0.9, 0.95, 1.0],
    [0.3, 0.9, 0.4, 1.0],
];

fn hex(color: [f32; 4]) -> u32 {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (c(color[0]) << 16) | (c(color[1]) << 8) | c(color[2])
}

fn same_color(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}

fn label(text: impl Into<SharedString>) -> impl IntoElement {
    div().text_xs().text_color(rgb(TEXT_DIM)).child(text.into())
}

fn section(title: &'static str, body: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(rgb(TEXT)).child(title))
        .child(body)
}

fn row() -> gpui::Div {
    div().flex().flex_row().flex_wrap().items_center().gap_1()
}

/// A pill that is either on or off.
fn chip(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .h(px(24.0))
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .cursor_pointer()
        .text_xs()
        .border_1()
        .when(active, |pill| {
            pill.bg(rgb(PANEL_RAISED))
                .border_color(rgb(ACCENT))
                .text_color(rgb(ACCENT))
        })
        .when(!active, |pill| {
            pill.border_color(rgb(BORDER))
                .text_color(rgb(TEXT))
                .hover(|style| style.bg(rgb(PANEL_RAISED)))
        })
        .on_click(on_click)
        .child(text.into())
}

fn swatch(
    id: impl Into<gpui::ElementId>,
    color: Option<[f32; 4]>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .size(px(20.0))
        .rounded_full()
        .cursor_pointer()
        .border_2()
        .border_color(rgb(if active { ACCENT } else { BORDER }))
        .flex()
        .items_center()
        .justify_center()
        .map(|dot| match color {
            Some(color) => dot.bg(rgb(hex(color))),
            // "None": a crossed-out dot.
            None => dot
                .bg(rgb(PANEL))
                .text_color(rgb(TEXT_DIM))
                .child(Icon::new(Lucide::Ban).size(px(12.0))),
        })
        .on_click(on_click)
}

impl Editor {
    pub(crate) fn render_captions_tab(
        &mut self,
        category: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = match category {
            0 => self.render_auto_captions(cx).into_any_element(),
            1 => self.render_caption_list(cx).into_any_element(),
            2 => self.render_caption_style(cx).into_any_element(),
            _ => self.render_caption_files(cx).into_any_element(),
        };
        div()
            .id("captions-tab")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(div().flex().flex_col().gap_3().pb_2().child(body))
            .into_any_element()
    }

    // --- auto captions ------------------------------------------------------

    fn render_auto_captions(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.captions.settings.clone();
        let backend = settings.backend;

        let transcriber = row()
            .child(chip(
                "cap-backend-cloud",
                "Cloud account",
                backend == Backend::Cloud,
                set(|s| s.backend = Backend::Cloud, cx),
            ))
            .child(chip(
                "cap-backend-local",
                "On this computer",
                backend == Backend::Local,
                set(|s| s.backend = Backend::Local, cx),
            ));

        let source = match backend {
            Backend::Cloud => self.render_accounts(cx).into_any_element(),
            Backend::Local => self.render_local_models(cx).into_any_element(),
        };

        let editor = cx.entity().downgrade();
        let language = settings.language.clone();
        let language_label = match &language {
            Some(code) => speech::language_name(code).to_string(),
            None => "Detect automatically".to_string(),
        };
        let languages = Button::new("cap-language")
            .label(language_label)
            .small()
            .outline()
            .dropdown_menu_with_anchor(gpui::Anchor::TopLeft, move |menu, _, _| {
                let pick = |code: Option<&'static str>| {
                    let editor = editor.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let _ = editor.update(cx, |this, cx| {
                            this.captions.settings.language = code.map(str::to_string);
                            let _ = this.captions.save_settings();
                            cx.notify();
                        });
                    }
                };
                let menu = menu.max_h(px(360.0)).scrollable(true).item(
                    PopupMenuItem::new("Detect automatically")
                        .checked(language.is_none())
                        .on_click(pick(None)),
                );
                speech::LANGUAGES.iter().fold(menu, |menu, (code, name)| {
                    menu.item(
                        PopupMenuItem::new(*name)
                            .checked(language.as_deref() == Some(*code))
                            .on_click(pick(Some(*code))),
                    )
                })
            });

        let mode = settings.mode;
        let words_mode = matches!(mode, CaptionMode::Words { .. });
        let mode_row = row()
            .child(chip(
                "cap-mode-words",
                "Word by word",
                words_mode,
                set(|s| s.mode = CaptionMode::words(1), cx),
            ))
            .child(chip(
                "cap-mode-sentences",
                "Sentences",
                !words_mode,
                set(|s| s.mode = CaptionMode::default(), cx),
            ));
        let mode_detail = match mode {
            CaptionMode::Words { max_words } => row()
                .child(label("Words on screen"))
                .children((1..=4usize).map(|n| {
                    chip(
                        ("cap-words", n),
                        n.to_string(),
                        max_words == n,
                        cx.listener(move |this, _, _, cx| {
                            this.captions.settings.mode = CaptionMode::words(n);
                            let _ = this.captions.save_settings();
                            cx.notify();
                        }),
                    )
                }))
                .into_any_element(),
            CaptionMode::Sentences {
                max_chars_per_line,
                max_lines,
                max_duration,
            } => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    row().child(label("Characters per line")).child(
                        div()
                            .w(px(64.0))
                            .child(Input::new(&self.captions.chars).small()),
                    ),
                )
                .child(row().child(label("Lines")).children((1..=3usize).map(|n| {
                    chip(
                        ("cap-lines", n),
                        n.to_string(),
                        max_lines == n,
                        cx.listener(move |this, _, _, cx| {
                            this.captions.settings.mode = CaptionMode::Sentences {
                                max_chars_per_line,
                                max_lines: n,
                                max_duration,
                            };
                            let _ = this.captions.save_settings();
                            cx.notify();
                        }),
                    )
                })))
                .child(row().child(label("Longest caption")).children(
                    [2i64, 3, 5, 7].into_iter().map(|seconds| {
                        chip(
                            ("cap-seconds", seconds as usize),
                            format!("{seconds} s"),
                            max_duration == seconds * 1_000_000,
                            cx.listener(move |this, _, _, cx| {
                                this.captions.settings.mode = CaptionMode::Sentences {
                                    max_chars_per_line,
                                    max_lines,
                                    max_duration: seconds * 1_000_000,
                                };
                                let _ = this.captions.save_settings();
                                cx.notify();
                            }),
                        )
                    }),
                ))
                .into_any_element(),
        };

        let switches = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                Switch::new("cap-auto-emoji")
                    .checked(settings.auto_emoji)
                    .label("Auto emoji: add a fitting emoji to each caption")
                    .small()
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.captions.settings.auto_emoji = *checked;
                        let _ = this.captions.save_settings();
                        cx.notify();
                    })),
            )
            .child(
                Switch::new("cap-replace")
                    .checked(settings.replace)
                    .label("Clear current captions")
                    .small()
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.captions.settings.replace = *checked;
                        let _ = this.captions.save_settings();
                        cx.notify();
                    })),
            );

        let action = match &self.captions.job {
            Some(job) => {
                let progress = job.progress.lock().clone();
                let fraction = progress.fraction.unwrap_or(0.0).clamp(0.0, 1.0);
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label(progress.label))
                    .child(
                        div()
                            .h(px(6.0))
                            .w_full()
                            .rounded_full()
                            .bg(rgb(PANEL_RAISED))
                            .child(div().h_full().rounded_full().bg(rgb(ACCENT)).w(
                                gpui::relative(if progress.fraction.is_some() {
                                    fraction
                                } else {
                                    0.15
                                }),
                            )),
                    )
                    .child(
                        row().child(
                            Button::new("cap-cancel").label("Cancel").small().on_click(
                                cx.listener(|this, _, _, cx| this.cancel_transcription(cx)),
                            ),
                        ),
                    )
                    .into_any_element()
            }
            None => {
                let ready = match backend {
                    Backend::Cloud => settings.account.is_some(),
                    Backend::Local => speech_commands::speech_local_available(),
                };
                row()
                    .child(
                        Button::new("cap-generate")
                            .label("Generate captions")
                            .icon(Lucide::Captions)
                            .primary()
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, _, cx| this.start_transcription(cx))),
                    )
                    .when(!caption_edit::clips(&self.project).is_empty(), |r| {
                        r.child(
                            Button::new("cap-regroup")
                                .label("Regroup existing")
                                .ghost()
                                .small()
                                .tooltip("Re-split the current captions with these settings")
                                .on_click(cx.listener(|this, _, _, cx| this.regroup_captions(cx))),
                        )
                    })
                    .into_any_element()
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(section("Transcribe with", transcriber))
            .child(source)
            .child(section("Spoken language", row().child(languages)))
            .child(section(
                "Captions",
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(mode_row)
                    .child(mode_detail),
            ))
            .child(switches)
            .child(action)
    }

    fn render_accounts(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.captions.form.is_some() {
            return self.render_account_form(cx).into_any_element();
        }
        let accounts = self.captions.accounts.clone();
        let presets = registry::descriptor(registry::ProviderKind::OpenaiCompatible).presets;
        let add_buttons = row()
            .children(presets.iter().enumerate().map(|(i, preset)| {
                let preset = *preset;
                Button::new(("cap-add-preset", i))
                    .label(format!("Add {}", preset.0))
                    .small()
                    .outline()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_account_form(None, Some(preset), window, cx)
                    }))
            }))
            .child(
                Button::new("cap-add-other")
                    .label("Other OpenAI-compatible")
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_account_form(None, None, window, cx)
                    })),
            );
        if accounts.is_empty() {
            return section(
                "Account",
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label(
                        "Any service with OpenAI's transcription API works: OpenAI (whisper-1) or Groq (whisper-large-v3-turbo, fast and cheap). Your key stays on this computer.",
                    ))
                    .child(add_buttons),
            )
            .into_any_element();
        }

        let selected = self.captions.settings.account.clone();
        let current = accounts
            .iter()
            .find(|a| Some(&a.account.id) == selected.as_ref())
            .cloned();
        let editor = cx.entity().downgrade();
        let picker = Button::new("cap-account")
            .label(
                current
                    .as_ref()
                    .map(|a| a.account.name.clone())
                    .unwrap_or_else(|| "Choose an account".into()),
            )
            .small()
            .outline()
            .dropdown_menu_with_anchor(gpui::Anchor::TopLeft, move |menu, _, _| {
                accounts.iter().fold(menu, |menu, view| {
                    let editor = editor.clone();
                    let id = view.account.id.clone();
                    menu.item(
                        PopupMenuItem::new(format!(
                            "{}  ·  {}",
                            view.account.name, view.account.default_model
                        ))
                        .checked(Some(&id) == selected.as_ref())
                        .on_click(move |_, _, cx| {
                            let id = id.clone();
                            let _ = editor.update(cx, |this, cx| {
                                this.captions.settings.account = Some(id);
                                this.captions.test = None;
                                let _ = this.captions.save_settings();
                                cx.notify();
                            });
                        }),
                    )
                })
            });

        let mut details = div().flex().flex_col().gap_1();
        if let Some(view) = current {
            let id = view.account.id.clone();
            let edit_id = id.clone();
            details = details
                .child(label(format!(
                    "{} · {} · {}",
                    view.account.base_url,
                    if view.account.default_model.is_empty() {
                        "no model"
                    } else {
                        view.account.default_model.as_str()
                    },
                    if view.has_key {
                        format!("key {}", view.key_hint)
                    } else {
                        "no key".into()
                    }
                )))
                .child(
                    row()
                        .child(
                            Button::new("cap-account-edit")
                                .label("Edit")
                                .small()
                                .ghost()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_account_form(Some(edit_id.clone()), None, window, cx)
                                })),
                        )
                        .child(
                            Button::new("cap-account-test")
                                .label(if self.captions.testing {
                                    "Testing…"
                                } else {
                                    "Test connection"
                                })
                                .small()
                                .ghost()
                                .disabled(self.captions.testing)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.test_account(id.clone(), cx)
                                })),
                        ),
                );
        }
        if let Some(report) = &self.captions.test {
            details = details.child(
                div()
                    .text_xs()
                    .text_color(rgb(if report.ok { 0x6fd38a } else { 0xff7a7a }))
                    .child(report.message.clone()),
            );
        }

        section(
            "Account",
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(row().child(picker))
                .child(details)
                .child(add_buttons),
        )
        .into_any_element()
    }

    fn render_account_form(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let editing = self.captions.form.as_ref().and_then(|f| f.id.clone());
        let field = |caption: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(label(caption))
                .child(Input::new(input).small())
        };
        section(
            if editing.is_some() {
                "Edit account"
            } else {
                "New account"
            },
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(field("Name", &self.captions.name))
                .child(field("Base URL (up to /v1)", &self.captions.url))
                .child(field("Model", &self.captions.model))
                .child(field(
                    if editing.is_some() {
                        "API key (leave empty to keep the saved one)"
                    } else {
                        "API key"
                    },
                    &self.captions.key,
                ))
                .child(
                    row()
                        .child(
                            Button::new("cap-form-save-test")
                                .label("Save and test")
                                .small()
                                .primary()
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.save_account(true, cx)),
                                ),
                        )
                        .child(
                            Button::new("cap-form-save").label("Save").small().on_click(
                                cx.listener(|this, _, _, cx| this.save_account(false, cx)),
                            ),
                        )
                        .child(
                            Button::new("cap-form-cancel")
                                .label("Cancel")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.captions.form = None;
                                    cx.notify();
                                })),
                        )
                        .when_some(editing, |r, id| {
                            r.child(
                                Button::new("cap-form-remove")
                                    .label("Remove account")
                                    .small()
                                    .danger()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_account(id.clone(), cx)
                                    })),
                            )
                        }),
                ),
        )
    }

    fn render_local_models(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !speech_commands::speech_local_available() {
            return section(
                "Model",
                label("This build has no offline transcription. Use a cloud account, or build with the local-whisper feature."),
            )
            .into_any_element();
        }
        let current = self.captions.settings.local_model;
        let models = speech_commands::speech_models();
        let editor = cx.entity().downgrade();
        let picker = Button::new("cap-local-model")
            .label(current.label())
            .small()
            .outline()
            .dropdown_menu_with_anchor(gpui::Anchor::TopLeft, move |menu, _, _| {
                models.iter().fold(menu, |menu, info| {
                    let editor = editor.clone();
                    let model: LocalModel = info.model;
                    menu.item(
                        PopupMenuItem::new(if info.downloaded {
                            format!("{} · downloaded", info.label)
                        } else {
                            info.label.to_string()
                        })
                        .checked(model == current)
                        .on_click(move |_, _, cx| {
                            let _ = editor.update(cx, |this, cx| {
                                this.captions.settings.local_model = model;
                                let _ = this.captions.save_settings();
                                cx.notify();
                            });
                        }),
                    )
                })
            });
        let downloaded = current.is_downloaded();
        section(
            "Model",
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(row().child(picker))
                .child(label(if downloaded {
                    "Ready. Runs offline with whisper.cpp; nothing leaves this computer."
                } else {
                    "Downloaded once on first use, checked, and kept in the cache."
                })),
        )
        .into_any_element()
    }

    // --- the caption list ---------------------------------------------------

    fn render_caption_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let clips = caption_edit::clips(&self.project);
        let selected = self.selected_caption();
        let fps = self.project.fps;

        let toolbar = row()
            .child(
                Button::new("cap-add-manual")
                    .label("Add caption")
                    .icon(Lucide::Plus)
                    .small()
                    .tooltip("A new caption at the playhead")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.add_caption_at_playhead(window, cx)),
                    ),
            )
            .when(!clips.is_empty(), |r| {
                r.child(
                    Button::new("cap-clear")
                        .label("Delete all")
                        .icon(Lucide::Trash)
                        .small()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.clear_captions(cx))),
                )
            });

        let editor_block = selected.as_ref().map(|_| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(rgb(PANEL_RAISED))
                .child(Input::new(&self.captions.text).small())
                .child(
                    row()
                        .child(
                            Button::new("cap-split")
                                .label("Split at playhead")
                                .icon(Lucide::Scissors)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.split_caption(window, cx)
                                })),
                        )
                        .child(
                            Button::new("cap-merge")
                                .label("Merge with next")
                                .icon(Lucide::Combine)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.merge_caption_with_next(window, cx)
                                })),
                        )
                        .child(
                            Button::new("cap-emoji")
                                .label("Emoji")
                                .icon(Lucide::Sticker)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.captions.emoji_open = !this.captions.emoji_open;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("cap-delete")
                                .label("Delete")
                                .icon(Lucide::Trash)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| this.delete_caption(cx))),
                        ),
                )
                .when(self.captions.emoji_open, |block| {
                    block.child(self.render_emoji_picker(cx))
                })
        });

        let rows =
            clips.into_iter().enumerate().map(|(i, clip)| {
                let active = selected.as_deref() == Some(clip.segment_id.as_str());
                let id = clip.segment_id.clone();
                div()
                    .id(("cap-row", i))
                    .flex()
                    .flex_row()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(active, |r| {
                        r.bg(rgb(PANEL_RAISED)).border_1().border_color(rgb(ACCENT))
                    })
                    .when(!active, |r| r.hover(|s| s.bg(rgb(PANEL_RAISED))))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_caption(id.clone(), window, cx)
                    }))
                    .child(
                        div()
                            .w(px(64.0))
                            .flex_none()
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .child(timecode(clip.start, fps)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .child(clip.text.replace('\n', " ")),
                    )
            });

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(toolbar)
            .children(editor_block)
            .child(div().flex().flex_col().gap(px(2.0)).children(rows))
            .when(caption_edit::clips(&self.project).is_empty(), |d| {
                d.child(label(
                    "No captions yet. Generate them under Auto captions, import an .srt, or add one by hand.",
                ))
            })
    }

    fn render_emoji_picker(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let category = self.captions.emoji_category.min(PICKER.len() - 1);
        let tabs = row().children(PICKER.iter().enumerate().map(|(i, (name, _))| {
            chip(
                ("cap-emoji-cat", i),
                *name,
                i == category,
                cx.listener(move |this, _, _, cx| {
                    this.captions.emoji_category = i;
                    cx.notify();
                }),
            )
        }));
        let grid = row().children(PICKER[category].1.iter().enumerate().map(|(i, emoji)| {
            let emoji: &'static str = emoji;
            div()
                .id(("cap-emoji-item", i))
                .size(px(30.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .text_lg()
                // Named, so every emoji is the colour glyph: through the UI
                // font's fallback some come out as flat outlines.
                .font_family("Noto Color Emoji")
                .hover(|s| s.bg(rgb(BORDER)))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.insert_emoji(emoji, window, cx)),
                )
                .child(emoji)
        }));
        div().flex().flex_col().gap_1().child(tabs).child(grid)
    }

    // --- style ----------------------------------------------------------------

    fn render_caption_style(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let style = self.current_caption_style();
        let all = self.captions.style_all;
        let has_selection = self.selected_caption().is_some();

        let scope = row()
            .child(label("Apply to"))
            .child(chip(
                "cap-scope-all",
                "All captions",
                all,
                cx.listener(|this, _, _, cx| {
                    this.captions.style_all = true;
                    cx.notify();
                }),
            ))
            .child(chip(
                "cap-scope-one",
                if has_selection {
                    "Selected caption"
                } else {
                    "Selected caption (none)"
                },
                !all,
                cx.listener(|this, _, _, cx| {
                    this.captions.style_all = false;
                    cx.notify();
                }),
            ));

        let presets = row().children(
            CaptionStyle::presets(&self.project.canvas)
                .into_iter()
                .enumerate()
                .map(|(i, (name, preset))| {
                    let keep = style.position;
                    let preset_for_click = preset.clone();
                    div()
                        .id(("cap-preset", i))
                        .w(px(84.0))
                        .h(px(40.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(rgb(if preset.background.is_some() {
                            0x000000
                        } else {
                            0x4a4a4a
                        }))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .hover(|s| s.border_color(rgb(ACCENT)))
                        .text_sm()
                        .when(preset.bold, |d| d.font_weight(gpui::FontWeight::BOLD))
                        .text_color(rgb(hex(preset.highlight.unwrap_or(preset.color))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut chosen = preset_for_click.clone();
                            // A preset is a look, not a place: keep where the
                            // user put the captions unless it moves them on
                            // purpose (the big one sits in the middle).
                            if name != "Big" {
                                chosen.position = keep;
                            }
                            this.restyle_captions(move |s| *s = chosen, cx)
                        }))
                        .child(name)
                }),
        );

        let editor = cx.entity().downgrade();
        let fonts = self.captions.fonts.lock().clone();
        let family = style.font_family.clone();
        let font_picker = Button::new("cap-font")
            .label(if family == "sans-serif" {
                "Default (sans-serif)".to_string()
            } else {
                family.clone()
            })
            .small()
            .outline()
            .dropdown_menu_with_anchor(gpui::Anchor::TopLeft, move |menu, _, _| {
                let pick = |name: String| {
                    let editor = editor.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let name = name.clone();
                        let _ = editor.update(cx, |this, cx| {
                            this.restyle_captions(move |s| s.font_family = name, cx)
                        });
                    }
                };
                let menu = menu.max_h(px(420.0)).scrollable(true).item(
                    PopupMenuItem::new("Default (sans-serif)")
                        .checked(family == "sans-serif")
                        .on_click(pick("sans-serif".into())),
                );
                // The hook for an online font library: once a downloaded
                // family is registered with the text renderer it appears in
                // this list like any installed one.
                fonts.iter().fold(menu, |menu, name| {
                    menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(&family == name)
                            .on_click(pick(name.clone())),
                    )
                })
            });

        let size = style.font_size;
        let step_button = |id: &'static str, icon: Lucide, delta: f32, cx: &mut Context<Self>| {
            Button::new(id)
                .icon(icon)
                .xsmall()
                .ghost()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.restyle_captions(
                        move |s| {
                            let next = (s.font_size + delta).clamp(8.0, 400.0);
                            // The outline keeps its proportion to the text.
                            if s.font_size > 0.0 {
                                s.stroke_width = (s.stroke_width * next / s.font_size).round();
                            }
                            s.font_size = next;
                        },
                        cx,
                    )
                }))
        };
        let size_row = row()
            .child(step_button("cap-size-down", Lucide::Minus, -4.0, cx))
            .child(
                div()
                    .w(px(40.0))
                    .text_center()
                    .text_sm()
                    .child(format!("{}", size.round())),
            )
            .child(step_button("cap-size-up", Lucide::Plus, 4.0, cx))
            .child(chip(
                "cap-bold",
                "Bold",
                style.bold,
                cx.listener(|this, _, _, cx| this.restyle_captions(|s| s.bold = !s.bold, cx)),
            ))
            .child(chip(
                "cap-italic",
                "Italic",
                style.italic,
                cx.listener(|this, _, _, cx| this.restyle_captions(|s| s.italic = !s.italic, cx)),
            ));

        let align_row = row().children(
            [
                (TextAlign::Left, Lucide::TextAlignStart),
                (TextAlign::Center, Lucide::TextAlignCenter),
                (TextAlign::Right, Lucide::TextAlignEnd),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (align, icon))| {
                Button::new(("cap-align", i))
                    .icon(icon)
                    .xsmall()
                    .ghost()
                    .selected(style.align == align)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.restyle_captions(move |s| s.align = align, cx)
                    }))
            }),
        );

        let color_row = |prefix: &'static str,
                         current: Option<[f32; 4]>,
                         allow_none: bool,
                         apply: fn(&mut CaptionStyle, Option<[f32; 4]>),
                         cx: &mut Context<Self>| {
            let mut swatches: Vec<AnyElement> = Vec::new();
            if allow_none {
                swatches.push(
                    swatch(
                        (prefix, 100usize),
                        None,
                        current.is_none(),
                        cx.listener(move |this, _, _, cx| {
                            this.restyle_captions(move |s| apply(s, None), cx)
                        }),
                    )
                    .into_any_element(),
                );
            }
            for (i, color) in COLORS.into_iter().enumerate() {
                swatches.push(
                    swatch(
                        (prefix, i),
                        Some(color),
                        current.is_some_and(|c| same_color(c, color)),
                        cx.listener(move |this, _, _, cx| {
                            this.restyle_captions(move |s| apply(s, Some(color)), cx)
                        }),
                    )
                    .into_any_element(),
                );
            }
            row().children(swatches)
        };

        let text_colors = color_row(
            "cap-color",
            Some(style.color),
            false,
            |s, c| s.color = c.unwrap_or(s.color),
            cx,
        );
        let outline_colors = color_row(
            "cap-stroke",
            (style.stroke_width > 0.0).then_some(style.stroke_color),
            true,
            |s, c| match c {
                Some(c) => {
                    s.stroke_color = c;
                    if s.stroke_width <= 0.0 {
                        s.stroke_width = (s.font_size * 0.07).round().max(1.0);
                    }
                }
                None => s.stroke_width = 0.0,
            },
            cx,
        );
        let outline_width = row()
            .child(label("Width"))
            .child(step_button_width("cap-stroke-down", -1.0, cx))
            .child(
                div()
                    .w(px(28.0))
                    .text_center()
                    .text_sm()
                    .child(format!("{}", style.stroke_width.round())),
            )
            .child(step_button_width("cap-stroke-up", 1.0, cx));
        let box_colors = color_row(
            "cap-box",
            style.background.map(|c| [c[0], c[1], c[2], 1.0]),
            true,
            |s, c| s.background = c.map(|c| [c[0], c[1], c[2], 0.75]),
            cx,
        );
        let highlight_colors = color_row(
            "cap-highlight",
            style.highlight,
            true,
            |s, c| s.highlight = c,
            cx,
        );

        let shadow = Switch::new("cap-shadow")
            .checked(style.shadow.is_some())
            .label("Drop shadow")
            .small()
            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                let on = *checked;
                this.restyle_captions(
                    move |s| {
                        s.shadow = on.then(|| TextShadow {
                            color: [0.0, 0.0, 0.0, 0.6],
                            offset: [(s.font_size / 14.0).round(), (s.font_size / 14.0).round()],
                            blur: (s.font_size / 10.0).round(),
                        })
                    },
                    cx,
                )
            }));

        let placement = Placement::of(style.position[1]);
        let position = row()
            .children(Placement::ALL.into_iter().enumerate().map(|(i, p)| {
                chip(
                    ("cap-place", i),
                    p.label(),
                    placement == Some(p),
                    cx.listener(move |this, _, _, cx| {
                        this.restyle_captions(move |s| s.position = [0.0, p.y()], cx)
                    }),
                )
            }))
            .child(label("or drag the caption on the player"));

        let words = caption_edit::clips(&self.project)
            .iter()
            .any(|c| c.words > 0);

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(scope)
            .child(section("Presets", presets))
            .child(section("Font", row().child(font_picker)))
            .child(section("Size", size_row))
            .child(section("Alignment", align_row))
            .child(section("Colour", text_colors))
            .child(section(
                "Outline",
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(outline_colors)
                    .child(outline_width),
            ))
            .child(shadow)
            .child(section("Background box", box_colors))
            .child(section(
                "Highlight the spoken word",
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(highlight_colors)
                    .child(label(if words {
                        "Karaoke: the word being said lights up in this colour."
                    } else {
                        "Needs word timing: generate captions rather than importing an .srt."
                    })),
            ))
            .child(section("Position", position))
    }

    // --- files ----------------------------------------------------------------

    fn render_caption_files(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let has = !caption_edit::clips(&self.project).is_empty();
        let burned = self
            .project
            .tracks
            .iter()
            .filter(|t| caption_edit::is_caption_track(&self.project, t))
            .all(|t| !t.hidden);
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(section(
                "Import",
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(row().child(
                        Button::new("cap-import")
                            .label("Import .srt or .vtt")
                            .icon(Lucide::FileUp)
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.import_subtitles(cx))),
                    ))
                    .child(label(
                        "Each subtitle becomes a caption you can edit and style. \"Clear current captions\" under Auto captions decides whether existing ones are replaced.",
                    )),
            ))
            .child(section(
                "Export subtitles",
                row()
                    .child(
                        Button::new("cap-export-srt")
                            .label("Export .srt")
                            .icon(Lucide::FileDown)
                            .small()
                            .disabled(!has)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export_subtitles(SubtitleFormat::Srt, cx)
                            })),
                    )
                    .child(
                        Button::new("cap-export-vtt")
                            .label("Export .vtt")
                            .small()
                            .ghost()
                            .disabled(!has)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export_subtitles(SubtitleFormat::Vtt, cx)
                            })),
                    ),
            ))
            .child(section(
                "With the video",
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        Switch::new("cap-burn")
                            .checked(burned)
                            .label("Burn captions into exported videos")
                            .small()
                            .disabled(!has)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.set_burn_in(*checked, cx)
                            })),
                    )
                    .child(
                        Switch::new("cap-sidecar")
                            .checked(self.captions.settings.sidecar)
                            .label("Also write an .srt next to every exported video")
                            .small()
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.captions.settings.sidecar = *checked;
                                let _ = this.captions.save_settings();
                                cx.notify();
                            })),
                    )
                    .child(label(
                        "Burned-in captions are drawn into the picture. Turning it off hides the caption lane, so the export carries none; the sidecar file works either way.",
                    )),
            ))
    }
}

/// A click that changes one setting and saves.
fn set(
    change: fn(&mut SpeechSettings),
    cx: &mut Context<Editor>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let editor = cx.entity().downgrade();
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        let _ = editor.update(cx, |this, cx| {
            change(&mut this.captions.settings);
            let _ = this.captions.save_settings();
            cx.notify();
        });
    }
}

fn step_button_width(id: &'static str, delta: f32, cx: &mut Context<Editor>) -> impl IntoElement {
    Button::new(id)
        .icon(if delta < 0.0 {
            Lucide::Minus
        } else {
            Lucide::Plus
        })
        .xsmall()
        .ghost()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.restyle_captions(
                move |s| s.stroke_width = (s.stroke_width + delta).clamp(0.0, 60.0),
                cx,
            )
        }))
}

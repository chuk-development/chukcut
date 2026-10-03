//! Settings → Accounts: the cloud services the user brings a key for.
//!
//! List, add, edit, test, delete. Keys go into the engine's key store
//! (`secrets.toml`, owner-only) and never come back out: the list shows the
//! last four characters, and an edit with an empty key field keeps the saved
//! key. Everything here calls `cloud::commands`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use chukcut_engine::modules::cloud::commands::{self as cloud_commands, AccountView};
use chukcut_engine::modules::cloud::registry::{descriptor, ProviderKind};
use chukcut_engine::modules::cloud::{CloudStore, TestReport};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputState};
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::{Disableable as _, Sizable as _};
use gpui::{AnyElement, Entity};

use super::*;
use crate::ui::{Badge, Tone};

/// Bumped whenever an account is added, changed or removed, so panels that
/// list accounts know to read them again.
pub(crate) static ACCOUNTS_CHANGED: AtomicU64 = AtomicU64::new(0);

pub(crate) fn accounts_generation() -> u64 {
    ACCOUNTS_CHANGED.load(Ordering::Relaxed)
}

fn changed() {
    ACCOUNTS_CHANGED.fetch_add(1, Ordering::Relaxed);
}

enum TestState {
    Running,
    Done(TestReport),
}

struct Form {
    /// `None` while adding.
    id: Option<String>,
    kind: ProviderKind,
    name: Entity<InputState>,
    url: Entity<InputState>,
    model: Entity<InputState>,
    key: Entity<InputState>,
}

pub(crate) struct AccountsSettings {
    accounts: Vec<AccountView>,
    form: Option<Form>,
    tests: HashMap<String, TestState>,
    notice: Option<SharedString>,
}

impl AccountsSettings {
    pub(crate) fn new(_: &mut Context<Self>) -> Self {
        Self {
            accounts: cloud_commands::cloud_accounts(&CloudStore::user(), None),
            form: None,
            tests: HashMap::new(),
            notice: None,
        }
    }

    fn reload(&mut self) {
        self.accounts = cloud_commands::cloud_accounts(&CloudStore::user(), None);
        changed();
    }

    fn open_form(
        &mut self,
        kind: ProviderKind,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = id
            .as_ref()
            .and_then(|id| self.accounts.iter().find(|a| &a.account.id == id))
            .map(|a| a.account.clone())
            .unwrap_or_else(|| cloud_commands::cloud_account_new_of(kind));
        let input =
            |value: String, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
                let placeholder = placeholder.to_string();
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            };
        let name = input(existing.name.clone(), "Name", window, cx);
        let url = input(
            existing.base_url.clone(),
            "https://api.example.com/v1",
            window,
            cx,
        );
        let model = input(existing.default_model.clone(), "Default model", window, cx);
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(if id.is_some() {
                    "Leave empty to keep the saved key"
                } else {
                    "Paste the API key"
                })
                .masked(true)
        });
        self.form = Some(Form {
            id,
            kind,
            name,
            url,
            model,
            key,
        });
        self.notice = None;
        cx.notify();
    }

    fn save(&mut self, test: bool, cx: &mut Context<Self>) {
        let Some(form) = &self.form else {
            return;
        };
        let value =
            |input: &Entity<InputState>, cx: &App| input.read(cx).value().trim().to_string();
        let store = CloudStore::user();
        let mut account = form
            .id
            .as_ref()
            .and_then(|id| store.account(id))
            .unwrap_or_else(|| cloud_commands::cloud_account_new_of(form.kind));
        let d = descriptor(form.kind);
        let name = value(&form.name, cx);
        account.name = if name.is_empty() {
            d.name.to_string()
        } else {
            name
        };
        if d.custom_url {
            account.base_url = value(&form.url, cx);
        }
        if d.has_model {
            account.default_model = value(&form.model, cx);
        }
        let key = value(&form.key, cx);
        if form.id.is_none() && key.is_empty() && d.needs_key {
            self.notice = Some(format!("{} needs an API key", d.name).into());
            cx.notify();
            return;
        }
        let key = (!key.is_empty()).then_some(key);
        match cloud_commands::cloud_account_set(&store, account, key.as_deref()) {
            Ok(id) => {
                self.form = None;
                self.notice = None;
                self.reload();
                if test {
                    self.test(id, cx);
                }
            }
            Err(error) => self.notice = Some(error.into()),
        }
        cx.notify();
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        if let Err(error) = cloud_commands::cloud_account_remove(&CloudStore::user(), &id) {
            self.notice = Some(error.into());
        }
        self.tests.remove(&id);
        self.form = None;
        self.reload();
        cx.notify();
    }

    fn test(&mut self, id: String, cx: &mut Context<Self>) {
        self.tests.insert(id.clone(), TestState::Running);
        let task_id = id.clone();
        let task = cx.background_spawn(async move {
            cloud_commands::cloud_account_test(&CloudStore::user(), &task_id)
        });
        cx.spawn(async move |this, cx| {
            let report = task.await;
            let _ = this.update(cx, |this, cx| {
                this.tests.insert(id, TestState::Done(report));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_row(&self, view: &AccountView, cx: &mut Context<Self>) -> AnyElement {
        let id = view.account.id.clone();
        let kind = view.account.kind;
        let d = descriptor(kind);
        let detail = {
            let mut parts = Vec::new();
            if d.custom_url {
                parts.push(view.account.base_url.clone());
            }
            if d.has_model && !view.account.default_model.is_empty() {
                parts.push(view.account.default_model.clone());
            }
            parts.push(if view.has_key {
                format!("key {}", view.key_hint)
            } else if d.needs_key {
                "no key".into()
            } else {
                "no key needed".into()
            });
            parts.join(" \u{b7} ")
        };
        let test = self.tests.get(&id);
        let result = match test {
            Some(TestState::Done(report)) => Some(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(if report.ok { SUCCESS } else { DANGER }))
                    .child(report.message.clone()),
            ),
            _ => None,
        };
        let running = matches!(test, Some(TestState::Running));
        let (test_id, edit_id, remove_id) = (id.clone(), id.clone(), id.clone());
        div()
            .id(SharedString::from(format!("account-{id}")))
            .px_3()
            .py_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(PANEL))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().child(view.account.name.clone()))
                            .child(Badge::new(d.name)),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_CAPTION))
                            .text_color(rgb(TEXT_DIM))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(detail),
                    )
                    .children(result),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .child(
                        Button::new(SharedString::from(format!("account-test-{id}")))
                            .label(if running { "Testing\u{2026}" } else { "Test" })
                            .small()
                            .disabled(running)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.test(test_id.clone(), cx)),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("account-edit-{id}")))
                            .label("Edit")
                            .small()
                            .ghost()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_form(kind, Some(edit_id.clone()), window, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("account-remove-{id}")))
                            .label("Delete")
                            .small()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove(remove_id.clone(), cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_form(&self, form: &Form, cx: &mut Context<Self>) -> AnyElement {
        let d = descriptor(form.kind);
        let field = |caption: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(TEXT_DIM))
                        .child(caption),
                )
                .child(Input::new(input).small())
        };
        let key_page = d.key_page;
        div()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().font_weight(gpui::FontWeight::MEDIUM).child(
                        if form.id.is_some() {
                            format!("Edit {} account", d.name)
                        } else {
                            format!("New {} account", d.name)
                        },
                    ))
                    .child(div().flex_1())
                    .child(
                        Button::new("account-key-page")
                            .label("Get a key")
                            .small()
                            .ghost()
                            .on_click(move |_, _, cx| cx.open_url(key_page)),
                    ),
            )
            .child(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_DIM))
                    .child(d.help),
            )
            .child(field("Name", &form.name))
            .when(d.custom_url, |f| {
                f.child(field("Base URL (up to /v1)", &form.url))
            })
            .when(d.has_model, |f| {
                f.child(field("Default model", &form.model))
            })
            .child(field(
                if d.needs_key {
                    "API key (stays on this computer)"
                } else {
                    "API key (optional for a local server)"
                },
                &form.key,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        Button::new("account-save-test")
                            .label("Save and test")
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| this.save(true, cx))),
                    )
                    .child(
                        Button::new("account-save")
                            .label("Save")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.save(false, cx))),
                    )
                    .child(
                        Button::new("account-cancel")
                            .label("Cancel")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.form = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for AccountsSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let add = Button::new("account-add")
            .label("Add account")
            .small()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                ProviderKind::ALL.iter().fold(menu, |menu, kind| {
                    let (this, kind) = (this.clone(), *kind);
                    menu.item(PopupMenuItem::new(descriptor(kind).name).on_click(
                        move |_, window, cx| {
                            let _ = this.update(cx, |settings, cx| {
                                settings.open_form(kind, None, window, cx)
                            });
                        },
                    ))
                })
            });
        let rows: Vec<AnyElement> = self
            .accounts
            .clone()
            .iter()
            .map(|view| self.render_row(view, cx))
            .collect();
        let empty = rows.is_empty();
        let form = self.form.as_ref().map(|form| self.render_form(form, cx));
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .pb_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(ACCENT))
                            .child("ACCOUNTS"),
                    )
                    .child(add),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded_md()
                    .bg(rgb(PANEL_RAISED))
                    .when(empty && form.is_none(), |list| {
                        list.child(
                            div()
                                .px_3()
                                .py_3()
                                .text_size(px(TEXT_LABEL))
                                .text_color(rgb(TEXT_DIM))
                                .child("No accounts yet. Add one to use voices, sound effects, stock media, fal.ai or caption translation with your own key. Keys stay on this computer, in a file only you can read."),
                        )
                    })
                    .children(rows)
                    .children(form)
                    .children(self.notice.clone().map(|notice| {
                        div()
                            .px_3()
                            .py_2()
                            .text_size(px(TEXT_CAPTION))
                            .text_color(rgb(WARNING))
                            .child(notice)
                    })),
            )
    }
}

/// A short tone badge for a licence state, shared by the panels.
pub(crate) fn licence_badge(
    commercial: chukcut_engine::modules::cloud::provenance::Commercial,
    attribution: bool,
) -> Badge {
    use chukcut_engine::modules::cloud::provenance::Commercial;
    match commercial {
        Commercial::No => Badge::new("Non-commercial").tone(Tone::Danger),
        Commercial::Unknown => Badge::new("Check terms").tone(Tone::Warning),
        Commercial::Yes if attribution => Badge::new("Credit needed").tone(Tone::Accent),
        Commercial::Yes => Badge::new("Free to use").tone(Tone::Success),
    }
}

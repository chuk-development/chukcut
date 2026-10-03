//! "Translate captions" in the Captions tab: the first caption lane into a
//! new lane in another language, through DeepL or an OpenAI-compatible chat
//! model. One undo step.

use chukcut_engine::modules::cloud::registry::ProviderKind;
use chukcut_engine::modules::speech::{language_name, LANGUAGES};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::Input;
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::{Disableable as _, Sizable as _};

use super::*;

impl Editor {
    pub(crate) fn render_translate_captions(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.assets.cloud.refresh_accounts();
        let able = self.assets.cloud.able(Capability::Translate);
        let Some(account) = self.assets.cloud.chosen_of("translate", &able) else {
            return no_account(
                "translate-empty",
                "Translate captions",
                "DeepL or OpenAI-compatible",
                cx,
            );
        };
        let chat = account.account.kind == ProviderKind::OpenaiCompatible;
        let cloud = &self.assets.cloud;
        let target = cloud.translate_target.clone();
        let editor = cx.entity().downgrade();
        let current = target.clone();
        let language = Button::new("translate-language")
            .label(language_name(&target).to_string())
            .small()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                LANGUAGES.iter().fold(menu, |menu, (code, name)| {
                    let editor = editor.clone();
                    let code = code.to_string();
                    menu.item(PopupMenuItem::new(*name).checked(code == current).on_click(
                        move |_, _, cx| {
                            let code = code.clone();
                            let _ = editor.update(cx, |this, cx| {
                                this.assets.cloud.translate_target = code;
                                cx.notify();
                            });
                        },
                    ))
                })
            });
        let busy = cloud.translating;
        let has_captions =
            chukcut_engine::modules::captions::edit::caption_lane(&self.project).is_some();
        column()
            .gap(px(10.0))
            .child(
                row()
                    .child(label("Account"))
                    .child(account_picker("translate", able, &account, cx)),
            )
            .child(row().child(label("Into")).child(language))
            .when(chat, |c| {
                c.child(
                    column()
                        .gap(px(4.0))
                        .child(label("Chat model"))
                        .child(Input::new(&cloud.translate_model).small())
                        .child(hint("Empty uses gpt-4o-mini on OpenAI, or the account's model elsewhere.")),
                )
            })
            .child(hint(
                "The captions of the first caption lane are translated line by line onto a new lane above them, with the same timing. Ctrl+Z takes it back.",
            ))
            .child(
                row().child(
                    Button::new("translate-run")
                        .label(if busy { "Translating\u{2026}" } else { "Translate captions" })
                        .small()
                        .primary()
                        .disabled(busy || !has_captions)
                        .on_click(cx.listener(|this, _, _, cx| this.translate_captions(cx))),
                ),
            )
            .when(!has_captions, |c| c.child(hint("There are no captions yet: make them under Auto captions first.")))
            .into_any_element()
    }

    fn translate_captions(&mut self, cx: &mut Context<Self>) {
        let able = self.assets.cloud.able(Capability::Translate);
        let Some(account) = self.assets.cloud.chosen_of("translate", &able) else {
            return;
        };
        let state = Arc::clone(&self.state);
        let target = self.assets.cloud.translate_target.clone();
        let model = self
            .assets
            .cloud
            .translate_model
            .read(cx)
            .value()
            .trim()
            .to_string();
        let id = account.account.id.clone();
        self.assets.cloud.translating = true;
        self.status = Some(
            format!(
                "Translating captions into {}\u{2026}",
                language_name(&target)
            )
            .into(),
        );
        self.cloud_task(
            cx,
            move || {
                cloud_commands::cloud_translate_captions(
                    &state,
                    &CloudStore::user(),
                    &id,
                    &target,
                    None,
                    &model,
                )
            },
            move |editor, result, cx| {
                editor.assets.cloud.translating = false;
                editor.refresh(cx);
                match result {
                    Ok(done) => {
                        editor.status = Some(
                            format!(
                                "Translated {} caption{} onto a new lane",
                                done.lines,
                                if done.lines == 1 { "" } else { "s" }
                            )
                            .into(),
                        );
                    }
                    Err(error) => editor.report(Err(error), cx),
                }
            },
        );
        cx.notify();
    }
}

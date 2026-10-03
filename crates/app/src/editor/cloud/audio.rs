//! The Audio tab's cloud categories: Text to speech, Sound effects, Music.

use chukcut_engine::modules::cloud::audition;
use chukcut_engine::modules::cloud::provenance::Commercial;
use chukcut_engine::modules::cloud::registry::ProviderKind;
use chukcut_engine::modules::cloud::{AudioGenRequest, TtsRequest};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, Textarea};
use gpui::component::switch::Switch;
use gpui::component::{Disableable as _, Sizable as _};

use super::*;

/// The Audio tab's categories after Import and Project audio.
pub(crate) const AUDIO_CATEGORIES: [&str; 3] = ["Text to speech", "Sound effects", "Music"];

impl Editor {
    /// One of [`AUDIO_CATEGORIES`], by index.
    pub(crate) fn render_cloud_audio(
        &mut self,
        which: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.assets.cloud.refresh_accounts();
        let body = match which {
            0 => self.render_tts(cx),
            1 => self.render_audio_gen(Capability::SoundEffects, cx),
            _ => self.render_audio_gen(Capability::Music, cx),
        };
        div()
            .id("cloud-audio")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(
                column()
                    .gap(px(12.0))
                    .pb(px(12.0))
                    .child(body)
                    .child(self.render_recent(cx)),
            )
            .into_any_element()
    }

    fn render_tts(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let able = self.assets.cloud.able(Capability::Tts);
        let Some(account) = self.assets.cloud.chosen_of("tts", &able) else {
            return no_account(
                "tts-empty",
                "Text to speech",
                "ElevenLabs or OpenAI-compatible",
                cx,
            );
        };
        let id = account.account.id.clone();
        let eleven = account.account.kind == ProviderKind::Elevenlabs;

        // The voice list, fetched once per account.
        if !self.assets.cloud.voices.contains_key(&id) {
            self.assets.cloud.voices.insert(id.clone(), Voices::Loading);
            let fetch = id.clone();
            self.cloud_task(
                cx,
                move || cloud_commands::cloud_voices(&CloudStore::user(), &fetch),
                move |editor, result, _| {
                    let voices = match result {
                        Ok(list) => Voices::Ready(list),
                        Err(error) => Voices::Failed(error),
                    };
                    editor.assets.cloud.voices.insert(id, voices);
                },
            );
        }
        let cloud = &self.assets.cloud;
        let voices = match cloud.voices.get(&account.account.id) {
            Some(Voices::Ready(list)) if !list.is_empty() => {
                let rows: Vec<AnyElement> = list
                    .iter()
                    .take(200)
                    .map(|voice| {
                        let picked = cloud.voice.as_deref() == Some(voice.id.as_str());
                        let pick = voice.id.clone();
                        let preview = voice.preview_url.clone();
                        div()
                            .id(SharedString::from(format!("voice-{}", voice.id)))
                            .h(px(36.0))
                            .flex_none()
                            .px(px(8.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .rounded(px(R_SM))
                            .cursor_pointer()
                            .border_1()
                            .border_color(rgb(if picked { ACCENT } else { HAIRLINE }))
                            .when(picked, |r| r.bg(accent_soft()))
                            .when(!picked, |r| r.hover(|s| s.bg(rgb(PANEL_RAISED))))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.assets.cloud.voice = Some(pick.clone());
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_row()
                                    .items_baseline()
                                    .gap(px(8.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(
                                        div()
                                            .text_size(px(TEXT_BODY))
                                            .text_color(rgb(TEXT))
                                            .child(voice.name.clone()),
                                    )
                                    .child(hint(voice.description.clone())),
                            )
                            .children(preview.map(|url| {
                                Button::new(SharedString::from(format!("voice-play-{}", voice.id)))
                                    .label("Play")
                                    .xsmall()
                                    .ghost()
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        audition::play(&url);
                                    })
                            }))
                            .into_any_element()
                    })
                    .collect();
                column()
                    .gap(px(4.0))
                    .child(label("Voice"))
                    .child(
                        div()
                            .id("voice-list")
                            .max_h(px(220.0))
                            .overflow_y_scroll()
                            .child(column().gap(px(4.0)).children(rows)),
                    )
                    .into_any_element()
            }
            Some(Voices::Loading) => hint("Loading voices\u{2026}").into_any_element(),
            Some(Voices::Failed(error)) => column()
                .child(label("Voice"))
                .child(
                    div()
                        .text_size(px(TEXT_CAPTION))
                        .text_color(rgb(DANGER))
                        .child(error.clone()),
                )
                .child(Input::new(&cloud.voice_typed).small())
                .into_any_element(),
            _ => column()
                .gap(px(4.0))
                .child(label("Voice"))
                .child(Input::new(&cloud.voice_typed).small())
                .child(hint("This server lists no voices; type the voice's name."))
                .into_any_element(),
        };

        let settings = if eleven {
            let (stability, similarity, speed) = (cloud.stability, cloud.similarity, cloud.speed);
            column()
                .child(
                    row()
                        .child(label("Stability").w(px(84.0)))
                        .child(stepper(
                            "tts-stability",
                            format!("{stability:.2}"),
                            |e, d, _| {
                                let c = &mut e.assets.cloud;
                                c.stability = (c.stability + d * 0.05).clamp(0.0, 1.0);
                            },
                            cx,
                        ))
                        .child(hint("lower is more expressive")),
                )
                .child(row().child(label("Similarity").w(px(84.0))).child(stepper(
                    "tts-similarity",
                    format!("{similarity:.2}"),
                    |e, d, _| {
                        let c = &mut e.assets.cloud;
                        c.similarity = (c.similarity + d * 0.05).clamp(0.0, 1.0);
                    },
                    cx,
                )))
                .child(row().child(label("Speed").w(px(84.0))).child(stepper(
                    "tts-speed",
                    format!("{speed:.2}\u{d7}"),
                    |e, d, _| {
                        let c = &mut e.assets.cloud;
                        c.speed = (c.speed + d * 0.05).clamp(0.7, 1.2);
                    },
                    cx,
                )))
                .child(
                    Switch::new("tts-captions")
                        .checked(cloud.tts_captions)
                        .label("Captions from the voice's word timing")
                        .small()
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.assets.cloud.tts_captions = *checked;
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else {
            let speed = cloud.speed;
            column()
                .child(
                    row()
                        .child(label("Speed").w(px(84.0)))
                        .child(stepper(
                            "tts-speed",
                            format!("{speed:.2}\u{d7}"),
                            |e, d, _| {
                                let c = &mut e.assets.cloud;
                                c.speed = (c.speed + d * 0.05).clamp(0.25, 4.0);
                            },
                            cx,
                        )),
                )
                .child(hint(
                    "No word timing from this provider: run Auto captions on the voiceover for captions.",
                ))
                .into_any_element()
        };

        let busy = cloud.busy.is_some();
        column()
            .gap(px(10.0))
            .child(
                row()
                    .child(label("Account"))
                    .child(account_picker("tts", able, &account, cx)),
            )
            .child(Textarea::new(&cloud.tts_text).h(px(96.0)))
            .child(voices)
            .child(settings)
            .child(
                row()
                    .child(
                        Button::new("tts-generate")
                            .label(if busy {
                                "Generating\u{2026}"
                            } else {
                                "Generate and add at playhead"
                            })
                            .small()
                            .primary()
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.generate_speech(true, cx))),
                    )
                    .child(
                        Button::new("tts-generate-library")
                            .label("Generate")
                            .small()
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.generate_speech(false, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn generate_speech(&mut self, place: bool, cx: &mut Context<Self>) {
        let able = self.assets.cloud.able(Capability::Tts);
        let Some(account) = self.assets.cloud.chosen_of("tts", &able) else {
            return;
        };
        let cloud = &self.assets.cloud;
        let text = cloud.tts_text.read(cx).value().trim().to_string();
        let listed = matches!(cloud.voices.get(&account.account.id), Some(Voices::Ready(l)) if !l.is_empty());
        let voice = if listed {
            cloud.voice.clone().unwrap_or_default()
        } else {
            cloud.voice_typed.read(cx).value().trim().to_string()
        };
        if text.is_empty() {
            self.report(Err("Type the text first".into()), cx);
            return;
        }
        if voice.is_empty() {
            self.report(Err("Pick a voice first".into()), cx);
            return;
        }
        let mut request = TtsRequest::new(text, voice);
        request.stability = cloud.stability;
        request.similarity = cloud.similarity;
        request.speed = cloud.speed;
        request.with_timing = account.account.kind == ProviderKind::Elevenlabs;
        let captions = cloud.tts_captions && request.with_timing;
        let at = self.clock.position();
        let id = account.account.id.clone();
        self.assets.cloud.busy = Some("Speaking".into());
        self.status = Some("Generating the voice\u{2026}".into());
        self.cloud_task(
            cx,
            move || {
                cloud_commands::cloud_tts(
                    &CloudStore::user(),
                    &id,
                    &request,
                    &cloud_commands::generated_root(),
                )
            },
            move |editor, result, cx| {
                editor.assets.cloud.busy = None;
                match result {
                    Ok(generated) => {
                        let words = if captions {
                            generated.words.clone()
                        } else {
                            Vec::new()
                        };
                        let placement = if place {
                            Placement::At(at)
                        } else {
                            Placement::Library
                        };
                        editor.cloud_import(generated.path.clone(), placement, words, cx);
                        editor.assets.cloud.recent.insert(0, generated);
                    }
                    Err(error) => editor.report(Err(error), cx),
                }
            },
        );
        cx.notify();
    }

    fn render_audio_gen(&mut self, capability: Capability, cx: &mut Context<Self>) -> AnyElement {
        let music = capability == Capability::Music;
        let slot = if music { "music" } else { "sfx" };
        let able = self.assets.cloud.able(capability);
        let Some(account) = self.assets.cloud.chosen_of(slot, &able) else {
            return no_account(
                if music { "music-empty" } else { "sfx-empty" },
                if music {
                    "Generate music"
                } else {
                    "Sound effects"
                },
                "ElevenLabs",
                cx,
            );
        };
        let cloud = &self.assets.cloud;
        let busy = cloud.busy.is_some();
        let seconds = if music {
            cloud.music_seconds
        } else {
            cloud.sfx_seconds
        };
        let length = if seconds <= 0.0 {
            "Auto".to_string()
        } else {
            format!("{seconds:.0} s")
        };
        let toggle = if music {
            Switch::new("music-instrumental")
                .checked(cloud.instrumental)
                .label("Instrumental, no vocals")
                .small()
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    this.assets.cloud.instrumental = *checked;
                    cx.notify();
                }))
        } else {
            Switch::new("sfx-loop")
                .checked(cloud.sfx_loop)
                .label("Seamless loop")
                .small()
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    this.assets.cloud.sfx_loop = *checked;
                    cx.notify();
                }))
        };
        column()
            .gap(px(10.0))
            .child(
                row()
                    .child(label("Account"))
                    .child(account_picker(slot, able, &account, cx)),
            )
            .child(
                Input::new(if music {
                    &cloud.music_prompt
                } else {
                    &cloud.sfx_prompt
                })
                .small(),
            )
            .child(row().child(label("Length").w(px(84.0))).child(stepper(
                if music { "music-length" } else { "sfx-length" },
                length,
                move |e, d, _| {
                    let c = &mut e.assets.cloud;
                    if music {
                        c.music_seconds = (c.music_seconds + d * 5.0).clamp(5.0, 600.0);
                    } else {
                        // 0 is "let the model choose".
                        c.sfx_seconds = (c.sfx_seconds + d).clamp(0.0, 30.0);
                    }
                },
                cx,
            )))
            .child(toggle)
            .child(hint(if music {
                "ElevenLabs Music: commercial use needs a paid plan (Starter and up)."
            } else {
                "ElevenLabs sound effects, 0.5 to 30 seconds."
            }))
            .child(
                row()
                    .child(
                        Button::new(SharedString::from(format!("{slot}-generate")))
                            .label(if busy {
                                "Generating\u{2026}"
                            } else {
                                "Generate and add at playhead"
                            })
                            .small()
                            .primary()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.generate_audio(capability, true, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{slot}-generate-library")))
                            .label("Generate")
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.generate_audio(capability, false, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn generate_audio(&mut self, capability: Capability, place: bool, cx: &mut Context<Self>) {
        let music = capability == Capability::Music;
        let able = self.assets.cloud.able(capability);
        let Some(account) = self
            .assets
            .cloud
            .chosen_of(if music { "music" } else { "sfx" }, &able)
        else {
            return;
        };
        let cloud = &self.assets.cloud;
        let prompt = if music {
            &cloud.music_prompt
        } else {
            &cloud.sfx_prompt
        }
        .read(cx)
        .value()
        .trim()
        .to_string();
        if prompt.is_empty() {
            self.report(Err("Describe the sound first".into()), cx);
            return;
        }
        let seconds = if music {
            cloud.music_seconds
        } else {
            cloud.sfx_seconds
        };
        let request = AudioGenRequest {
            prompt,
            duration_seconds: (seconds > 0.0).then_some(seconds),
            looping: !music && cloud.sfx_loop,
            instrumental: music && cloud.instrumental,
            prompt_influence: None,
        };
        let at = self.clock.position();
        let id = account.account.id.clone();
        self.assets.cloud.busy = Some("Generating".into());
        self.status = Some(
            if music {
                "Composing\u{2026} music can take a minute"
            } else {
                "Generating the sound\u{2026}"
            }
            .into(),
        );
        self.cloud_task(
            cx,
            move || {
                cloud_commands::cloud_generate_audio(
                    &CloudStore::user(),
                    &id,
                    capability,
                    &request,
                    &cloud_commands::generated_root(),
                )
            },
            move |editor, result, cx| {
                editor.assets.cloud.busy = None;
                match result {
                    Ok(generated) => {
                        let placement = if place {
                            Placement::At(at)
                        } else {
                            Placement::Library
                        };
                        editor.cloud_import(generated.path.clone(), placement, Vec::new(), cx);
                        editor.assets.cloud.recent.insert(0, generated);
                    }
                    Err(error) => editor.report(Err(error), cx),
                }
            },
        );
        cx.notify();
    }

    /// What this session generated: play it, put it at the playhead.
    fn render_recent(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let recent = self.assets.cloud.recent.clone();
        if recent.is_empty() {
            return div().into_any_element();
        }
        let rows = recent
            .into_iter()
            .take(12)
            .enumerate()
            .map(|(i, generated)| {
                let path = generated.path.clone();
                let play = path.to_string_lossy().to_string();
                let words = generated.words.clone();
                let origin = &generated.origin;
                let badge = super::super::accounts::licence_badge(
                    origin.licence.commercial,
                    origin.licence.attribution_required,
                );
                let plan = origin
                    .account_tier
                    .as_deref()
                    .map(|t| format!("{t} plan"))
                    .unwrap_or_default();
                div()
                    .id(SharedString::from(format!("generated-{i}")))
                    .px(px(8.0))
                    .py(px(6.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(px(R_SM))
                    .bg(rgb(PANEL_RAISED))
                    .border_1()
                    .border_color(rgb(HAIRLINE))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(TEXT_BODY))
                                    .child(origin.title.clone()),
                            )
                            .child(row().child(badge).child(hint(format!(
                                "{} {}",
                                chukcut_engine::modules::cloud::provenance::provider_name(
                                    &origin.provider
                                ),
                                plan
                            )))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("generated-play-{i}")))
                            .label("Play")
                            .xsmall()
                            .ghost()
                            .on_click(move |_, _, _| audition::play(&play)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("generated-add-{i}")))
                            .label("Add at playhead")
                            .xsmall()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let at = this.clock.position();
                                this.cloud_import(
                                    path.clone(),
                                    Placement::At(at),
                                    words.clone(),
                                    cx,
                                );
                            })),
                    )
                    .into_any_element()
            });
        let warn = self
            .assets
            .cloud
            .recent
            .iter()
            .any(|g| g.origin.licence.commercial == Commercial::No);
        column()
            .gap(px(6.0))
            .child(label("Generated this session"))
            .children(rows)
            .when(warn, |c| {
                c.child(hint(
                    "Non-commercial items: a free plan does not allow monetised use. The export dialog lists them again.",
                ))
            })
            .into_any_element()
    }
}

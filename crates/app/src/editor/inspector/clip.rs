//! The tabs of a selected clip: Video, Audio, Speed, Animation, Adjust.

use chukcut_engine::modules::render::layout::{crop_extent, crop_uv, fit_size};
use gpui::component::scroll::ScrollableElement;
use gpui::component::slider::Slider;
use gpui::component::switch::Switch;
use gpui::component::Disableable;
use gpui::AnyElement;

use super::controls::*;
use super::*;

const VIDEO: &str = "Video";
const AUDIO: &str = "Audio";
const SPEED: &str = "Speed";
const ANIMATION: &str = "Animation";
const ADJUST: &str = "Adjust";
const BASIC: &str = "Basic";
const VOICE: &str = "Voice changer";
const TRACKING: &str = "Tracking";

/// Which edge or centre an alignment button snaps the clip to.
#[derive(Clone, Copy)]
enum Align {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
}

impl Editor {
    /// The segment a property row edits. Sound lives on the linked audio clip
    /// when the picture was imported with it, so the Audio tab of a video
    /// clip edits that one.
    pub(crate) fn target_id(&self, prop: Prop) -> Option<String> {
        let id = self.selected.clone()?;
        if !matches!(prop, Prop::Volume | Prop::FadeIn | Prop::FadeOut) {
            return Some(id);
        }
        Some(self.sound_segment(&id).unwrap_or(id))
    }

    /// The clip that plays `segment_id`'s sound, when that is another clip.
    fn sound_segment(&self, segment_id: &str) -> Option<String> {
        let (track, segment) = self.project.segment(segment_id)?;
        if !self.project.sound_is_on_a_linked_lane(track, segment) {
            return None;
        }
        let group = self.project.link_group_of(segment_id)?;
        self.project
            .link_members(group)
            .into_iter()
            .find(|(track, _, s)| track.kind == TrackKind::Audio && s.id != segment_id)
            .map(|(_, _, s)| s.id.clone())
    }

    pub(crate) fn target_segment(&self, prop: Prop) -> Option<&Segment> {
        let id = self.target_id(prop)?;
        self.project.segment(&id).map(|(_, s)| s)
    }

    fn has_sound(&self, segment: &Segment) -> bool {
        self.project
            .materials
            .videos
            .iter()
            .any(|m| m.id == segment.material_id && m.has_audio)
    }

    pub(super) fn render_inspector_clip(
        &mut self,
        kind: ClipKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(segment) = self.selected_segment().map(|(_, s)| s.clone()) else {
            return div().into_any_element();
        };
        let mut tabs: Vec<&'static str> = match kind {
            ClipKind::Video if self.has_sound(&segment) => {
                vec![VIDEO, AUDIO, SPEED, ANIMATION, ADJUST]
            }
            ClipKind::Video => vec![VIDEO, SPEED, ANIMATION, ADJUST],
            ClipKind::Image => vec![VIDEO, ANIMATION, ADJUST, TRACKING],
            ClipKind::Audio => vec![BASIC, VOICE, SPEED],
            ClipKind::Text => vec![VIDEO, ANIMATION, TRACKING],
        };
        // Effects: on every picture, and all an effect clip has.
        if self.project.materials.is_effect_clip(&segment) {
            tabs = vec![effects::EFFECTS];
        } else if kind != ClipKind::Audio {
            tabs.push(effects::EFFECTS);
        }
        let active = self
            .inspector
            .tab
            .filter(|tab| tabs.contains(tab))
            .unwrap_or(tabs[0]);

        let sub = |this: &Self, owner: &'static str, first: &'static str| {
            this.inspector.sub_tab.get(owner).copied().unwrap_or(first)
        };
        let (sub_tabs_row, body, footer): (Option<AnyElement>, AnyElement, Option<AnyElement>) =
            match active {
                // A title has a position, scale and opacity and nothing to
                // cut out or retouch: no sub-tabs, only Basic.
                VIDEO if kind == ClipKind::Text => {
                    (None, self.video_basic(&segment, kind, window, cx), None)
                }
                VIDEO => {
                    let names = ["Basic", "Remove background", "Mask", "Retouch"];
                    let current = sub(self, VIDEO, names[0]);
                    let body = if current == "Basic" {
                        self.video_basic(&segment, kind, window, cx)
                    } else {
                        not_yet(current)
                    };
                    (
                        Some(sub_tabs(VIDEO, &names, current, cx).into_any_element()),
                        body,
                        None,
                    )
                }
                effects::EFFECTS => (None, self.effects_tab(&segment, window, cx), None),
                AUDIO | BASIC => (None, self.audio_basic(window, cx), None),
                VOICE => (None, not_yet("Voice changer"), None),
                TRACKING => (None, self.tracking_tab(&segment, window, cx), None),
                SPEED => {
                    let names = ["Standard", "Curve", "Speed effects"];
                    let current = sub(self, SPEED, names[0]);
                    let (body, footer) = if current == "Standard" {
                        (
                            self.speed_standard(&segment, window, cx),
                            Some(self.speed_footer(&segment, cx)),
                        )
                    } else {
                        (not_yet(current), None)
                    };
                    (
                        Some(sub_tabs(SPEED, &names, current, cx).into_any_element()),
                        body,
                        footer,
                    )
                }
                ANIMATION => self.animation_tab(&segment, kind, window, cx),
                _ => {
                    let names = ["Basic", "HSL", "Curves", "Colour wheels", "Mask"];
                    let current = sub(self, ADJUST, names[0]);
                    let body = match current {
                        "Basic" => self.adjust_basic(&segment, window, cx),
                        "HSL" => self.adjust_hsl(&segment, window, cx),
                        "Curves" => self.adjust_curves(&segment, cx),
                        "Colour wheels" => self.adjust_wheels(&segment, window, cx),
                        _ => not_yet(current),
                    };
                    (
                        Some(sub_tabs(ADJUST, &names, current, cx).into_any_element()),
                        body,
                        Some(self.adjust_footer(&segment, cx)),
                    )
                }
            };

        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(top_tabs(&tabs, active, cx))
            .children(sub_tabs_row)
            .child(
                div()
                    .id("inspector-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scrollbar()
                    .child(body),
            )
            .children(footer.map(panel_footer))
            .into_any_element()
    }

    pub(super) fn collapsed(&self, title: &'static str) -> bool {
        self.inspector.collapsed.contains(title)
    }

    // --- Video › Basic ------------------------------------------------------------------

    fn video_basic(
        &mut self,
        segment: &Segment,
        kind: ClipKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut transform_rows = Vec::new();
        if self.inspector.non_uniform_scale {
            transform_rows.push(self.slider_row(Prop::ScaleX, segment, window, cx));
            transform_rows.push(self.slider_row(Prop::ScaleY, segment, window, cx));
        } else {
            transform_rows.push(self.slider_row(Prop::Scale, segment, window, cx));
        }
        let uniform = !self.inspector.non_uniform_scale;
        let entity = cx.entity().downgrade();
        transform_rows.push(label_row(
            "Uniform scale",
            Switch::new("uniform-scale")
                .checked(uniform)
                .on_click(move |checked, _, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        this.inspector.non_uniform_scale = !*checked;
                        cx.notify();
                    });
                }),
        ));
        transform_rows.push(self.field_row(
            "Position",
            &[(Prop::PosX, Some("X")), (Prop::PosY, Some("Y"))],
            segment,
            window,
            cx,
        ));
        transform_rows.push(self.field_row(
            "Rotate",
            &[(Prop::Rotation, None)],
            segment,
            window,
            cx,
        ));
        if kind != ClipKind::Text {
            transform_rows.push(self.align_buttons(cx));
        }
        let transform = Section {
            on_reset: Some(Box::new(|this: &mut Editor, cx| this.reset_transform(cx))),
            ..Section::new("Transform")
        }
        .render(self.collapsed("Transform"), transform_rows, cx);

        let blend_rows = vec![
            label_row(
                "Mode",
                div()
                    .w(px(160.0))
                    .h(px(CONTROL_H))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .rounded(px(R_SM))
                    .bg(rgb(WELL))
                    .border_1()
                    .border_color(rgb(HAIRLINE))
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(DISABLED))
                    .child("Normal"),
            ),
            self.slider_row(Prop::Opacity, segment, window, cx),
        ];
        let blend = Section {
            on_reset: Some(Box::new(|this: &mut Editor, cx| {
                this.reset_prop(Prop::Opacity, cx)
            })),
            ..Section::new("Blend")
        }
        .render(self.collapsed("Blend"), blend_rows, cx);

        let mut sections = vec![transform, blend];
        // Footage tools; a title has no footage to stabilise or denoise.
        let footage_tools: &[&str] = if kind == ClipKind::Text {
            &[]
        } else {
            &[
                "Stabilise",
                "Enhance quality",
                "Reduce image noise",
                "Optical flow",
            ]
        };
        for &title in footage_tools {
            sections.push(Section::missing(title, "Not in the engine yet").render(
                true,
                Vec::new(),
                cx,
            ));
        }
        div()
            .flex()
            .flex_col()
            .children(sections)
            .into_any_element()
    }

    fn align_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        let buttons = [
            (Align::Left, icons::ALIGN_LEFT, "Align left"),
            (Align::HCenter, icons::ALIGN_HCENTER, "Centre horizontally"),
            (Align::Right, icons::ALIGN_RIGHT, "Align right"),
            (Align::Top, icons::ALIGN_TOP, "Align top"),
            (Align::VCenter, icons::ALIGN_VCENTER, "Centre vertically"),
            (Align::Bottom, icons::ALIGN_BOTTOM, "Align bottom"),
        ];
        div()
            .flex()
            .flex_row()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(2.0))
                    .p(px(2.0))
                    .rounded(px(R_SM + 1.0))
                    .bg(rgb(WELL))
                    .border_1()
                    .border_color(rgb(HAIRLINE))
                    .children(buttons.into_iter().enumerate().map(
                        |(i, (align, data, tooltip))| {
                            crate::ui::IconButton::new(("align", i), crate::ui::Glyph(data))
                                .small()
                                .tooltip(tooltip)
                                .on_click(cx.listener(move |this, _, _, cx| this.align(align, cx)))
                        },
                    )),
            )
            .into_any_element()
    }

    /// The clip's drawn size on the canvas, in canvas pixels, before rotation.
    fn drawn_size(&self, segment: &Segment) -> Option<(f32, f32)> {
        let pool = &self.project.materials;
        let source = if let Some(video) = pool.videos.iter().find(|m| m.id == segment.material_id) {
            if video.rotation.rem_euclid(180) == 90 {
                (video.height, video.width)
            } else {
                (video.width, video.height)
            }
        } else {
            let image = pool.images.iter().find(|m| m.id == segment.material_id)?;
            (image.width, image.height)
        };
        let (crop_w, crop_h) = crop_extent(crop_uv(segment.crop)?);
        let cropped = (
            (source.0.max(1) as f32 * crop_w).max(1.0) as u32,
            (source.1.max(1) as f32 * crop_h).max(1.0) as u32,
        );
        let canvas = (self.project.canvas.width, self.project.canvas.height);
        let (w, h) = fit_size(canvas, cropped);
        let scale_x = self.prop_value(Prop::ScaleX, segment) / 100.0;
        let scale_y = self.prop_value(Prop::ScaleY, segment) / 100.0;
        Some(((w * scale_x).abs(), (h * scale_y).abs()))
    }

    fn align(&mut self, align: Align, cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let Some((w, h)) = self.drawn_size(segment) else {
            return;
        };
        let (hw, hh) = self.half_canvas();
        // Position is the clip's centre, +y up.
        let (prop, value) = match align {
            Align::Left => (Prop::PosX, -hw + w * 0.5),
            Align::HCenter => (Prop::PosX, 0.0),
            Align::Right => (Prop::PosX, hw - w * 0.5),
            Align::Top => (Prop::PosY, hh - h * 0.5),
            Align::VCenter => (Prop::PosY, 0.0),
            Align::Bottom => (Prop::PosY, -hh + h * 0.5),
        };
        self.set_prop(prop, value.round(), Phase::Commit, cx);
    }

    /// Scale, position and rotation back to rest, as one step. Opacity and
    /// flips are not part of CapCut's Transform section and stay.
    fn reset_transform(&mut self, cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let before = segment.transform;
        let after = Transform {
            opacity: before.opacity,
            flip_h: before.flip_h,
            flip_v: before.flip_v,
            ..Transform::default()
        };
        if after.position == before.position
            && after.scale == before.scale
            && after.rotation == before.rotation
        {
            return;
        }
        let command = EditCommand::SetTransform {
            segment_id: segment.id.clone(),
            before,
            after,
        };
        self.apply(Ok(command), cx);
    }

    // --- Audio --------------------------------------------------------------------------------

    fn audio_basic(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(segment) = self.target_segment(Prop::Volume).cloned() else {
            return not_yet("Audio");
        };
        let rows = vec![
            self.slider_row(Prop::Volume, &segment, window, cx),
            self.slider_row(Prop::FadeIn, &segment, window, cx),
            self.slider_row(Prop::FadeOut, &segment, window, cx),
        ];
        let basic = Section {
            checkbox: Some(true),
            on_reset: Some(Box::new(|this: &mut Editor, cx| {
                this.reset_prop(Prop::Volume, cx)
            })),
            ..Section::new("Basic")
        }
        .render(self.collapsed("Basic"), rows, cx);
        let mut sections = vec![basic];
        sections.extend(self.voice_sections(&segment, cx));
        div()
            .flex()
            .flex_col()
            .children(sections)
            .into_any_element()
    }

    // --- Speed ----------------------------------------------------------------------------------

    fn speed_standard(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let speed = self.prop_value(Prop::Speed, segment);
        let number = self.number_box(Prop::Speed, speed, 64.0, None, window, cx);
        let (_, slider) = self.field(Prop::Speed, window, cx);
        let ticks =
            div()
                .relative()
                .h(px(14.0))
                .mx(px(8.0))
                .children(SPEED_KNOTS.iter().enumerate().map(|(i, knot)| {
                    div()
                        .absolute()
                        .top_0()
                        .left(gpui::relative(i as f32 / 5.0))
                        .ml(px(-12.0))
                        .w(px(24.0))
                        .flex()
                        .justify_center()
                        .text_size(px(TEXT_BADGE))
                        .font_family(FONT_MONO)
                        .text_color(rgb(TEXT_MUTED))
                        .child(format!("{knot}x"))
                }));
        let speed_row = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(TEXT_DIM))
                    .child("Speed"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .pt(px(8.0))
                            .px_1()
                            .children(slider.map(|slider| Slider::new(&slider)))
                            .child(ticks),
                    )
                    .child(number),
            );

        let duration = self.prop_value(Prop::Duration, segment);
        let original = segment.source_range.duration as f32 / 1_000_000.0;
        let duration_box = self.number_box(Prop::Duration, duration, 64.0, None, window, cx);
        let duration_row = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(TEXT_DIM))
                    .child("Duration"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(TEXT_LABEL))
                            .font_family(FONT_MONO)
                            .text_color(rgb(TEXT_DIM))
                            .child(format!("{original:.1}s")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .child(div().flex_1().h(px(1.0)).bg(rgb(BORDER_STRONG)))
                            .child(icon(icons::NEXT, 12.0, TEXT_MUTED).ml(px(-5.0))),
                    )
                    .child(duration_box),
            );

        let pitch = label_row(
            "Change audio pitch",
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(TEXT_CAPTION))
                        .text_color(rgb(TEXT_MUTED))
                        .child("Not in the engine yet"),
                )
                .child(Switch::new("speed-pitch").checked(false).disabled(true)),
        );

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .px(px(PAD))
            .py(px(PAD))
            .child(speed_row)
            .child(duration_row)
            .child(pitch)
            .into_any_element()
    }

    fn speed_footer(&self, segment: &Segment, cx: &mut Context<Self>) -> AnyElement {
        let changed = segment.speed != 1.0;
        panel_button(
            "speed-reset",
            "Reset",
            false,
            changed,
            cx.listener(|this, _, _, cx| this.reset_prop(Prop::Speed, cx)),
        )
        .into_any_element()
    }

    // --- Adjust ---------------------------------------------------------------------------------

    fn adjust_footer(&self, segment: &Segment, cx: &mut Context<Self>) -> AnyElement {
        let graded = self.project.materials.color_adjust_of(segment).is_some();
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(panel_button(
                "adjust-preset",
                "Save as preset",
                true,
                false,
                |_, _, _| {},
            ))
            .child(panel_button(
                "adjust-all",
                "Apply to all",
                false,
                graded,
                cx.listener(|this, _, _, cx| this.apply_grade_to_all(cx)),
            ))
            .into_any_element()
    }

    fn apply_grade_to_all(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let result = inspector_commands::inspector_apply_color_to_all(&self.state, id).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }
}

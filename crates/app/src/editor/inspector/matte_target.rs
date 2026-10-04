//! "Apply to: Whole clip / Subject / Background" in Adjust › Mask and at the
//! top of the Effects tab: the clip's grade or its effects limited by its
//! matte (`matting::matting_set_target` in the engine), so "grade only the
//! person" or "blur only the background" needs no copy of the clip.

use chukcut_engine::modules::matting::commands as matting;
use chukcut_engine::modules::project::compositing::MatteTarget;
use gpui::AnyElement;

use crate::ui::SegmentedTabs;

use super::controls::*;
use super::*;

const TARGETS: [MatteTarget; 3] = [
    MatteTarget::Whole,
    MatteTarget::Subject,
    MatteTarget::Background,
];

fn caption(text: impl Into<SharedString>, colour: u32) -> AnyElement {
    div()
        .text_size(px(TEXT_CAPTION))
        .text_color(rgb(colour))
        .child(text.into())
        .into_any_element()
}

impl Editor {
    /// The "Apply to" row for `part` of a video clip, with a line on where
    /// the subject comes from and how far its matte is baked; `None` for a
    /// clip without frames to matte.
    pub(super) fn apply_to_rows(
        &mut self,
        segment: &Segment,
        part: matting::MattePart,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.project.materials.video(&segment.material_id)?;
        let setting = self
            .project
            .materials
            .compositing_of(segment)
            .and_then(|m| m.background.clone());
        let current = setting
            .as_ref()
            .map(|s| match part {
                matting::MattePart::Grade => s.grade.clone(),
                matting::MattePart::Effects => s.effects.clone(),
            })
            .unwrap_or_default();
        let id = segment.id.clone();
        let editor = cx.entity().downgrade();
        let tabs = SegmentedTabs::new(
            match part {
                matting::MattePart::Grade => "grade-apply-to",
                matting::MattePart::Effects => "effects-apply-to",
            },
            TARGETS.iter().map(|t| t.label().to_string()),
            TARGETS.iter().position(|t| *t == current).unwrap_or(0),
        )
        .on_select(move |index, _, cx| {
            let target = TARGETS[index].clone();
            let id = id.clone();
            let _ = editor.update(cx, |this, cx| this.set_matte_target(&id, part, target, cx));
        });

        let source = match &setting {
            Some(s) if s.prompt.is_some() => "the object selected in Remove background",
            Some(s) if s.model == matting::setting_for(matting::BackgroundMode::Objects).model => {
                "the main object (BiRefNet lite)"
            }
            _ => "people (Robust Video Matting)",
        };
        let explain = match current {
            MatteTarget::Subject | MatteTarget::Background => format!(
                "The subject is {source}; change it in Video › Remove background. Frames whose \
                 matte is not baked yet show the {} on the whole clip.",
                match part {
                    matting::MattePart::Grade => "grade",
                    matting::MattePart::Effects => "effects",
                }
            ),
            _ => format!(
                "Subject or Background limits the {} by a matte of {source}, made on this \
                 machine; the clip is not cut.",
                match part {
                    matting::MattePart::Grade => "grade",
                    matting::MattePart::Effects => "effects",
                }
            ),
        };
        let mut rows = vec![label_row("Apply to", tabs), caption(explain, TEXT_MUTED)];
        if let Some((_, _, progress)) = matting::matting_running()
            .into_iter()
            .find(|(_, s, _)| *s == segment.id)
        {
            rows.push(caption(
                format!(
                    "Making the matte\u{2026} {} of {} frames",
                    progress.done, progress.total
                ),
                TEXT_DIM,
            ));
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(PAD))
                .py(px(8.0))
                .children(rows)
                .into_any_element(),
        )
    }

    fn set_matte_target(
        &mut self,
        segment_id: &str,
        part: matting::MattePart,
        target: MatteTarget,
        cx: &mut Context<Self>,
    ) {
        let result = matting::matting_set_target(&self.state, segment_id.to_string(), part, target);
        let job = result.as_ref().ok().and_then(|r| r.job);
        self.after_command(result, cx);
        if let Some(job) = job {
            // Followed like an automatic re-bake: the preview redraws as the
            // matte's frames land.
            self.follow_matte_bake(job);
        }
    }

    /// Adjust › Mask: where the grade applies.
    pub(super) fn adjust_mask(&mut self, segment: &Segment, cx: &mut Context<Self>) -> AnyElement {
        self.apply_to_rows(segment, matting::MattePart::Grade, cx)
            .unwrap_or_else(|| not_yet("Mask"))
    }
}

//! The text animator: by letter, word or line, with stagger.
//!
//! Timing and poses are here and pure; the pixels are
//! [`crate::modules::text::animate`], which moves glyph sprites and knows
//! nothing about presets.
//!
//! ## Stagger
//!
//! A window of `W` microseconds holds every unit. With `R + 1` distinct start
//! ranks and an overlap `o` (0 = strictly one after another, 1 = all
//! together), each unit animates for `L = W / (1 + R(1 - o))` and rank `r`
//! starts at `r·L(1 - o)`; the last unit ends exactly at `W`. The order picks
//! the ranks: reading order, reversed, from the middle out (the two units
//! equally far from the middle share a rank), or a seeded shuffle.

use std::sync::Arc;

use crate::modules::project::animation::{
    AnimationMaterial, StaggerOrder, TextAnimator, TextPreset,
};
use crate::modules::project::document::{MaterialPool, Micros, Segment, TextMaterial};
use crate::modules::render::{RenderContext, SourceFrame};
use crate::modules::text::animate::{self, GlyphPose};
use crate::modules::text::{RasterOptions, TextRenderer, TextRequest};

use super::pose::hash;

/// The start rank of each of `n` units.
pub fn ranks(n: usize, order: StaggerOrder, seed: u32) -> Vec<f32> {
    match order {
        StaggerOrder::Forward => (0..n).map(|i| i as f32).collect(),
        StaggerOrder::Backward => (0..n).map(|i| (n - 1 - i) as f32).collect(),
        StaggerOrder::Centre => {
            let mid = (n as f32 - 1.0) / 2.0;
            // Rounded down to whole steps from the middle, so an even count's
            // two middle units start together.
            (0..n).map(|i| ((i as f32 - mid).abs()).floor()).collect()
        }
        StaggerOrder::Random => {
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by_key(|&i| hash(((seed as u64) << 32) ^ i as u64));
            let mut ranks = vec![0.0; n];
            for (rank, unit) in order.into_iter().enumerate() {
                ranks[unit] = rank as f32;
            }
            ranks
        }
    }
}

/// Raw (uneased) progress `0..1` of a unit at `rank`, `t` into a window
/// `window` long, of which the highest rank is `max_rank`.
pub fn unit_progress(t: Micros, window: Micros, rank: f32, max_rank: f32, overlap: f32) -> f32 {
    if window <= 0 {
        return 1.0;
    }
    let gap = 1.0 - overlap.clamp(0.0, 1.0);
    let w = window as f32;
    let length = w / (1.0 + max_rank * gap);
    let start = rank * length * gap;
    ((t as f32 - start) / length).clamp(0.0, 1.0)
}

/// A unit's pose for `preset` at eased progress `p` (1 = at rest). `em` is
/// the font size in image pixels: distances are in lines, so a title moves
/// the same on a preview and in an export.
pub fn unit_pose(preset: TextPreset, p: f32, raw: f32, strength: f32, em: f32) -> GlyphPose {
    let d = 1.0 - p;
    let s = strength;
    let mut pose = GlyphPose::REST;
    let fade = p.clamp(0.0, 1.0);
    match preset {
        TextPreset::Typewriter => pose.opacity = if raw > 0.0 { 1.0 } else { 0.0 },
        TextPreset::Fade => pose.opacity = fade,
        TextPreset::FadeUp => {
            pose.dy = 0.5 * em * s * d;
            pose.opacity = fade;
        }
        TextPreset::Pop => {
            pose.scale = (1.0 - s * d).max(0.0);
            pose.opacity = (p * 3.0).clamp(0.0, 1.0);
        }
        TextPreset::SlideUp => {
            pose.dy = 1.0 * em * s * d;
            pose.opacity = fade;
        }
        TextPreset::Drop => {
            pose.dy = -1.5 * em * s * d;
            pose.opacity = (p * 4.0).clamp(0.0, 1.0);
        }
        TextPreset::Zoom => {
            pose.scale = 1.0 + 1.5 * s * d;
            pose.opacity = fade;
        }
        TextPreset::Spin => {
            pose.rotation = -180.0 * s * d;
            pose.scale = (1.0 - d).max(0.0);
            pose.opacity = fade;
        }
    }
    pose
}

/// The In and Out windows of a text clip `duration` long, shrunk in
/// proportion when they do not fit — the rule `pose::windows` follows.
pub fn text_windows(material: &AnimationMaterial, duration: Micros) -> (Micros, Micros) {
    let wanted_in = material.text_in.map_or(0, |a| a.duration.max(0));
    let wanted_out = material.text_out.map_or(0, |a| a.duration.max(0));
    let total = wanted_in + wanted_out;
    let duration = duration.max(0);
    if total <= duration || total == 0 {
        return (wanted_in, wanted_out);
    }
    let fit_in = (wanted_in as f64 * duration as f64 / total as f64).floor() as Micros;
    (fit_in, duration - fit_in)
}

/// Whether a text animator is moving anything at `rel`.
pub fn is_active(material: &AnimationMaterial, rel: Micros, duration: Micros) -> bool {
    let (win_in, win_out) = text_windows(material, duration);
    (material.text_in.is_some() && rel < win_in)
        || (material.text_out.is_some() && rel >= duration - win_out)
}

/// One animator's poses for every glyph, given each glyph's unit.
fn poses_for(
    animator: &TextAnimator,
    units: &[Option<usize>],
    count: usize,
    t: Micros,
    window: Micros,
    leaving: bool,
    em: f32,
) -> Vec<GlyphPose> {
    let ranks = ranks(count, animator.order, animator.seed);
    let max_rank = ranks.iter().copied().fold(0.0, f32::max);
    let unit_poses: Vec<GlyphPose> = ranks
        .iter()
        .map(|&rank| {
            let raw = unit_progress(t, window, rank, max_rank, animator.overlap);
            // Leaving is arriving backwards: the first unit to go is the
            // first in the order, and its progress runs down to 0.
            let raw = if leaving { 1.0 - raw } else { raw };
            let p = animator.easing.apply(raw);
            unit_pose(animator.preset, p, raw, animator.strength, em)
        })
        .collect();
    units
        .iter()
        .map(|u| u.map_or(GlyphPose::REST, |u| unit_poses[u]))
        .collect()
}

/// The poses of every glyph of `text` at `rel` into a clip `duration` long,
/// and how opaque the background box is. `None` when nothing is moving.
pub fn glyph_poses(
    material: &AnimationMaterial,
    text: &str,
    layout: &crate::modules::text::TextLayout,
    rel: Micros,
    duration: Micros,
    em: f32,
) -> Option<(Vec<GlyphPose>, f32, Vec<Option<usize>>)> {
    if !is_active(material, rel, duration) {
        return None;
    }
    let (win_in, win_out) = text_windows(material, duration);
    let mut poses = vec![GlyphPose::REST; layout.glyphs.len()];
    let mut backdrop = 1.0f32;
    let mut grouping = Vec::new();
    let mut layer = |animator: &TextAnimator, t: Micros, window: Micros, leaving: bool| {
        let (units, count) = animate::glyph_units(text, layout, animator.unit);
        let more = poses_for(animator, &units, count, t, window, leaving, em);
        for (pose, more) in poses.iter_mut().zip(more) {
            *pose = pose.then(more);
        }
        let progress = (t as f32 / window.max(1) as f32).clamp(0.0, 1.0);
        backdrop *= if leaving { 1.0 - progress } else { progress };
        // The In and Out windows never overlap (`text_windows`), so at most
        // one animator is running and its grouping is the one to pose about.
        grouping = units;
    };
    if let Some(a) = &material.text_in {
        if rel < win_in {
            layer(a, rel.max(0), win_in, false);
        }
    }
    if let Some(a) = &material.text_out {
        let start = duration - win_out;
        if rel >= start {
            layer(a, rel - start, win_out, true);
        }
    }
    Some((poses, backdrop, grouping))
}

/// The animated picture of a text clip at `time`, or `None` when its text
/// animator is at rest there (the ordinary cached title is then correct).
///
/// Called by the compositor instead of the source provider while a text
/// animator runs. It reads the text from `materials` — the document the
/// frame is rendered from — and goes through the shared `TextRenderer`, so
/// the two rasters it needs (the glyphs, and the background box alone) are
/// cached like any title; only the sprite composite is per frame.
pub fn animated_text_frame(
    ctx: &RenderContext,
    materials: &MaterialPool,
    segment: &Segment,
    time: Micros,
    size: (u32, u32),
    canvas: (u32, u32),
) -> Option<SourceFrame> {
    let text = materials.text(&segment.material_id)?;
    let animation = materials.animation_of(segment)?;
    let rel = time - segment.target_range.start;
    let duration = segment.target_range.duration;
    if !is_active(animation, rel, duration) {
        return None;
    }
    let size = (size.0.max(1), size.1.max(1));
    let scale = size.0 as f32 / canvas.0.max(1) as f32;
    let (pixels, width, height) = render(text, animation, rel, duration, size, scale)?;
    Some(upload(ctx, &pixels, width, height))
}

/// The pixels of [`animated_text_frame`], without a GPU. Public for tests and
/// for anything that wants a frame of animated text on the CPU.
pub fn render(
    text: &TextMaterial,
    animation: &AnimationMaterial,
    rel: Micros,
    duration: Micros,
    size: (u32, u32),
    scale: f32,
) -> Option<(Vec<u8>, u32, u32)> {
    let renderer = TextRenderer::shared();
    let options = RasterOptions::canvas(size.0, size.1).with_scale(scale);

    let mut glyph_request = TextRequest::from(text);
    glyph_request.background = None;
    let pad = glyph_request.bleed() * scale;
    let glyphs: Arc<_> = renderer.rasterize(&glyph_request, &options);

    let backdrop = text.background.map(|_| {
        let mut request = TextRequest::from(text);
        request.color = [0.0; 4];
        request.stroke_width = 0.0;
        request.shadow = None;
        renderer.rasterize(&request, &options)
    });

    let em = text.font_size * scale;
    let (poses, backdrop_opacity, units) =
        glyph_poses(animation, &text.content, &glyphs.layout, rel, duration, em)?;
    let pixels = animate::compose(
        &glyphs,
        backdrop.as_deref(),
        backdrop_opacity,
        &poses,
        pad,
        Some(&units),
    );
    Some((pixels, glyphs.width, glyphs.height))
}

/// Upload straight-alpha RGBA8 as an sRGB texture — what `media` does for a
/// still title, repeated here because that helper is private to `media`.
fn upload(ctx: &RenderContext, data: &[u8], width: u32, height: u32) -> SourceFrame {
    let extent = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("animated text"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        extent,
    );
    SourceFrame::from_texture(Arc::new(texture))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::animation::{Ease, TextUnit};

    fn animator(preset: TextPreset, overlap: f32) -> TextAnimator {
        TextAnimator {
            preset,
            unit: TextUnit::Letter,
            order: StaggerOrder::Forward,
            seed: 0,
            duration: 1_000_000,
            overlap,
            easing: Ease::Linear,
            strength: 1.0,
        }
    }

    #[test]
    fn the_stagger_fills_the_window_exactly() {
        // Four units, no overlap: four quarters.
        let p = |t, r| unit_progress(t, 1_000_000, r, 3.0, 0.0);
        assert_eq!(p(0, 0.0), 0.0);
        assert_eq!(p(250_000, 0.0), 1.0);
        assert_eq!(p(250_000, 1.0), 0.0);
        assert_eq!(p(375_000, 1.0), 0.5);
        assert_eq!(p(1_000_000, 3.0), 1.0);
        // Full overlap: all together.
        assert_eq!(unit_progress(500_000, 1_000_000, 3.0, 3.0, 1.0), 0.5);
    }

    #[test]
    fn orders_rank_as_named() {
        assert_eq!(ranks(4, StaggerOrder::Forward, 0), vec![0.0, 1.0, 2.0, 3.0]);
        assert_eq!(
            ranks(4, StaggerOrder::Backward, 0),
            vec![3.0, 2.0, 1.0, 0.0]
        );
        assert_eq!(
            ranks(5, StaggerOrder::Centre, 0),
            vec![2.0, 1.0, 0.0, 1.0, 2.0]
        );
        assert_eq!(ranks(4, StaggerOrder::Centre, 0), vec![1.0, 0.0, 0.0, 1.0]);
        let a = ranks(10, StaggerOrder::Random, 7);
        assert_eq!(a, ranks(10, StaggerOrder::Random, 7), "seeded");
        assert_ne!(a, ranks(10, StaggerOrder::Random, 8));
        let mut sorted = a.clone();
        sorted.sort_by(f32::total_cmp);
        assert_eq!(sorted, (0..10).map(|i| i as f32).collect::<Vec<_>>());
    }

    #[test]
    fn a_typewriter_shows_letters_in_turn_and_then_all() {
        let a = animator(TextPreset::Typewriter, 0.0);
        let units: Vec<Option<usize>> = vec![Some(0), None, Some(1), Some(2)];
        let at = |t| poses_for(&a, &units, 3, t, 900_000, false, 10.0);
        let opacities = |p: Vec<GlyphPose>| p.iter().map(|g| g.opacity).collect::<Vec<_>>();
        assert_eq!(opacities(at(100_000)), vec![1.0, 1.0, 0.0, 0.0]);
        assert_eq!(opacities(at(400_000)), vec![1.0, 1.0, 1.0, 0.0]);
        assert_eq!(opacities(at(900_000)), vec![1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn a_text_in_and_out_bracket_the_clip_and_rest_between() {
        let mut m = AnimationMaterial::new();
        m.text_in = Some(animator(TextPreset::FadeUp, 0.5));
        m.text_out = Some(animator(TextPreset::Fade, 0.5));
        assert!(is_active(&m, 0, 5_000_000));
        assert!(!is_active(&m, 2_000_000, 5_000_000));
        assert!(is_active(&m, 4_500_000, 5_000_000));
        // Trimmed to one second, both still fit by shrinking.
        assert_eq!(text_windows(&m, 1_000_000), (500_000, 500_000));
    }

    #[test]
    fn every_text_preset_rests_at_the_end_and_moves_at_the_start() {
        for d in crate::modules::motion::catalog::text_presets() {
            let rest = unit_pose(d.preset, 1.0, 1.0, 1.0, 20.0);
            assert_eq!(rest, GlyphPose::REST, "{:?}", d.preset);
            let start = unit_pose(d.preset, 0.0, 0.0, 1.0, 20.0);
            assert_ne!(start, GlyphPose::REST, "{:?}", d.preset);
        }
    }
}

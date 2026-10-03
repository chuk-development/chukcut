//! Preview tiles for the asset panel, rendered by the compositor.
//!
//! A tile shows the real effect, transition or layout applied to a sample
//! picture — drawn here, procedurally, so there is no asset to ship and no
//! licence to track — through the same `Compositor` the preview and the
//! export use. Each tile is rendered once and kept as a PNG under the cache
//! root; its file name carries a hash of the shader source and parameters it
//! was drawn with, so changing an effect redraws its tile and nothing else.
//!
//! Blocking and GPU-bound: the app calls these from a background thread.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::catalog::{self, descriptor};
use super::edit::{fill_cell, SplitLayout};
use crate::modules::project::document::{
    CanvasConfig, ImageMaterial, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    TransitionMaterial,
};
use crate::modules::project::effects::{EffectMaterial, EffectValue};
use crate::modules::render::{
    Compositor, CompositorConfig, RenderContext, SourceFrame, SourceProvider, SourceRequest,
};
use crate::modules::transitions::library;

/// The compositor every tile is drawn with, built on first use.
fn compositor() -> Result<&'static Compositor, String> {
    static COMPOSITOR: OnceLock<Option<Compositor>> = OnceLock::new();
    COMPOSITOR
        .get_or_init(|| {
            crate::modules::gpu::render_context().map(|ctx| {
                Compositor::with_config(
                    ctx,
                    CompositorConfig {
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        // Small: tiles are tiny and few at a time.
                        texture_budget_bytes: 32 * 1024 * 1024,
                        strict_sources: false,
                    },
                )
            })
        })
        .as_ref()
        .ok_or_else(|| "there is no GPU to draw previews with".to_string())
}

/// FNV-1a, 64 bits: the cache key of a tile.
fn hash(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.bytes().chain([0xff]) {
            h ^= byte as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn tiles_dir() -> PathBuf {
    crate::modules::workspace::paths::cache_root().join("fx-tiles")
}

/// The tile at `path`, drawn by `draw` if it is not on disk yet.
fn cached(
    path: PathBuf,
    draw: impl FnOnce() -> Result<image::RgbaImage, String>,
) -> Result<PathBuf, String> {
    if path.exists() {
        return Ok(path);
    }
    let image = draw()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    // Written beside and renamed, so a reader never sees half a PNG.
    let partial = path.with_extension("part.png");
    image
        .save(&partial)
        .map_err(|e| format!("cannot write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, &path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// The sample pictures
// ---------------------------------------------------------------------------

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// A small landscape: sky, a bright sun (something for glow and halation to
/// catch), two ranges of hills and a lake with a reflection. `night` gives
/// the second, cooler picture transitions go to.
pub fn sample_picture(w: u32, h: u32, night: bool) -> Vec<u8> {
    let (top, horizon, sun, near, far, water) = if night {
        (
            [0.05, 0.04, 0.20],
            [0.55, 0.25, 0.60],
            [0.95, 0.97, 1.0],
            [0.06, 0.05, 0.14],
            [0.20, 0.12, 0.35],
            [0.15, 0.20, 0.45],
        )
    } else {
        (
            [0.20, 0.45, 0.85],
            [1.0, 0.68, 0.35],
            [1.0, 0.98, 0.85],
            [0.10, 0.22, 0.18],
            [0.30, 0.38, 0.45],
            [0.25, 0.50, 0.70],
        )
    };
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    let (fw, fh) = (w as f32, h as f32);
    let (sx, sy, sr) = (
        if night { 0.25 } else { 0.7 } * fw,
        0.33 * fh,
        0.09 * fh.min(fw),
    );
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / fw, y as f32 / fh);
            let far_hill = 0.58 - 0.08 * (u * 7.0 + 1.0).sin() * (u * 2.3).cos();
            let near_hill = 0.70 - 0.10 * (u * 4.0 + 2.0).sin();
            let lake = 0.80;
            let mut c = if v < far_hill {
                let mut sky = mix(top, horizon, (v / far_hill).powf(1.6));
                let d = ((x as f32 - sx).powi(2) + (y as f32 - sy).powi(2)).sqrt();
                if d < sr {
                    sky = sun;
                } else {
                    sky = mix(
                        sky,
                        sun,
                        (1.0 - (d - sr) / (sr * 2.5)).max(0.0).powi(2) * 0.5,
                    );
                }
                sky
            } else if v < near_hill {
                mix(far, near, (v - far_hill) * 4.0)
            } else if v < lake {
                near
            } else {
                // The lake reflects the sky, darker and rippled.
                let ripple = ((y as f32 * 0.9).sin() * 0.5 + 0.5) * 0.15;
                mix(water, horizon, 0.3 + ripple)
            };
            // A dark lighthouse so the edges have something to show.
            if (0.12..0.16).contains(&u) && v > 0.42 && v < 0.72 {
                c = [0.08, 0.06, 0.06];
            }
            out.extend_from_slice(&[
                (c[0] * 255.0) as u8,
                (c[1] * 255.0) as u8,
                (c[2] * 255.0) as u8,
                255,
            ]);
        }
    }
    out
}

/// Serves the sample pictures: material `sample-day` and `sample-night`, at
/// the size asked for.
struct Samples;

impl SourceProvider for Samples {
    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        let night = request.material_id.ends_with("night");
        let (w, h) = (SAMPLE_W, SAMPLE_H);
        let bytes = sample_picture(w, h, night);
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("chukcut tile sample"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
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
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        Ok(Some(SourceFrame::from_texture(std::sync::Arc::new(
            texture,
        ))))
    }
}

/// The sample pictures' own size: 16:10, like the tiles.
const SAMPLE_W: u32 = 448;
const SAMPLE_H: u32 = 280;

fn sample_project(size: (u32, u32)) -> Project {
    let mut p = Project::new(
        "tile",
        CanvasConfig {
            width: size.0,
            height: size.1,
            background: [0.07, 0.07, 0.08, 1.0],
        },
        30.0,
    );
    for id in ["sample-day", "sample-night"] {
        p.materials.images.push(ImageMaterial {
            id: id.into(),
            path: String::new(),
            width: SAMPLE_W,
            height: SAMPLE_H,
        });
    }
    p
}

fn clip(id: &str, material: &str, start: Micros, duration: Micros) -> Segment {
    Segment {
        id: id.into(),
        material_id: material.into(),
        target_range: TimeRange::new(start, duration),
        source_range: TimeRange::new(0, duration),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

fn draw(project: &Project, time: Micros) -> Result<image::RgbaImage, String> {
    let c = compositor()?;
    let size = (project.canvas.width, project.canvas.height);
    let frame = c
        .render(project, time, size, &Samples)
        .map_err(|e| e.to_string())?;
    image::RgbaImage::from_raw(frame.width, frame.height, frame.data)
        .ok_or_else(|| "the frame did not fit its own size".into())
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

/// Parameters a tile shows an effect with when its defaults would read as
/// nothing at tile size.
fn tile_params(kind: &str) -> &'static [(&'static str, f32)] {
    match kind {
        catalog::FRAME => &[
            ("radius", 30.0),
            ("border", 40.0),
            ("shadow", 80.0),
            ("shadow_distance", 30.0),
        ],
        catalog::GATE_WEAVE => &[("amount", 100.0)],
        catalog::GRAIN => &[("amount", 100.0), ("size", 3.0)],
        catalog::GLITCH => &[("intensity", 80.0)],
        _ => &[],
    }
}

/// The instant a tile is drawn at, chosen to show the effect mid-move.
fn tile_time(kind: &str) -> Micros {
    match kind {
        catalog::LIGHT_SWEEP => 650_000,
        catalog::SHAKE => 370_000,
        _ => 400_000,
    }
}

/// The tile of effect `kind`.
pub fn effect_tile(kind: &str, size: (u32, u32)) -> Result<PathBuf, String> {
    effect_tile_in(&tiles_dir(), kind, size)
}

fn effect_tile_in(dir: &Path, kind: &str, size: (u32, u32)) -> Result<PathBuf, String> {
    let desc = descriptor(kind).ok_or_else(|| format!("there is no effect called {kind}"))?;
    let overrides = format!("{:?}", tile_params(kind));
    let key = hash(&[
        include_str!("shaders/fx.wgsl"),
        include_str!("render.rs"),
        desc.id,
        &format!("{:?}", desc.params),
        &overrides,
    ]);
    let path = dir.join(format!("{key:016x}-{kind}-{}x{}.png", size.0, size.1));
    cached(path, || {
        let mut p = sample_project(size);
        let mut track = Track::new(TrackKind::Video, "Main");
        let mut segment = clip("s", "sample-day", 0, 10_000_000);
        if kind == catalog::FRAME {
            segment.transform.scale = [0.62, 0.62];
        }
        let mut effect = EffectMaterial::new(kind);
        effect.seed = 1234;
        for (name, value) in tile_params(kind) {
            effect
                .params
                .insert((*name).to_string(), EffectValue::Number(*value));
        }
        segment.extras.push(effect.id.clone());
        p.materials.effects.push(effect);
        track.segments.push(segment);
        p.tracks.push(track);
        draw(&p, tile_time(kind))
    })
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

/// The tile of a transition, `preset` a library id or `None` for a built-in
/// `kind`, caught 40% of the way through.
pub fn transition_tile(
    kind: crate::modules::project::document::TransitionKind,
    preset: Option<&str>,
    size: (u32, u32),
) -> Result<PathBuf, String> {
    transition_tile_in(&tiles_dir(), kind, preset, size)
}

fn transition_tile_in(
    dir: &Path,
    kind: crate::modules::project::document::TransitionKind,
    preset: Option<&str>,
    size: (u32, u32),
) -> Result<PathBuf, String> {
    let source = match preset {
        Some(id) => {
            let (_, p) =
                library::preset(id).ok_or_else(|| format!("there is no transition called {id}"))?;
            p.wgsl
        }
        None => include_str!("../transitions/shaders/transition.wgsl"),
    };
    let name = preset.map_or_else(|| format!("{kind:?}"), |p| p.replace(':', "-"));
    let key = hash(&[source, &name, "40%"]);
    let path = dir.join(format!("{key:016x}-tr-{name}-{}x{}.png", size.0, size.1));
    cached(path, || {
        let mut p = sample_project(size);
        let mut track = Track::new(TrackKind::Video, "Main");
        track.segments.push(clip("a", "sample-day", 0, 2_000_000));
        let mut b = clip("b", "sample-night", 2_000_000, 2_000_000);
        let mut transition = match preset {
            Some(id) => TransitionMaterial::library(id, 1_000_000),
            None => TransitionMaterial::new(kind, 1_000_000),
        };
        // Linear, so 40% of the window is 40% of the way.
        transition.easing = crate::modules::project::document::Easing::Linear;
        b.extras.push(transition.id.clone());
        p.materials.transitions.push(transition);
        track.segments.push(b);
        p.tracks.push(track);
        draw(&p, 1_900_000)
    })
}

// ---------------------------------------------------------------------------
// Layouts
// ---------------------------------------------------------------------------

/// The tile of a split-screen layout, the two sample pictures alternating
/// in its cells.
pub fn split_tile(layout: SplitLayout, size: (u32, u32)) -> Result<PathBuf, String> {
    split_tile_in(&tiles_dir(), layout, size)
}

fn split_tile_in(dir: &Path, layout: SplitLayout, size: (u32, u32)) -> Result<PathBuf, String> {
    let key = hash(&[include_str!("edit.rs"), &format!("{layout:?}")]);
    let path = dir.join(format!(
        "{key:016x}-split-{layout:?}-{}x{}.png",
        size.0, size.1
    ));
    cached(path, || {
        let mut p = sample_project(size);
        for (i, cell) in layout.cells().into_iter().enumerate() {
            let mut track = Track::new(TrackKind::Video, format!("Cell {i}"));
            let material = if i % 2 == 0 {
                "sample-day"
            } else {
                "sample-night"
            };
            let mut segment = clip(&format!("c{i}"), material, 0, 1_000_000);
            let (transform, crop) = fill_cell(size, Some((SAMPLE_W as f32, SAMPLE_H as f32)), cell);
            segment.transform = transform;
            segment.crop = crop;
            segment.render_index = i as i32;
            track.segments.push(segment);
            p.tracks.push(track);
        }
        draw(&p, 0)
    })
}

/// The tile of the picture-in-picture preset.
pub fn pip_tile(size: (u32, u32)) -> Result<PathBuf, String> {
    pip_tile_in(&tiles_dir(), size)
}

fn pip_tile_in(dir: &Path, size: (u32, u32)) -> Result<PathBuf, String> {
    let key = hash(&[
        include_str!("edit.rs"),
        include_str!("shaders/fx.wgsl"),
        "pip",
    ]);
    let path = dir.join(format!("{key:016x}-pip-{}x{}.png", size.0, size.1));
    cached(path, || {
        let mut p = sample_project(size);
        let mut back = Track::new(TrackKind::Video, "Back");
        back.segments.push(clip("bg", "sample-night", 0, 1_000_000));
        p.tracks.push(back);
        let mut front = Track::new(TrackKind::Video, "Front");
        front.segments.push(clip("fg", "sample-day", 0, 1_000_000));
        front.segments[0].render_index = 1;
        p.tracks.push(front);
        let (frame, command) = super::edit::pip_command(&p, "fg", super::edit::Corner::TopRight)?;
        p.materials.effects.push(frame);
        command.apply(&mut p)?;
        draw(&p, 0)
    })
}

/// Remove every cached tile. They are redrawn on demand.
pub fn clear() -> std::io::Result<()> {
    let dir = tiles_dir();
    if Path::new(&dir).exists() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_pictures_differ_and_have_a_bright_sun() {
        let day = sample_picture(64, 40, false);
        let night = sample_picture(64, 40, true);
        assert_eq!(day.len(), 64 * 40 * 4);
        assert_ne!(day, night);
        assert!(day.chunks(4).any(|p| p[0] > 240 && p[1] > 240));
    }

    #[test]
    fn every_effect_transition_and_layout_draws_a_tile() {
        // Into a directory of the test's own, so it neither reads stale tiles
        // nor leaves files in the user's cache.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/fx-tile-test")
            .join(format!("{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        if compositor().is_err() {
            eprintln!("skipping: no GPU adapter");
            return;
        }
        let size = (112, 70);
        for effect in catalog::catalog() {
            let path = effect_tile_in(&dir, effect.id, size).unwrap();
            let image = image::open(&path).unwrap().to_rgba8();
            assert_eq!(image.dimensions(), size, "{}", effect.id);
        }
        // Two effects draw two different tiles.
        let pixelate = image::open(effect_tile_in(&dir, catalog::PIXELATE, size).unwrap()).unwrap();
        let mirror = image::open(effect_tile_in(&dir, catalog::MIRROR, size).unwrap()).unwrap();
        assert_ne!(pixelate.to_rgba8().into_raw(), mirror.to_rgba8().into_raw());
        for descriptor in crate::modules::transitions::catalog() {
            let path = transition_tile_in(&dir, descriptor.kind, descriptor.preset, size).unwrap();
            assert!(path.exists());
        }
        for layout in SplitLayout::ALL {
            assert!(split_tile_in(&dir, layout, size).unwrap().exists());
        }
        assert!(pip_tile_in(&dir, size).unwrap().exists());
        // A second call is a cache hit, not a redraw.
        let modified = || {
            std::fs::metadata(effect_tile_in(&dir, catalog::GLOW, size).unwrap())
                .unwrap()
                .modified()
                .unwrap()
        };
        assert_eq!(modified(), modified());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

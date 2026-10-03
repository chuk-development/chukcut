//! Every library preset on the GPU: it compiles, it starts on the outgoing
//! clip and ends on the incoming one.

use std::sync::Arc;

use super::*;
use crate::modules::project::document::{TransitionDirection, TransitionMaterial};
use crate::modules::render::test_context;
use crate::modules::transitions::{TransitionParams, TransitionPipeline};

const W: u32 = 64;
const H: u32 = 36;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn layer(
    ctx: &RenderContext,
    f: impl Fn(u32, u32) -> [u8; 4],
) -> (Arc<wgpu::Texture>, wgpu::TextureView) {
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("library test layer"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut bytes = Vec::new();
    for y in 0..H {
        for x in 0..W {
            bytes.extend_from_slice(&f(x, y));
        }
    }
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
            bytes_per_row: Some(W * 4),
            rows_per_image: Some(H),
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (Arc::new(texture), view)
}

fn read(ctx: &RenderContext, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
    const PADDED: u32 = 256;
    let buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("library test readback"),
        size: (PADDED * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(PADDED),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue().submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    let view = slice.get_mapped_range().unwrap();
    let mut out = Vec::new();
    for row in 0..H as usize {
        for column in 0..W as usize {
            let at = row * PADDED as usize + column * 4;
            out.push([view[at], view[at + 1], view[at + 2], view[at + 3]]);
        }
    }
    drop(view);
    buffer.unmap();
    out
}

fn from_pixel(x: u32, y: u32) -> [u8; 4] {
    [200, (x * 255 / W) as u8, (y * 255 / H) as u8, 255]
}

fn to_pixel(x: u32, y: u32) -> [u8; 4] {
    [(y * 255 / H) as u8, 30, (x * 255 / W) as u8, 255]
}

/// Mean absolute channel difference against the expected layer.
fn distance(got: &[[u8; 4]], want: impl Fn(u32, u32) -> [u8; 4]) -> f32 {
    let mut sum = 0.0;
    for y in 0..H {
        for x in 0..W {
            let g = got[(y * W + x) as usize];
            let w = want(x, y);
            for c in 0..3 {
                sum += (g[c] as f32 - w[c] as f32).abs();
            }
        }
    }
    sum / (W * H * 3) as f32
}

/// gl-transitions that, by design, do not start exactly on the outgoing clip
/// or end exactly on the incoming one at the very ends of the window. Each
/// was looked at; the reason is the transition's, not the port's.
const LOOSE_ENDS: &[&str] = &[
    // Steps through a hand-timed table of 24 frames, and the last frame of
    // the table still carries a glitch of 0.28: the incoming clip arrives
    // with its last flicker on it.
    "gl:drop_zone_flicker",
];

#[test]
fn every_preset_compiles_and_runs_from_the_outgoing_clip_to_the_incoming_one() {
    let Some(ctx) = test_context() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let pipeline = TransitionPipeline::new(&ctx, FORMAT);
    let (_a, from) = layer(&ctx, from_pixel);
    let (_b, to) = layer(&ctx, to_pixel);
    let target = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("library test target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut failures = Vec::new();
    for preset in presets() {
        let mut material = TransitionMaterial::library(preset.id.clone(), 1_000_000);
        material.direction = TransitionDirection::Right;
        let mut ends = [0.0f32; 2];
        for (i, progress) in [0.0f32, 0.9999].into_iter().enumerate() {
            let params = TransitionParams::new(&material, progress);
            assert!(params.library.is_some(), "{} did not resolve", preset.id);
            pipeline.blend_to_texture(&ctx, &params, &from, &to, &view);
            let got = read(&ctx, &target);
            ends[i] = if i == 0 {
                distance(&got, from_pixel)
            } else {
                distance(&got, to_pixel)
            };
        }
        if (ends[0] > 3.0 || ends[1] > 3.0) && !LOOSE_ENDS.contains(&preset.id.as_str()) {
            failures.push(format!(
                "{} start {:.1} end {:.1}",
                preset.id, ends[0], ends[1]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_push_brings_the_incoming_clip_in_from_the_side_it_travels_from() {
    let Some(ctx) = test_context() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let pipeline = TransitionPipeline::new(&ctx, FORMAT);
    let (_a, from) = layer(&ctx, from_pixel);
    let (_b, to) = layer(&ctx, to_pixel);
    let target = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("library test target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    for (direction, incoming_x, outgoing_x) in [
        (TransitionDirection::Right, W / 4, 3 * W / 4),
        (TransitionDirection::Left, 3 * W / 4, W / 4),
    ] {
        let mut material = TransitionMaterial::library("seamless:push", 1_000_000);
        material.direction = direction;
        pipeline.blend_to_texture(
            &ctx,
            &TransitionParams::new(&material, 0.5),
            &from,
            &to,
            &view,
        );
        let got = read(&ctx, &target);
        // The incoming clip has a flat green of 30; the outgoing one a red of
        // 200. Halfway through, each holds half the frame.
        let incoming = got[((H / 2) * W + incoming_x) as usize];
        let outgoing = got[((H / 2) * W + outgoing_x) as usize];
        assert!(
            (incoming[1] as i32 - 30).abs() <= 6 && incoming[0] < 170,
            "{direction:?}: expected the incoming clip at x {incoming_x}, got {incoming:?}"
        );
        assert!(
            outgoing[0] >= 190,
            "{direction:?}: expected the outgoing clip at x {outgoing_x}, got {outgoing:?}"
        );
    }
}

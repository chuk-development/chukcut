// RGBA8 -> NV12, written into two *images* rather than into one buffer.
//
// `nv12.wgsl` is the compute version and is still the right one whenever the
// destination is memory we chose the layout of. This one exists for the case
// where we did not choose it: the destination is a VA surface the media driver
// allocated, imported into Vulkan through `VK_EXT_image_drm_format_modifier`,
// and therefore **tiled in whatever way that driver likes**. Nothing here knows
// or needs to know which way — the tiling is the image's, and the hardware
// swizzles on write.
//
// That is the whole reason for a second shader. Intel's fixed-function JPEG
// engine reads an imported *linear* NV12 surface as though it were 32-row
// tiled, so the buffer route produces a mosaic; see
// `docs/research/preview-zerocopy-jpeg.md`. Letting the driver allocate removes
// the question rather than answering it.
//
// **Why a render pass and not a compute pass.** WebGPU has no storage-writable
// `R8Unorm` or `Rg8Unorm` — that is stated in `nv12.wgsl`'s own header and is
// why it writes a buffer — but both formats are perfectly good *colour
// attachments*. So the two planes are drawn rather than stored, one full-screen
// triangle each.
//
// **The input view is not sRGB**, for the same reason as in `nv12.wgsl`: `src`
// is a `Rgba8Unorm` view of a `Rgba8UnormSrgb` target, so `textureLoad` returns
// the stored byte over 255 rather than a linearised value, which is what makes
// this agree with the CPU converter the encoder has been fed for months.
//
// **The output is quantised by the hardware.** A fragment writing `v / 255.0`
// into an `R8Unorm` attachment lands on `round(v)`, which is the same rule
// `nv12.wgsl`'s `byte()` applies explicitly.

struct Params {
    // The *picture* size, in luma samples. The chroma pass derives its own
    // half-size extent from this rather than being told it, so the two can
    // never disagree about an odd edge.
    width: u32,
    height: u32,
    // `RANGE_LIMITED` for a video encoder, `RANGE_FULL` for the JPEG one.
    range: u32,
    // Always `MATRIX_BT601` today: the only caller is the preview's JPEG
    // encoder, and JFIF is BT.601. A parameter so the two shaders agree.
    matrix: u32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;

// Clamp rather than skip, exactly as in `nv12.wgsl`: the chroma pass of an odd
// edge reads one sample past it, and duplicating the edge sample is cheaper
// than a branch and is what a 4:2:0 encoder would do anyway.
fn texel(x: u32, y: u32) -> vec3<f32> {
    let cx = min(x, params.width - 1u);
    let cy = min(y, params.height - 1u);
    return textureLoad(src, vec2<i32>(i32(cx), i32(cy)), 0).rgb;
}

// One triangle covering the viewport. Three vertices and no vertex buffer; the
// clip-space positions are (-1,-1), (3,-1), (-1,3), whose triangle contains the
// whole of the [-1,1] square.
@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(index) / 2) * 4.0 - 1.0;
    let y = f32(i32(index) & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// The luma plane: one fragment per picture sample.
@fragment
fn luma(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let x = u32(at.x);
    let y = u32(at.y);
    return vec4<f32>(luma_in(texel(x, y), params.matrix, params.range) / 255.0, 0.0, 0.0, 1.0);
}

// The chroma plane: one fragment per 2x2 block, so the attachment is half the
// size in both axes and each fragment reads four samples.
//
// The average is taken over RGB and converted afterwards — the order swscale's
// `_half` converters use. Converting first and averaging the chroma is visibly
// different on saturated edges.
@fragment
fn chroma(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let x = u32(at.x) * 2u;
    let y = u32(at.y) * 2u;
    let mean = (texel(x, y) + texel(x + 1u, y) + texel(x, y + 1u) + texel(x + 1u, y + 1u)) * 0.25;
    let c = chroma_in(mean, params.matrix, params.range);
    return vec4<f32>(c.x / 255.0, c.y / 255.0, 0.0, 1.0);
}

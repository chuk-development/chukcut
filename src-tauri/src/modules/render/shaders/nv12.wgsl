// RGBA8 -> NV12, on the GPU.
//
// Replaces the swscale pass that used to sit between the compositor's readback
// and the hardware encoder's upload. Two things about it are load-bearing.
//
// **The input view is not sRGB.** `src` is bound as a `Rgba8Unorm` view of a
// `Rgba8UnormSrgb` render target, so `textureLoad` returns the stored byte
// divided by 255 rather than a linearised value. swscale is handed those same
// stored bytes, so this is what makes the two agree. Binding the sRGB view here
// would silently produce a washed-out picture that still encodes and still
// plays.
//
// **The output is a storage buffer, not two storage textures.** R8Unorm and
// Rg8Unorm are not storage-capable formats in core WebGPU, and a buffer lets us
// choose the plane strides so the result can be memcpy'd straight into an
// `AVFrame` — with no 256-byte row alignment to strip afterwards.
//
// Each invocation owns a 4x2 pixel block, which is exactly one aligned `u32` of
// luma in each of two rows and one aligned `u32` of interleaved chroma. That is
// the smallest block for which every write is a whole word, so no atomics and
// no read-modify-write anywhere.

struct Params {
    width: u32,
    height: u32,
    // All three in words, not bytes: the destination is `array<u32>`.
    y_stride_words: u32,
    uv_offset_words: u32,
    uv_stride_words: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

// Clamp rather than skip: the last 4-pixel block of an odd-width frame reads
// past the edge, and duplicating the edge pixel into padding nobody decodes is
// cheaper than a branch per texel.
fn texel(x: u32, y: u32) -> vec3<f32> {
    let cx = min(x, params.width - 1u);
    let cy = min(y, params.height - 1u);
    return textureLoad(src, vec2<i32>(i32(cx), i32(cy)), 0).rgb;
}

// BT.601, limited range — what swscale produces by default for an untagged RGB
// source, and what `encoder.rs` tags the stream as with `color_range: MPEG`.
// The coefficients are the ones in the standard; the 219/224 scales are the
// limited-range excursions.
fn luma(c: vec3<f32>) -> f32 {
    return 16.0 + 219.0 * (0.299 * c.r + 0.587 * c.g + 0.114 * c.b);
}

fn chroma(c: vec3<f32>) -> vec2<f32> {
    let cb = -0.168736 * c.r - 0.331264 * c.g + 0.5 * c.b;
    let cr = 0.5 * c.r - 0.418688 * c.g - 0.081312 * c.b;
    return vec2<f32>(128.0 + 224.0 * cb, 128.0 + 224.0 * cr);
}

fn byte(v: f32) -> u32 {
    return u32(clamp(round(v), 0.0, 255.0));
}

fn pack(a: f32, b: f32, c: f32, d: f32) -> u32 {
    return byte(a) | (byte(b) << 8u) | (byte(c) << 16u) | (byte(d) << 24u);
}

@compute @workgroup_size(8, 8, 1)
fn convert(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x0 = gid.x * 4u;
    let y0 = gid.y * 2u;
    if (x0 >= params.width || y0 >= params.height) {
        return;
    }

    let top = array<vec3<f32>, 4>(
        texel(x0, y0),
        texel(x0 + 1u, y0),
        texel(x0 + 2u, y0),
        texel(x0 + 3u, y0),
    );
    let bottom = array<vec3<f32>, 4>(
        texel(x0, y0 + 1u),
        texel(x0 + 1u, y0 + 1u),
        texel(x0 + 2u, y0 + 1u),
        texel(x0 + 3u, y0 + 1u),
    );

    dst[y0 * params.y_stride_words + gid.x] =
        pack(luma(top[0]), luma(top[1]), luma(top[2]), luma(top[3]));

    // An odd-height frame has no second row to write for its last block. The
    // chroma below still averages the clamped duplicate, which is what a 4:2:0
    // encoder would do with an odd height anyway.
    if (y0 + 1u < params.height) {
        dst[(y0 + 1u) * params.y_stride_words + gid.x] =
            pack(luma(bottom[0]), luma(bottom[1]), luma(bottom[2]), luma(bottom[3]));
    }

    // Chroma is the 2x2 box average of the *linear-in-code-value* RGB, then
    // converted — the same order swscale's `_half` converters use. Converting
    // first and averaging the chroma afterwards gives a visibly different
    // result on saturated edges.
    let left = (top[0] + top[1] + bottom[0] + bottom[1]) * 0.25;
    let right = (top[2] + top[3] + bottom[2] + bottom[3]) * 0.25;
    let cl = chroma(left);
    let cr = chroma(right);

    dst[params.uv_offset_words + gid.y * params.uv_stride_words + gid.x] =
        pack(cl.x, cl.y, cr.x, cr.y);
}

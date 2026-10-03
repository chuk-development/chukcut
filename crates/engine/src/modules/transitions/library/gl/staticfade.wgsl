// StaticFade, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Ben Lucas
// License: MIT
//
// Translated from GLSL to WGSL by naga 30 through the harness in
// `../port.py`; the licence texts are in `../LICENSE-gl-transitions.md`.
// Edit `port.py`, not this file.
struct GlBlock {
    state: vec4<f32>,
    params: array<vec4<f32>, 12>,
}

struct FragmentOutput {
    @location(0) o_color: vec4<f32>,
}

var<private> v_uv_1: vec2<f32>;
var<private> o_color: vec4<f32>;
@group(0) @binding(0) 
var<uniform> U: GlBlock;
@group(1) @binding(0) 
var u_from: texture_2d<f32>;
@group(1) @binding(1) 
var u_to: texture_2d<f32>;
@group(1) @binding(2) 
var u_sampler: sampler;
var<private> progress: f32;
var<private> ratio: f32;
var<private> n_noise_pixels: f32;
var<private> static_luminosity: f32;

fn chukcut_premultiply(c: vec4<f32>) -> vec4<f32> {
    var c_1: vec4<f32>;

    c_1 = c;
    let _e12 = c_1;
    let _e14 = c_1;
    let _e16 = (_e12.xyz * _e14.w);
    let _e17 = c_1;
    return vec4<f32>(_e16.x, _e16.y, _e16.z, _e17.w);
}

fn getFromColor(uv: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;

    uv_1 = uv;
    let _e12 = uv_1;
    let _e15 = uv_1;
    let _e19 = textureSample(u_from, u_sampler, vec2<f32>(_e12.x, (1f - _e15.y)));
    let _e20 = chukcut_premultiply(_e19);
    return _e20;
}

fn getToColor(uv_2: vec2<f32>) -> vec4<f32> {
    var uv_3: vec2<f32>;

    uv_3 = uv_2;
    let _e12 = uv_3;
    let _e15 = uv_3;
    let _e19 = textureSample(u_to, u_sampler, vec2<f32>(_e12.x, (1f - _e15.y)));
    let _e20 = chukcut_premultiply(_e19);
    return _e20;
}

fn rnd(st: vec2<f32>) -> f32 {
    var st_1: vec2<f32>;

    st_1 = st;
    let _e16 = st_1;
    return fract((sin(dot(_e16.xy, vec2<f32>(10.530234f, 70.23493f))) * 12345.545f));
}

fn staticNoise(st_2: vec2<f32>, offset: f32, luminosity: f32) -> vec4<f32> {
    var st_3: vec2<f32>;
    var offset_1: f32;
    var luminosity_1: f32;
    var staticR: f32;
    var staticG: f32;
    var staticB: f32;

    st_3 = st_2;
    offset_1 = offset;
    luminosity_1 = luminosity;
    let _e20 = luminosity_1;
    let _e21 = st_3;
    let _e22 = offset_1;
    let _e25 = offset_1;
    let _e30 = rnd((_e21 * vec2<f32>((_e22 * 2f), (_e25 * 3f))));
    staticR = (_e20 * _e30);
    let _e33 = luminosity_1;
    let _e34 = st_3;
    let _e35 = offset_1;
    let _e38 = offset_1;
    let _e43 = rnd((_e34 * vec2<f32>((_e35 * 3f), (_e38 * 5f))));
    staticG = (_e33 * _e43);
    let _e46 = luminosity_1;
    let _e47 = st_3;
    let _e48 = offset_1;
    let _e51 = offset_1;
    let _e56 = rnd((_e47 * vec2<f32>((_e48 * 5f), (_e51 * 7f))));
    staticB = (_e46 * _e56);
    let _e59 = staticR;
    let _e60 = staticG;
    let _e61 = staticB;
    return vec4<f32>(_e59, _e60, _e61, 1f);
}

fn staticIntensity(t: f32) -> f32 {
    var t_1: f32;
    var transitionProgress: f32;
    var transformedThreshold: f32;

    t_1 = t;
    let _e17 = t_1;
    transitionProgress = abs((2f * (_e17 - 0.5f)));
    let _e25 = transitionProgress;
    transformedThreshold = ((1.2f * (1f - _e25)) - 0.1f);
    let _e32 = transformedThreshold;
    return min(1f, _e32);
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var baseMix: f32;
    var transitionMix: vec4<f32>;
    var uvStatic: vec2<f32>;
    var staticColor: vec4<f32>;
    var staticThresh: f32;
    var staticMix: f32;

    uv_5 = uv_4;
    let _e17 = progress;
    baseMix = step(0.5f, _e17);
    let _e20 = uv_5;
    let _e21 = getFromColor(_e20);
    let _e22 = uv_5;
    let _e23 = getToColor(_e22);
    let _e24 = baseMix;
    transitionMix = mix(_e21, _e23, vec4(_e24));
    let _e28 = uv_5;
    let _e29 = n_noise_pixels;
    let _e32 = n_noise_pixels;
    uvStatic = (floor((_e28 * _e29)) / vec2(_e32));
    let _e36 = uvStatic;
    let _e37 = progress;
    let _e38 = static_luminosity;
    let _e39 = staticNoise(_e36, _e37, _e38);
    staticColor = _e39;
    let _e41 = progress;
    let _e42 = staticIntensity(_e41);
    staticThresh = _e42;
    let _e44 = uvStatic;
    let _e45 = rnd(_e44);
    let _e46 = staticThresh;
    staticMix = step(_e45, _e46);
    let _e49 = transitionMix;
    let _e50 = staticColor;
    let _e51 = staticMix;
    return mix(_e49, _e50, vec4(_e51));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    n_noise_pixels = _e33.x;
    let _e38 = U.params[1];
    static_luminosity = _e38.x;
    chukcut_init_globals();
    let _e40 = v_uv_1;
    let _e43 = v_uv_1;
    let _e47 = transition(vec2<f32>(_e40.x, (1f - _e43.y)));
    c_2 = _e47;
    let _e49 = c_2;
    a = clamp(_e49.w, 0f, 1f);
    let _e55 = a;
    if (_e55 > 0.00001f) {
        let _e58 = c_2;
        let _e60 = a;
        let _e62 = (_e58.xyz / vec3(_e60));
        let _e63 = a;
        local = vec4<f32>(_e62.x, _e62.y, _e62.z, _e63);
    } else {
        local = vec4(0f);
    }
    let _e71 = local;
    o_color = _e71;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}

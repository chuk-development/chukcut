// DirectionalScaled, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Thibaut Foussard
// License: MIT
// based on Directional transition by Gaëtan Renaudeau
// https://gl-transitions.com/editor/Directional
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
var<private> direction: vec2<f32>;
var<private> scale: f32;

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

fn parabola(x: f32) -> f32 {
    var x_1: f32;
    var y: f32;

    x_1 = x;
    let _e16 = x_1;
    y = pow(sin((_e16 * 3.1415927f)), 1f);
    let _e25 = y;
    return _e25;
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var easedProgress: f32;
    var p: vec2<f32>;
    var f: vec2<f32>;
    var s: f32;
    var mixer: f32;
    var col: vec4<f32>;
    var border: f32;

    uv_5 = uv_4;
    let _e16 = progress;
    easedProgress = pow(sin(((_e16 * 3.1415927f) / 2f)), 3f);
    let _e27 = uv_5;
    let _e28 = easedProgress;
    let _e29 = direction;
    p = (_e27 + (_e28 * sign(_e29)));
    let _e34 = p;
    f = fract(_e34);
    let _e40 = scale;
    let _e43 = progress;
    let _e44 = parabola(_e43);
    s = (1f - ((1f - (1f / _e40)) * _e44));
    let _e48 = f;
    let _e52 = s;
    f = (((_e48 - vec2(0.5f)) * _e52) + vec2(0.5f));
    let _e58 = p;
    let _e61 = p;
    let _e67 = p;
    let _e71 = p;
    mixer = (((step(0f, _e58.y) * step(_e61.y, 1f)) * step(0f, _e67.x)) * step(_e71.x, 1f));
    let _e77 = f;
    let _e78 = getToColor(_e77);
    let _e79 = f;
    let _e80 = getFromColor(_e79);
    let _e81 = mixer;
    col = mix(_e78, _e80, vec4(_e81));
    let _e86 = f;
    let _e91 = f;
    let _e97 = f;
    let _e103 = f;
    border = (((step(0f, _e86.x) * step(0f, (1f - _e91.x))) * step(0f, _e97.y)) * step(0f, (1f - _e103.y)));
    let _e109 = col;
    let _e110 = border;
    col = (_e109 * _e110);
    let _e112 = col;
    return _e112;
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
    direction = _e33.xy;
    let _e38 = U.params[1];
    scale = _e38.x;
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

// RectangleCrop, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: martiniti
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
var<private> bgcolor: vec4<f32>;

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

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var s: f32;
    var q: vec2<f32>;
    var bl: vec2<f32>;
    var tr: vec2<f32>;
    var dist: f32;
    var local: vec4<f32>;

    uv_5 = uv_4;
    let _e16 = progress;
    s = pow((2f * abs((_e16 - 0.5f))), 3f);
    let _e24 = uv_5;
    q = (_e24.xy / vec2(1f));
    let _e33 = progress;
    let _e40 = q;
    bl = step(vec2((1f - (2f * abs((_e33 - 0.5f))))), (_e40 + vec2(0.25f)));
    let _e48 = progress;
    let _e56 = q;
    tr = step(vec2((1f - (2f * abs((_e48 - 0.5f))))), (vec2(1.25f) - _e56));
    let _e62 = bl;
    let _e64 = bl;
    let _e67 = tr;
    let _e70 = tr;
    dist = length((1f - (((_e62.x * _e64.y) * _e67.x) * _e70.y)));
    let _e76 = progress;
    if (_e76 < 0.5f) {
        let _e79 = uv_5;
        let _e80 = getFromColor(_e79);
        local = _e80;
    } else {
        let _e81 = uv_5;
        let _e82 = getToColor(_e81);
        local = _e82;
    }
    let _e84 = local;
    let _e85 = bgcolor;
    let _e86 = s;
    let _e87 = dist;
    return mix(_e84, _e85, vec4(step(_e86, _e87)));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_1: vec4<f32>;

    let _e13 = U;
    progress = _e13.state.x;
    let _e17 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e17);
    let _e20 = dims;
    let _e23 = dims;
    ratio = (f32(_e20.x) / f32(max(_e23.y, 1i)));
    let _e32 = U.params[0];
    bgcolor = _e32;
    chukcut_init_globals();
    let _e33 = v_uv_1;
    let _e36 = v_uv_1;
    let _e40 = transition(vec2<f32>(_e33.x, (1f - _e36.y)));
    c_2 = _e40;
    let _e42 = c_2;
    a = clamp(_e42.w, 0f, 1f);
    let _e48 = a;
    if (_e48 > 0.00001f) {
        let _e51 = c_2;
        let _e53 = a;
        let _e55 = (_e51.xyz / vec3(_e53));
        let _e56 = a;
        local_1 = vec4<f32>(_e55.x, _e55.y, _e55.z, _e56);
    } else {
        local_1 = vec4(0f);
    }
    let _e64 = local_1;
    o_color = _e64;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e24 = o_color;
    return FragmentOutput(_e24);
}

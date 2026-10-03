// SimpleZoomOut, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Tianshuo
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
var<private> zoom_quickness: f32;
var<private> fade: bool;
var<private> nQuick: f32;

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

fn zoom(uv_4: vec2<f32>, amount: f32) -> vec2<f32> {
    var uv_5: vec2<f32>;
    var amount_1: f32;

    uv_5 = uv_4;
    amount_1 = amount;
    let _e20 = uv_5;
    let _e25 = amount_1;
    return (vec2(0.5f) + ((_e20 - vec2(0.5f)) * (1f - _e25)));
}

fn transition(uv_6: vec2<f32>) -> vec4<f32> {
    var uv_7: vec2<f32>;
    var local: f32;
    var local_1: f32;

    uv_7 = uv_6;
    let _e17 = uv_7;
    let _e18 = getFromColor(_e17);
    let _e19 = uv_7;
    let _e22 = nQuick;
    let _e25 = progress;
    let _e28 = zoom(_e19, (1f - smoothstep((1f - _e22), 1f, _e25)));
    let _e29 = getToColor(_e28);
    let _e30 = fade;
    if _e30 {
        let _e32 = nQuick;
        let _e35 = progress;
        local_1 = smoothstep((1f - _e32), 1f, _e35);
    } else {
        let _e37 = progress;
        let _e39 = nQuick;
        if (_e37 < (1f - _e39)) {
            local = 0f;
        } else {
            local = 1f;
        }
        let _e45 = local;
        local_1 = _e45;
    }
    let _e47 = local_1;
    return mix(_e18, _e29, vec4(_e47));
}

fn chukcut_init_globals() {
    let _e15 = zoom_quickness;
    nQuick = clamp(_e15, 0.2f, 1f);
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_2: vec4<f32>;

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    let _e34 = U.params[0];
    zoom_quickness = _e34.x;
    let _e39 = U.params[1];
    fade = (_e39.x > 0.5f);
    chukcut_init_globals();
    let _e43 = v_uv_1;
    let _e46 = v_uv_1;
    let _e50 = transition(vec2<f32>(_e43.x, (1f - _e46.y)));
    c_2 = _e50;
    let _e52 = c_2;
    a = clamp(_e52.w, 0f, 1f);
    let _e58 = a;
    if (_e58 > 0.00001f) {
        let _e61 = c_2;
        let _e63 = a;
        let _e65 = (_e61.xyz / vec3(_e63));
        let _e66 = a;
        local_2 = vec4<f32>(_e65.x, _e65.y, _e65.z, _e66);
    } else {
        local_2 = vec4(0f);
    }
    let _e74 = local_2;
    o_color = _e74;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}

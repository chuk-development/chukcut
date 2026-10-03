// splitSlideOutHorizontal, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: OllyOllyOlly
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

const boundMin: vec2<f32> = vec2<f32>(0f, 0f);
const boundMax: vec2<f32> = vec2<f32>(1f, 1f);

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
var<private> reverse: bool;

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

fn inBounds(p: vec2<f32>) -> bool {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e17 = p_1;
    let _e20 = p_1;
    return (all((boundMin < _e17)) && all((_e20 < boundMax)));
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var local: f32;
    var local_1: f32;
    var modifier: f32;
    var p_2: vec2<f32>;
    var local_2: vec4<f32>;

    uv_5 = uv_4;
    let _e17 = uv_5;
    if (_e17.y > 0.5f) {
        local = 1f;
    } else {
        local = -1f;
    }
    let _e25 = local;
    let _e26 = reverse;
    if _e26 {
        local_1 = -1f;
    } else {
        local_1 = 1f;
    }
    let _e31 = local_1;
    modifier = (_e25 * _e31);
    let _e34 = uv_5;
    let _e36 = progress;
    let _e37 = modifier;
    let _e40 = uv_5;
    p_2 = vec2<f32>((_e34.x + (_e36 * _e37)), _e40.y);
    let _e44 = p_2;
    let _e45 = inBounds(_e44);
    if _e45 {
        let _e46 = p_2;
        let _e47 = getFromColor(_e46);
        local_2 = _e47;
    } else {
        let _e48 = uv_5;
        let _e49 = getToColor(_e48);
        local_2 = _e49;
    }
    let _e51 = local_2;
    return _e51;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_3: vec4<f32>;

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    let _e34 = U.params[0];
    reverse = (_e34.x > 0.5f);
    chukcut_init_globals();
    let _e38 = v_uv_1;
    let _e41 = v_uv_1;
    let _e45 = transition(vec2<f32>(_e38.x, (1f - _e41.y)));
    c_2 = _e45;
    let _e47 = c_2;
    a = clamp(_e47.w, 0f, 1f);
    let _e53 = a;
    if (_e53 > 0.00001f) {
        let _e56 = c_2;
        let _e58 = a;
        let _e60 = (_e56.xyz / vec3(_e58));
        let _e61 = a;
        local_3 = vec4<f32>(_e60.x, _e60.y, _e60.z, _e61);
    } else {
        local_3 = vec4(0f);
    }
    let _e69 = local_3;
    o_color = _e69;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}

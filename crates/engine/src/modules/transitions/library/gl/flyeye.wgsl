// flyeye, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: gre
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
var<private> size: f32;
var<private> zoom: f32;
var<private> colorSeparation: f32;

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

fn transition(p: vec2<f32>) -> vec4<f32> {
    var p_1: vec2<f32>;
    var inv: f32;
    var disp: vec2<f32>;
    var texTo: vec4<f32>;
    var texFrom: vec4<f32>;

    p_1 = p;
    let _e18 = progress;
    inv = (1f - _e18);
    let _e21 = size;
    let _e22 = zoom;
    let _e23 = p_1;
    let _e27 = zoom;
    let _e28 = p_1;
    disp = (_e21 * vec2<f32>(cos((_e22 * _e23.x)), sin((_e27 * _e28.y))));
    let _e35 = p_1;
    let _e36 = inv;
    let _e37 = disp;
    let _e40 = getToColor((_e35 + (_e36 * _e37)));
    texTo = _e40;
    let _e42 = p_1;
    let _e43 = progress;
    let _e44 = disp;
    let _e47 = colorSeparation;
    let _e51 = getFromColor((_e42 + ((_e43 * _e44) * (1f - _e47))));
    let _e53 = p_1;
    let _e54 = progress;
    let _e55 = disp;
    let _e58 = getFromColor((_e53 + (_e54 * _e55)));
    let _e60 = p_1;
    let _e61 = progress;
    let _e62 = disp;
    let _e65 = colorSeparation;
    let _e69 = getFromColor((_e60 + ((_e61 * _e62) * (1f + _e65))));
    texFrom = vec4<f32>(_e51.x, _e58.y, _e69.z, 1f);
    let _e74 = texTo;
    let _e75 = progress;
    let _e77 = texFrom;
    let _e78 = inv;
    return ((_e74 * _e75) + (_e77 * _e78));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    let _e34 = U.params[0];
    size = _e34.x;
    let _e39 = U.params[1];
    zoom = _e39.x;
    let _e44 = U.params[2];
    colorSeparation = _e44.x;
    chukcut_init_globals();
    let _e46 = v_uv_1;
    let _e49 = v_uv_1;
    let _e53 = transition(vec2<f32>(_e46.x, (1f - _e49.y)));
    c_2 = _e53;
    let _e55 = c_2;
    a = clamp(_e55.w, 0f, 1f);
    let _e61 = a;
    if (_e61 > 0.00001f) {
        let _e64 = c_2;
        let _e66 = a;
        let _e68 = (_e64.xyz / vec3(_e66));
        let _e69 = a;
        local = vec4<f32>(_e68.x, _e68.y, _e68.z, _e69);
    } else {
        local = vec4(0f);
    }
    let _e77 = local;
    o_color = _e77;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}

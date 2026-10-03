// Bounce, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Adrian Purser
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

const PI: f32 = 3.1415927f;

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
var<private> shadow_colour: vec4<f32>;
var<private> shadow_height: f32;
var<private> bounces: f32;

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
    var time: f32;
    var stime: f32;
    var phase: f32;
    var y: f32;
    var d: f32;

    uv_5 = uv_4;
    let _e18 = progress;
    time = _e18;
    let _e20 = time;
    stime = sin(((_e20 * PI) / 2f));
    let _e26 = time;
    let _e28 = bounces;
    phase = ((_e26 * PI) * _e28);
    let _e31 = phase;
    let _e35 = stime;
    y = (abs(cos(_e31)) * (1f - _e35));
    let _e39 = uv_5;
    let _e41 = y;
    d = (_e39.y - _e41);
    let _e44 = uv_5;
    let _e45 = getToColor(_e44);
    let _e46 = shadow_colour;
    let _e47 = d;
    let _e48 = shadow_height;
    let _e51 = d;
    let _e52 = shadow_height;
    let _e54 = shadow_colour;
    let _e58 = shadow_colour;
    let _e65 = progress;
    let _e72 = uv_5;
    let _e74 = uv_5;
    let _e77 = y;
    let _e81 = getFromColor(vec2<f32>(_e72.x, (_e74.y + (1f - _e77))));
    let _e82 = d;
    return mix(mix(_e45, _e46, vec4((step(_e47, _e48) * (1f - mix((((_e51 / _e52) * _e54.w) + (1f - _e58.w)), 1f, smoothstep(0.95f, 1f, _e65)))))), _e81, vec4(step(_e82, 0f)));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e16 = U;
    progress = _e16.state.x;
    let _e20 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e20);
    let _e23 = dims;
    let _e26 = dims;
    ratio = (f32(_e23.x) / f32(max(_e26.y, 1i)));
    let _e35 = U.params[0];
    shadow_colour = _e35;
    let _e39 = U.params[1];
    shadow_height = _e39.x;
    let _e44 = U.params[2];
    bounces = _e44.x;
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
    let _e30 = o_color;
    return FragmentOutput(_e30);
}

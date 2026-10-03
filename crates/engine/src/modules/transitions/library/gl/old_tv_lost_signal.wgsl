// old_tv_lost_signal, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: mernking gitlab: Godswork
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

fn hash(p: vec2<f32>) -> f32 {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e14 = p_1;
    return fract((sin(dot(_e14, vec2<f32>(127.1f, 311.7f))) * 43758.547f));
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var p_2: f32;
    var strength: f32;
    var tv: vec2<f32>;
    var fromColor: vec4<f32>;
    var toColor: vec4<f32>;
    var color: vec4<f32>;
    var lineY: f32;
    var noise: f32;
    var line: f32;
    var drift: f32;
    var shiftedFrom: vec4<f32>;
    var shiftedTo: vec4<f32>;
    var lineColor: vec4<f32>;
    var scan: f32;

    uv_5 = uv_4;
    let _e14 = progress;
    p_2 = _e14;
    let _e16 = p_2;
    strength = sin((_e16 * 3.1415927f));
    let _e21 = uv_5;
    tv = _e21;
    let _e23 = tv;
    let _e24 = getFromColor(_e23);
    fromColor = _e24;
    let _e26 = tv;
    let _e27 = getToColor(_e26);
    toColor = _e27;
    let _e29 = fromColor;
    let _e30 = toColor;
    let _e31 = p_2;
    color = mix(_e29, _e30, vec4(_e31));
    let _e35 = tv;
    lineY = floor((_e35.y * 120f));
    let _e41 = lineY;
    let _e42 = p_2;
    let _e46 = hash(vec2<f32>(_e41, (_e42 * 20f)));
    noise = _e46;
    let _e49 = noise;
    line = step(0.92f, _e49);
    let _e52 = tv;
    let _e56 = p_2;
    let _e63 = strength;
    drift = ((sin(((_e52.y * 30f) + (_e56 * 10f))) * 0.02f) * _e63);
    let _e66 = tv;
    let _e67 = drift;
    let _e71 = getFromColor((_e66 + vec2<f32>(_e67, 0f)));
    shiftedFrom = _e71;
    let _e73 = tv;
    let _e74 = drift;
    let _e78 = getToColor((_e73 + vec2<f32>(_e74, 0f)));
    shiftedTo = _e78;
    let _e80 = shiftedFrom;
    let _e81 = shiftedTo;
    let _e82 = p_2;
    lineColor = mix(_e80, _e81, vec4(_e82));
    let _e86 = color;
    let _e87 = lineColor;
    let _e88 = line;
    let _e89 = strength;
    color = mix(_e86, _e87, vec4((_e88 * _e89)));
    let _e93 = tv;
    scan = (sin((_e93.y * 900f)) * 0.03f);
    let _e101 = color;
    let _e103 = color;
    let _e105 = scan;
    let _e106 = strength;
    let _e109 = (_e103.xyz - vec3((_e105 * _e106)));
    color.x = _e109.x;
    color.y = _e109.y;
    color.z = _e109.z;
    let _e116 = color;
    return _e116;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e12 = U;
    progress = _e12.state.x;
    let _e16 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e16);
    let _e19 = dims;
    let _e22 = dims;
    ratio = (f32(_e19.x) / f32(max(_e22.y, 1i)));
    chukcut_init_globals();
    let _e28 = v_uv_1;
    let _e31 = v_uv_1;
    let _e35 = transition(vec2<f32>(_e28.x, (1f - _e31.y)));
    c_2 = _e35;
    let _e37 = c_2;
    a = clamp(_e37.w, 0f, 1f);
    let _e43 = a;
    if (_e43 > 0.00001f) {
        let _e46 = c_2;
        let _e48 = a;
        let _e50 = (_e46.xyz / vec3(_e48));
        let _e51 = a;
        local = vec4<f32>(_e50.x, _e50.y, _e50.z, _e51);
    } else {
        local = vec4(0f);
    }
    let _e59 = local;
    o_color = _e59;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e22 = o_color;
    return FragmentOutput(_e22);
}

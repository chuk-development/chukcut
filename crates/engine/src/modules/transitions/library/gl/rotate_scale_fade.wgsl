// rotate_scale_fade, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Fernando Kuteken
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
var<private> center: vec2<f32>;
var<private> rotations: f32;
var<private> scale: f32;
var<private> backColor: vec4<f32>;

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
    var difference: vec2<f32>;
    var dir: vec2<f32>;
    var dist: f32;
    var angle: f32;
    var c_2: f32;
    var s: f32;
    var currentScale: f32;
    var rotatedDir: vec2<f32>;
    var rotatedUv: vec2<f32>;

    uv_5 = uv_4;
    let _e18 = uv_5;
    let _e19 = center;
    difference = (_e18 - _e19);
    let _e22 = difference;
    dir = normalize(_e22);
    let _e25 = difference;
    dist = length(_e25);
    let _e31 = rotations;
    let _e33 = progress;
    angle = ((6.2831855f * _e31) * _e33);
    let _e36 = angle;
    c_2 = cos(_e36);
    let _e39 = angle;
    s = sin(_e39);
    let _e42 = scale;
    let _e45 = progress;
    currentScale = mix(_e42, 1f, (2f * abs((_e45 - 0.5f))));
    let _e52 = dir;
    let _e54 = c_2;
    let _e56 = dir;
    let _e58 = s;
    let _e61 = dir;
    let _e63 = s;
    let _e65 = dir;
    let _e67 = c_2;
    rotatedDir = vec2<f32>(((_e52.x * _e54) - (_e56.y * _e58)), ((_e61.x * _e63) + (_e65.y * _e67)));
    let _e72 = center;
    let _e73 = rotatedDir;
    let _e74 = dist;
    let _e76 = currentScale;
    rotatedUv = (_e72 + ((_e73 * _e74) / vec2(_e76)));
    let _e81 = rotatedUv;
    let _e85 = rotatedUv;
    let _e90 = rotatedUv;
    let _e95 = rotatedUv;
    if ((((_e81.x < 0f) || (_e85.x > 1f)) || (_e90.y < 0f)) || (_e95.y > 1f)) {
        let _e100 = backColor;
        return _e100;
    }
    let _e101 = rotatedUv;
    let _e102 = getFromColor(_e101);
    let _e103 = rotatedUv;
    let _e104 = getToColor(_e103);
    let _e105 = progress;
    return mix(_e102, _e104, vec4(_e105));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
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
    center = _e35.xy;
    let _e40 = U.params[1];
    rotations = _e40.x;
    let _e45 = U.params[2];
    scale = _e45.x;
    let _e50 = U.params[3];
    backColor = _e50;
    chukcut_init_globals();
    let _e51 = v_uv_1;
    let _e54 = v_uv_1;
    let _e58 = transition(vec2<f32>(_e51.x, (1f - _e54.y)));
    c_3 = _e58;
    let _e60 = c_3;
    a = clamp(_e60.w, 0f, 1f);
    let _e66 = a;
    if (_e66 > 0.00001f) {
        let _e69 = c_3;
        let _e71 = a;
        let _e73 = (_e69.xyz / vec3(_e71));
        let _e74 = a;
        local = vec4<f32>(_e73.x, _e73.y, _e73.z, _e74);
    } else {
        local = vec4(0f);
    }
    let _e82 = local;
    o_color = _e82;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e30 = o_color;
    return FragmentOutput(_e30);
}

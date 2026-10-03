// hexagonalize, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Fernando Kuteken
// License: MIT
// Hexagonal math from: http://www.redblobgames.com/grids/hexagons/
//
// Translated from GLSL to WGSL by naga 30 through the harness in
// `../port.py`; the licence texts are in `../LICENSE-gl-transitions.md`.
// Edit `port.py`, not this file.
struct GlBlock {
    state: vec4<f32>,
    params: array<vec4<f32>, 12>,
}

struct Hexagon {
    q: f32,
    r: f32,
    s: f32,
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
var<private> steps: i32;
var<private> horizontalHexagons: f32;

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

fn createHexagon(q: f32, r: f32) -> Hexagon {
    var q_1: f32;
    var r_1: f32;
    var hex: Hexagon;

    q_1 = q;
    r_1 = r;
    let _e20 = q_1;
    hex.q = _e20;
    let _e22 = r_1;
    hex.r = _e22;
    let _e24 = q_1;
    let _e26 = r_1;
    hex.s = (-(_e24) - _e26);
    let _e28 = hex;
    return _e28;
}

fn roundHexagon(hex_1: Hexagon) -> Hexagon {
    var hex_2: Hexagon;
    var q_2: f32;
    var r_2: f32;
    var s: f32;
    var deltaQ: f32;
    var deltaR: f32;
    var deltaS: f32;

    hex_2 = hex_1;
    let _e16 = hex_2;
    q_2 = floor((_e16.q + 0.5f));
    let _e22 = hex_2;
    r_2 = floor((_e22.r + 0.5f));
    let _e28 = hex_2;
    s = floor((_e28.s + 0.5f));
    let _e34 = q_2;
    let _e35 = hex_2;
    deltaQ = abs((_e34 - _e35.q));
    let _e40 = r_2;
    let _e41 = hex_2;
    deltaR = abs((_e40 - _e41.r));
    let _e46 = s;
    let _e47 = hex_2;
    deltaS = abs((_e46 - _e47.s));
    let _e52 = deltaQ;
    let _e53 = deltaR;
    let _e55 = deltaQ;
    let _e56 = deltaS;
    if ((_e52 > _e53) && (_e55 > _e56)) {
        let _e59 = r_2;
        let _e61 = s;
        q_2 = (-(_e59) - _e61);
    } else {
        let _e63 = deltaR;
        let _e64 = deltaS;
        if (_e63 > _e64) {
            let _e66 = q_2;
            let _e68 = s;
            r_2 = (-(_e66) - _e68);
        } else {
            let _e70 = q_2;
            let _e72 = r_2;
            s = (-(_e70) - _e72);
        }
    }
    let _e74 = q_2;
    let _e75 = r_2;
    let _e76 = createHexagon(_e74, _e75);
    return _e76;
}

fn hexagonFromPoint(point: vec2<f32>, size: f32) -> Hexagon {
    var point_1: vec2<f32>;
    var size_1: f32;
    var q_3: f32;
    var r_3: f32;
    var hex_3: Hexagon;

    point_1 = point;
    size_1 = size;
    let _e19 = point_1;
    let _e21 = ratio;
    point_1.y = (_e19.y / _e21);
    let _e23 = point_1;
    let _e27 = size_1;
    point_1 = ((_e23 - vec2(0.5f)) / vec2(_e27));
    let _e34 = point_1;
    let _e41 = point_1;
    q_3 = ((0.57735026f * _e34.x) + (-0.33333334f * _e41.y));
    let _e47 = point_1;
    let _e53 = point_1;
    r_3 = ((0f * _e47.x) + (0.6666667f * _e53.y));
    let _e58 = q_3;
    let _e59 = r_3;
    let _e60 = createHexagon(_e58, _e59);
    hex_3 = _e60;
    let _e62 = hex_3;
    let _e63 = roundHexagon(_e62);
    return _e63;
}

fn pointFromHexagon(hex_4: Hexagon, size_2: f32) -> vec2<f32> {
    var hex_5: Hexagon;
    var size_3: f32;
    var x: f32;
    var y: f32;

    hex_5 = hex_4;
    size_3 = size_2;
    let _e20 = hex_5;
    let _e27 = hex_5;
    let _e31 = size_3;
    x = ((((1.7320508f * _e20.q) + (0.8660254f * _e27.r)) * _e31) + 0.5f);
    let _e37 = hex_5;
    let _e43 = hex_5;
    let _e47 = size_3;
    y = ((((0f * _e37.q) + (1.5f * _e43.r)) * _e47) + 0.5f);
    let _e52 = x;
    let _e53 = y;
    let _e54 = ratio;
    return vec2<f32>(_e52, (_e53 * _e54));
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var dist: f32;
    var local: f32;
    var size_4: f32;
    var local_1: vec2<f32>;
    var point_2: vec2<f32>;

    uv_5 = uv_4;
    let _e17 = progress;
    let _e19 = progress;
    dist = (2f * min(_e17, (1f - _e19)));
    let _e24 = steps;
    if (_e24 > 0i) {
        let _e27 = dist;
        let _e28 = steps;
        let _e32 = steps;
        local = (ceil((_e27 * f32(_e28))) / f32(_e32));
    } else {
        let _e35 = dist;
        local = _e35;
    }
    let _e37 = local;
    dist = _e37;
    let _e42 = dist;
    let _e44 = horizontalHexagons;
    size_4 = ((0.57735026f * _e42) / _e44);
    let _e47 = dist;
    if (_e47 > 0f) {
        let _e50 = uv_5;
        let _e51 = size_4;
        let _e52 = hexagonFromPoint(_e50, _e51);
        let _e53 = size_4;
        let _e54 = pointFromHexagon(_e52, _e53);
        local_1 = _e54;
    } else {
        let _e55 = uv_5;
        local_1 = _e55;
    }
    let _e57 = local_1;
    point_2 = _e57;
    let _e59 = point_2;
    let _e60 = getFromColor(_e59);
    let _e61 = point_2;
    let _e62 = getToColor(_e61);
    let _e63 = progress;
    return mix(_e60, _e62, vec4(_e63));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_2: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    steps = i32(_e33.x);
    let _e39 = U.params[1];
    horizontalHexagons = _e39.x;
    chukcut_init_globals();
    let _e41 = v_uv_1;
    let _e44 = v_uv_1;
    let _e48 = transition(vec2<f32>(_e41.x, (1f - _e44.y)));
    c_2 = _e48;
    let _e50 = c_2;
    a = clamp(_e50.w, 0f, 1f);
    let _e56 = a;
    if (_e56 > 0.00001f) {
        let _e59 = c_2;
        let _e61 = a;
        let _e63 = (_e59.xyz / vec3(_e61));
        let _e64 = a;
        local_2 = vec4<f32>(_e63.x, _e63.y, _e63.z, _e64);
    } else {
        local_2 = vec4(0f);
    }
    let _e72 = local_2;
    o_color = _e72;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}

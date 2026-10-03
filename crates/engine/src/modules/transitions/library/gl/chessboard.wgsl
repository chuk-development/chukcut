// chessboard, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: lql
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
var<private> grid_num: f32;

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
    var st: vec2<f32>;
    var idx: vec2<f32>;
    var grid: vec2<f32>;
    var a: vec4<f32>;
    var b: vec4<f32>;
    var checker: f32;
    var mixFactor: f32;
    var local: f32;
    var local_1: f32;

    uv_5 = uv_4;
    let _e15 = uv_5;
    let _e16 = grid_num;
    st = (_e15 * _e16);
    let _e19 = st;
    idx = floor(_e19);
    let _e22 = st;
    grid = fract(_e22);
    let _e25 = uv_5;
    let _e26 = getFromColor(_e25);
    a = _e26;
    let _e28 = uv_5;
    let _e29 = getToColor(_e28);
    b = _e29;
    let _e31 = idx;
    let _e33 = idx;
    let _e35 = (_e31.x + _e33.y);
    checker = (_e35 - (floor((_e35 / 2f)) * 2f));
    let _e43 = progress;
    if (_e43 <= 0.5f) {
        {
            let _e46 = checker;
            if (_e46 > 0.5f) {
                let _e49 = grid;
                let _e51 = progress;
                local = step(_e49.x, (_e51 * 2f));
            } else {
                local = 0f;
            }
            let _e57 = local;
            mixFactor = _e57;
        }
    } else {
        {
            let _e58 = checker;
            if (_e58 < 0.5f) {
                let _e61 = grid;
                let _e63 = progress;
                local_1 = step(_e61.x, ((_e63 - 0.5f) * 2f));
            } else {
                local_1 = 1f;
            }
            let _e71 = local_1;
            mixFactor = _e71;
        }
    }
    let _e72 = a;
    let _e73 = b;
    let _e74 = mixFactor;
    return mix(_e72, _e73, vec4(_e74));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a_1: f32;
    var local_2: vec4<f32>;

    let _e13 = U;
    progress = _e13.state.x;
    let _e17 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e17);
    let _e20 = dims;
    let _e23 = dims;
    ratio = (f32(_e20.x) / f32(max(_e23.y, 1i)));
    let _e32 = U.params[0];
    grid_num = _e32.x;
    chukcut_init_globals();
    let _e34 = v_uv_1;
    let _e37 = v_uv_1;
    let _e41 = transition(vec2<f32>(_e34.x, (1f - _e37.y)));
    c_2 = _e41;
    let _e43 = c_2;
    a_1 = clamp(_e43.w, 0f, 1f);
    let _e49 = a_1;
    if (_e49 > 0.00001f) {
        let _e52 = c_2;
        let _e54 = a_1;
        let _e56 = (_e52.xyz / vec3(_e54));
        let _e57 = a_1;
        local_2 = vec4<f32>(_e56.x, _e56.y, _e56.z, _e57);
    } else {
        local_2 = vec4(0f);
    }
    let _e65 = local_2;
    o_color = _e65;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e24 = o_color;
    return FragmentOutput(_e24);
}

// squareswire, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
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

const center: vec2<f32> = vec2<f32>(0.5f, 0.5f);

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
var<private> squares: vec2<i32>;
var<private> direction: vec2<f32>;
var<private> smoothness: f32;

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
    var v: vec2<f32>;
    var d: f32;
    var offset: f32;
    var pr: f32;
    var squarep: vec2<f32>;
    var squaremin: vec2<f32>;
    var squaremax: vec2<f32>;
    var a: f32;

    p_1 = p;
    let _e18 = direction;
    v = normalize(_e18);
    let _e21 = v;
    let _e22 = v;
    let _e25 = v;
    v = (_e21 / vec2((abs(_e22.x) + abs(_e25.y))));
    let _e31 = v;
    let _e37 = v;
    d = ((_e31.x * 0.5f) + (_e37.y * 0.5f));
    let _e45 = smoothness;
    offset = _e45;
    let _e47 = offset;
    let _e50 = v;
    let _e52 = p_1;
    let _e55 = v;
    let _e57 = p_1;
    let _e61 = d;
    let _e64 = progress;
    let _e66 = offset;
    pr = smoothstep(-(_e47), 0f, (((_e50.x * _e52.x) + (_e55.y * _e57.y)) - ((_e61 - 0.5f) + (_e64 * (1f + _e66)))));
    let _e73 = p_1;
    let _e74 = squares;
    squarep = fract((_e73 * vec2<f32>(_e74)));
    let _e79 = pr;
    squaremin = vec2((_e79 / 2f));
    let _e85 = pr;
    squaremax = vec2((1f - (_e85 / 2f)));
    let _e92 = progress;
    let _e96 = squaremin;
    let _e98 = squarep;
    let _e102 = squaremin;
    let _e104 = squarep;
    let _e108 = squarep;
    let _e110 = squaremax;
    let _e114 = squarep;
    let _e116 = squaremax;
    a = (((((1f - step(_e92, 0f)) * step(_e96.x, _e98.x)) * step(_e102.y, _e104.y)) * step(_e108.x, _e110.x)) * step(_e114.y, _e116.y));
    let _e121 = p_1;
    let _e122 = getFromColor(_e121);
    let _e123 = p_1;
    let _e124 = getToColor(_e123);
    let _e125 = a;
    return mix(_e122, _e124, vec4(_e125));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a_1: f32;
    var local: vec4<f32>;

    let _e16 = U;
    progress = _e16.state.x;
    let _e20 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e20);
    let _e23 = dims;
    let _e26 = dims;
    ratio = (f32(_e23.x) / f32(max(_e26.y, 1i)));
    let _e35 = U.params[0];
    squares = vec2<i32>(_e35.xy);
    let _e41 = U.params[1];
    direction = _e41.xy;
    let _e46 = U.params[2];
    smoothness = _e46.x;
    chukcut_init_globals();
    let _e48 = v_uv_1;
    let _e51 = v_uv_1;
    let _e55 = transition(vec2<f32>(_e48.x, (1f - _e51.y)));
    c_2 = _e55;
    let _e57 = c_2;
    a_1 = clamp(_e57.w, 0f, 1f);
    let _e63 = a_1;
    if (_e63 > 0.00001f) {
        let _e66 = c_2;
        let _e68 = a_1;
        let _e70 = (_e66.xyz / vec3(_e68));
        let _e71 = a_1;
        local = vec4<f32>(_e70.x, _e70.y, _e70.z, _e71);
    } else {
        local = vec4(0f);
    }
    let _e79 = local;
    o_color = _e79;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e30 = o_color;
    return FragmentOutput(_e30);
}

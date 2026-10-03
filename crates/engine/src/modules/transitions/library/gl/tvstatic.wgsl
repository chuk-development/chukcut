// TVStatic, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Brandon Anzaldi
// License: MIT
// Pseudo-random noise function
// http://byteblacksmith.com/improvements-to-the-canonical-one-liner-glsl-rand-for-opengl-es-2-0/
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
var<private> offset: f32;

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

fn noise(co: vec2<f32>) -> f32 {
    var co_1: vec2<f32>;
    var a: f32 = 12.9898f;
    var b: f32 = 78.233f;
    var c_2: f32 = 43758.547f;
    var dt: f32;
    var sn: f32;

    co_1 = co;
    let _e21 = co_1;
    let _e23 = progress;
    let _e25 = a;
    let _e26 = b;
    dt = dot((_e21.xy * _e23), vec2<f32>(_e25, _e26));
    let _e30 = dt;
    sn = (_e30 - (floor((_e30 / 3.14f)) * 3.14f));
    let _e37 = sn;
    let _e39 = c_2;
    return fract((sin(_e37) * _e39));
}

fn transition(p: vec2<f32>) -> vec4<f32> {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e15 = progress;
    let _e16 = offset;
    if (_e15 < _e16) {
        {
            let _e18 = p_1;
            let _e19 = getFromColor(_e18);
            return _e19;
        }
    } else {
        let _e20 = progress;
        let _e22 = offset;
        if (_e20 > (1f - _e22)) {
            {
                let _e25 = p_1;
                let _e26 = getToColor(_e25);
                return _e26;
            }
        } else {
            {
                let _e27 = p_1;
                let _e28 = noise(_e27);
                let _e29 = vec3(_e28);
                return vec4<f32>(_e29.x, _e29.y, _e29.z, 1f);
            }
        }
    }
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
    var a_1: f32;
    var local: vec4<f32>;

    let _e13 = U;
    progress = _e13.state.x;
    let _e17 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e17);
    let _e20 = dims;
    let _e23 = dims;
    ratio = (f32(_e20.x) / f32(max(_e23.y, 1i)));
    let _e32 = U.params[0];
    offset = _e32.x;
    chukcut_init_globals();
    let _e34 = v_uv_1;
    let _e37 = v_uv_1;
    let _e41 = transition(vec2<f32>(_e34.x, (1f - _e37.y)));
    c_3 = _e41;
    let _e43 = c_3;
    a_1 = clamp(_e43.w, 0f, 1f);
    let _e49 = a_1;
    if (_e49 > 0.00001f) {
        let _e52 = c_3;
        let _e54 = a_1;
        let _e56 = (_e52.xyz / vec3(_e54));
        let _e57 = a_1;
        local = vec4<f32>(_e56.x, _e56.y, _e56.z, _e57);
    } else {
        local = vec4(0f);
    }
    let _e65 = local;
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

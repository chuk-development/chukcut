// Swirl, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Sergey Kosarevsky
// License: MIT
// ( http://www.linderdaum.com )
// ported by gre from https://gist.github.com/corporateshark/cacfedb8cca0f5ce3f7c
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

fn transition(UV: vec2<f32>) -> vec4<f32> {
    var UV_1: vec2<f32>;
    var Radius: f32 = 1f;
    var T: f32;
    var Dist: f32;
    var Percent: f32;
    var local: f32;
    var A: f32;
    var Theta: f32;
    var S: f32;
    var C: f32;
    var C0_: vec4<f32>;
    var C1_: vec4<f32>;

    UV_1 = UV;
    let _e16 = progress;
    T = _e16;
    let _e18 = UV_1;
    UV_1 = (_e18 - vec2<f32>(0.5f, 0.5f));
    let _e23 = UV_1;
    Dist = length(_e23);
    let _e26 = Dist;
    let _e27 = Radius;
    if (_e26 < _e27) {
        {
            let _e29 = Radius;
            let _e30 = Dist;
            let _e32 = Radius;
            Percent = ((_e29 - _e30) / _e32);
            let _e35 = T;
            if (_e35 <= 0.5f) {
                let _e40 = T;
                local = mix(0f, 1f, (_e40 / 0.5f));
            } else {
                let _e46 = T;
                local = mix(1f, 0f, ((_e46 - 0.5f) / 0.5f));
            }
            let _e53 = local;
            A = _e53;
            let _e55 = Percent;
            let _e56 = Percent;
            let _e58 = A;
            Theta = ((((_e55 * _e56) * _e58) * 8f) * 3.14159f);
            let _e65 = Theta;
            S = sin(_e65);
            let _e68 = Theta;
            C = cos(_e68);
            let _e71 = UV_1;
            let _e72 = C;
            let _e73 = S;
            let _e77 = UV_1;
            let _e78 = S;
            let _e79 = C;
            UV_1 = vec2<f32>(dot(_e71, vec2<f32>(_e72, -(_e73))), dot(_e77, vec2<f32>(_e78, _e79)));
        }
    }
    let _e83 = UV_1;
    UV_1 = (_e83 + vec2<f32>(0.5f, 0.5f));
    let _e88 = UV_1;
    let _e89 = getFromColor(_e88);
    C0_ = _e89;
    let _e91 = UV_1;
    let _e92 = getToColor(_e91);
    C1_ = _e92;
    let _e94 = C0_;
    let _e95 = C1_;
    let _e96 = T;
    return mix(_e94, _e95, vec4(_e96));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_1: vec4<f32>;

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
        local_1 = vec4<f32>(_e50.x, _e50.y, _e50.z, _e51);
    } else {
        local_1 = vec4(0f);
    }
    let _e59 = local_1;
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

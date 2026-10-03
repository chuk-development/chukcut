// Mosaic, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Xaychru
// License: MIT
// ported by gre from https://gist.github.com/Xaychru/130bb7b7affedbda9df5
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
var<private> endx: i32;
var<private> endy: i32;

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

fn Rand(v: vec2<f32>) -> f32 {
    var v_1: vec2<f32>;

    v_1 = v;
    let _e16 = v_1;
    return fract((sin(dot(_e16.xy, vec2<f32>(12.9898f, 78.233f))) * 43758.547f));
}

fn Rotate(v_2: vec2<f32>, a: f32) -> vec2<f32> {
    var v_3: vec2<f32>;
    var a_1: f32;
    var rm: mat2x2<f32>;

    v_3 = v_2;
    a_1 = a;
    let _e18 = a_1;
    let _e20 = a_1;
    let _e23 = a_1;
    let _e25 = a_1;
    rm = mat2x2<f32>(vec2<f32>(cos(_e18), -(sin(_e20))), vec2<f32>(sin(_e23), cos(_e25)));
    let _e31 = rm;
    let _e32 = v_3;
    return (_e31 * _e32);
}

fn CosInterpolation(x: f32) -> f32 {
    var x_1: f32;

    x_1 = x;
    let _e16 = x_1;
    return ((-(cos((_e16 * 3.1415927f))) / 2f) + 0.5f);
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var p: vec2<f32>;
    var rp: vec2<f32>;
    var rpr: f32;
    var z: f32;
    var az: f32;
    var mrp: vec2<f32>;
    var crp: vec2<f32>;
    var onEnd: bool;
    var ang: f32;

    uv_5 = uv_4;
    let _e16 = uv_5;
    p = ((_e16.xy / vec2(1f)) - vec2(0.5f));
    let _e26 = p;
    rp = _e26;
    let _e28 = progress;
    rpr = ((_e28 * 2f) - 1f);
    let _e34 = rpr;
    let _e35 = rpr;
    z = (-(((_e34 * _e35) * 2f)) + 3f);
    let _e43 = z;
    az = abs(_e43);
    let _e46 = rp;
    let _e47 = az;
    rp = (_e46 * _e47);
    let _e49 = rp;
    let _e53 = endx;
    let _e57 = endy;
    let _e62 = progress;
    let _e63 = CosInterpolation(_e62);
    let _e64 = progress;
    let _e65 = CosInterpolation(_e64);
    rp = (_e49 + mix(vec2<f32>(0.5f, 0.5f), vec2<f32>((f32(_e53) + 0.5f), (f32(_e57) + 0.5f)), vec2((_e63 * _e65))));
    let _e70 = rp;
    let _e72 = vec2(1f);
    mrp = (_e70 - (floor((_e70 / _e72)) * _e72));
    let _e78 = rp;
    crp = _e78;
    let _e80 = crp;
    let _e84 = endx;
    let _e86 = crp;
    let _e90 = endy;
    onEnd = ((i32(floor(_e80.x)) == _e84) && (i32(floor(_e86.y)) == _e90));
    let _e94 = onEnd;
    if !(_e94) {
        {
            let _e96 = crp;
            let _e98 = Rand(floor(_e96));
            ang = ((f32(i32((_e98 * 4f))) * 0.5f) * 3.1415927f);
            let _e110 = mrp;
            let _e114 = ang;
            let _e115 = Rotate((_e110 - vec2(0.5f)), _e114);
            mrp = (vec2(0.5f) + _e115);
        }
    }
    let _e117 = onEnd;
    let _e118 = crp;
    let _e120 = Rand(floor(_e118));
    if (_e117 || (_e120 > 0.5f)) {
        {
            let _e124 = mrp;
            let _e125 = getToColor(_e124);
            return _e125;
        }
    } else {
        {
            let _e126 = mrp;
            let _e127 = getFromColor(_e126);
            return _e127;
        }
    }
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a_2: f32;
    var local: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    endx = i32(_e33.x);
    let _e39 = U.params[1];
    endy = i32(_e39.x);
    chukcut_init_globals();
    let _e42 = v_uv_1;
    let _e45 = v_uv_1;
    let _e49 = transition(vec2<f32>(_e42.x, (1f - _e45.y)));
    c_2 = _e49;
    let _e51 = c_2;
    a_2 = clamp(_e51.w, 0f, 1f);
    let _e57 = a_2;
    if (_e57 > 0.00001f) {
        let _e60 = c_2;
        let _e62 = a_2;
        let _e64 = (_e60.xyz / vec3(_e62));
        let _e65 = a_2;
        local = vec4<f32>(_e64.x, _e64.y, _e64.z, _e65);
    } else {
        local = vec4(0f);
    }
    let _e73 = local;
    o_color = _e73;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}

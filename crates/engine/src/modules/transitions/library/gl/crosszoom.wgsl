// CrossZoom, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: rectalogic
// License: MIT
// ported by gre from https://gist.github.com/rectalogic/b86b90161503a0023231
// Converted from https://github.com/rectalogic/rendermix-basic-effects/blob/master/assets/com/rendermix/CrossZoom/CrossZoom.frag
// Which is based on https://github.com/evanw/glfx.js/blob/master/src/filters/blur/zoomblur.js
// With additional easing functions from https://github.com/rectalogic/rendermix-basic-effects/blob/master/assets/com/rendermix/Easing/Easing.glsllib
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
var<private> strength_1: f32;

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

fn Linear_ease(begin: f32, change: f32, duration: f32, time: f32) -> f32 {
    var begin_1: f32;
    var change_1: f32;
    var duration_1: f32;
    var time_1: f32;

    begin_1 = begin;
    change_1 = change;
    duration_1 = duration;
    time_1 = time;
    let _e22 = change_1;
    let _e23 = time_1;
    let _e25 = duration_1;
    let _e27 = begin_1;
    return (((_e22 * _e23) / _e25) + _e27);
}

fn Exponential_easeInOut(begin_2: f32, change_2: f32, duration_2: f32, time_2: f32) -> f32 {
    var begin_3: f32;
    var change_3: f32;
    var duration_3: f32;
    var time_3: f32;

    begin_3 = begin_2;
    change_3 = change_2;
    duration_3 = duration_2;
    time_3 = time_2;
    let _e22 = time_3;
    if (_e22 == 0f) {
        let _e25 = begin_3;
        return _e25;
    } else {
        let _e26 = time_3;
        let _e27 = duration_3;
        if (_e26 == _e27) {
            let _e29 = begin_3;
            let _e30 = change_3;
            return (_e29 + _e30);
        }
    }
    let _e32 = time_3;
    let _e33 = duration_3;
    time_3 = (_e32 / (_e33 / 2f));
    let _e37 = time_3;
    if (_e37 < 1f) {
        let _e40 = change_3;
        let _e45 = time_3;
        let _e51 = begin_3;
        return (((_e40 / 2f) * pow(2f, (10f * (_e45 - 1f)))) + _e51);
    }
    let _e53 = change_3;
    let _e59 = time_3;
    let _e68 = begin_3;
    return (((_e53 / 2f) * (-(pow(2f, (-10f * (_e59 - 1f)))) + 2f)) + _e68);
}

fn Sinusoidal_easeInOut(begin_4: f32, change_4: f32, duration_4: f32, time_4: f32) -> f32 {
    var begin_5: f32;
    var change_5: f32;
    var duration_5: f32;
    var time_5: f32;

    begin_5 = begin_4;
    change_5 = change_4;
    duration_5 = duration_4;
    time_5 = time_4;
    let _e22 = change_5;
    let _e26 = time_5;
    let _e28 = duration_5;
    let _e34 = begin_5;
    return (((-(_e22) / 2f) * (cos(((PI * _e26) / _e28)) - 1f)) + _e34);
}

fn rand(co: vec2<f32>) -> f32 {
    var co_1: vec2<f32>;

    co_1 = co;
    let _e16 = co_1;
    return fract((sin(dot(_e16.xy, vec2<f32>(12.9898f, 78.233f))) * 43758.547f));
}

fn crossFade(uv_4: vec2<f32>, dissolve: f32) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var dissolve_1: f32;

    uv_5 = uv_4;
    dissolve_1 = dissolve;
    let _e18 = uv_5;
    let _e19 = getFromColor(_e18);
    let _e20 = uv_5;
    let _e21 = getToColor(_e20);
    let _e22 = dissolve_1;
    return mix(_e19, _e21, vec4(_e22));
}

fn transition(uv_6: vec2<f32>) -> vec4<f32> {
    var uv_7: vec2<f32>;
    var texCoord: vec2<f32>;
    var center: vec2<f32>;
    var dissolve_2: f32;
    var strength: f32;
    var color: vec4<f32> = vec4(0f);
    var total: f32 = 0f;
    var toCenter: vec2<f32>;
    var offset: f32;
    var t: f32 = 0f;
    var percent: f32;
    var weight: f32;

    uv_7 = uv_6;
    let _e16 = uv_7;
    texCoord = (_e16.xy / vec2(1f));
    let _e26 = progress;
    let _e27 = Linear_ease(0.25f, 0.5f, 1f, _e26);
    center = vec2<f32>(_e27, 0.5f);
    let _e34 = progress;
    let _e35 = Exponential_easeInOut(0f, 1f, 1f, _e34);
    dissolve_2 = _e35;
    let _e38 = strength_1;
    let _e40 = progress;
    let _e41 = Sinusoidal_easeInOut(0f, _e38, 0.5f, _e40);
    strength = _e41;
    let _e48 = center;
    let _e49 = texCoord;
    toCenter = (_e48 - _e49);
    let _e52 = uv_7;
    let _e53 = rand(_e52);
    offset = _e53;
    loop {
        let _e57 = t;
        if !((_e57 <= 40f)) {
            break;
        }
        {
            let _e64 = t;
            let _e65 = offset;
            percent = ((_e64 + _e65) / 40f);
            let _e71 = percent;
            let _e72 = percent;
            let _e73 = percent;
            weight = (4f * (_e71 - (_e72 * _e73)));
            let _e78 = color;
            let _e79 = texCoord;
            let _e80 = toCenter;
            let _e81 = percent;
            let _e83 = strength;
            let _e86 = dissolve_2;
            let _e87 = crossFade((_e79 + ((_e80 * _e81) * _e83)), _e86);
            let _e88 = weight;
            color = (_e78 + (_e87 * _e88));
            let _e91 = total;
            let _e92 = weight;
            total = (_e91 + _e92);
        }
        continuing {
            let _e61 = t;
            t = (_e61 + 1f);
        }
    }
    let _e94 = color;
    let _e95 = total;
    return (_e94 / vec4(_e95));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    strength_1 = _e33.x;
    chukcut_init_globals();
    let _e35 = v_uv_1;
    let _e38 = v_uv_1;
    let _e42 = transition(vec2<f32>(_e35.x, (1f - _e38.y)));
    c_2 = _e42;
    let _e44 = c_2;
    a = clamp(_e44.w, 0f, 1f);
    let _e50 = a;
    if (_e50 > 0.00001f) {
        let _e53 = c_2;
        let _e55 = a;
        let _e57 = (_e53.xyz / vec3(_e55));
        let _e58 = a;
        local = vec4<f32>(_e57.x, _e57.y, _e57.z, _e58);
    } else {
        local = vec4(0f);
    }
    let _e66 = local;
    o_color = _e66;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}

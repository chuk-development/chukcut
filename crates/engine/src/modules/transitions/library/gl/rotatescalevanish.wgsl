// RotateScaleVanish, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Mark Craig
// License: MIT
// mrmcsoftware on github and youtube ( http://www.youtube.com/MrMcSoftware )
// RotateScaleVanish Transition by Mark Craig (Copyright © 2022)
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
var<private> FadeInSecond: bool;
var<private> ReverseEffect: bool;
var<private> ReverseRotation: bool;

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
    var iResolution: vec2<f32>;
    var local: f32;
    var t: f32;
    var local_1: f32;
    var theta: f32;
    var c1_: f32;
    var s1_: f32;
    var rad: f32;
    var xc1_: f32;
    var yc1_: f32;
    var xc2_: f32;
    var yc2_: f32;
    var uv2_: vec2<f32>;
    var col3_: vec4<f32>;
    var local_2: vec4<f32>;
    var ColorTo: vec4<f32>;
    var local_3: vec4<f32>;
    var local_4: vec4<f32>;

    uv_5 = uv_4;
    let _e17 = ratio;
    iResolution = vec2<f32>(_e17, 1f);
    let _e21 = ReverseEffect;
    if _e21 {
        let _e23 = progress;
        local = (1f - _e23);
    } else {
        let _e25 = progress;
        local = _e25;
    }
    let _e27 = local;
    t = _e27;
    let _e29 = ReverseRotation;
    if _e29 {
        let _e31 = t;
        local_1 = (6.2831855f * _e31);
    } else {
        let _e35 = t;
        local_1 = (-6.2831855f * _e35);
    }
    let _e38 = local_1;
    theta = _e38;
    let _e40 = theta;
    c1_ = cos(_e40);
    let _e43 = theta;
    s1_ = sin(_e43);
    let _e48 = t;
    rad = max(0.00001f, (1f - _e48));
    let _e52 = uv_5;
    let _e56 = iResolution;
    xc1_ = ((_e52.x - 0.5f) * _e56.x);
    let _e60 = uv_5;
    let _e64 = iResolution;
    yc1_ = ((_e60.y - 0.5f) * _e64.y);
    let _e68 = xc1_;
    let _e69 = c1_;
    let _e71 = yc1_;
    let _e72 = s1_;
    let _e75 = rad;
    xc2_ = (((_e68 * _e69) - (_e71 * _e72)) / _e75);
    let _e78 = xc1_;
    let _e79 = s1_;
    let _e81 = yc1_;
    let _e82 = c1_;
    let _e85 = rad;
    yc2_ = (((_e78 * _e79) + (_e81 * _e82)) / _e85);
    let _e88 = xc2_;
    let _e89 = iResolution;
    let _e94 = yc2_;
    let _e95 = iResolution;
    uv2_ = vec2<f32>((_e88 + (_e89.x / 2f)), (_e94 + (_e95.y / 2f)));
    let _e103 = ReverseEffect;
    if _e103 {
        let _e104 = uv_5;
        let _e105 = getFromColor(_e104);
        local_2 = _e105;
    } else {
        let _e106 = uv_5;
        let _e107 = getToColor(_e106);
        local_2 = _e107;
    }
    let _e109 = local_2;
    ColorTo = _e109;
    let _e111 = uv2_;
    let _e115 = uv2_;
    let _e117 = iResolution;
    let _e121 = uv2_;
    let _e126 = uv2_;
    let _e128 = iResolution;
    if ((((_e111.x >= 0f) && (_e115.x <= _e117.x)) && (_e121.y >= 0f)) && (_e126.y <= _e128.y)) {
        {
            let _e132 = uv2_;
            let _e133 = iResolution;
            uv2_ = (_e132 / _e133);
            let _e135 = ReverseEffect;
            if _e135 {
                let _e136 = uv2_;
                let _e137 = getToColor(_e136);
                local_3 = _e137;
            } else {
                let _e138 = uv2_;
                let _e139 = getFromColor(_e138);
                local_3 = _e139;
            }
            let _e141 = local_3;
            col3_ = _e141;
        }
    } else {
        {
            let _e142 = FadeInSecond;
            if _e142 {
                local_4 = vec4<f32>(0f, 0f, 0f, 1f);
            } else {
                let _e148 = ColorTo;
                local_4 = _e148;
            }
            let _e150 = local_4;
            col3_ = _e150;
        }
    }
    let _e152 = t;
    let _e154 = col3_;
    let _e156 = t;
    let _e157 = ColorTo;
    return (((1f - _e152) * _e154) + (_e156 * _e157));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_5: vec4<f32>;

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    let _e34 = U.params[0];
    FadeInSecond = (_e34.x > 0.5f);
    let _e41 = U.params[1];
    ReverseEffect = (_e41.x > 0.5f);
    let _e48 = U.params[2];
    ReverseRotation = (_e48.x > 0.5f);
    chukcut_init_globals();
    let _e52 = v_uv_1;
    let _e55 = v_uv_1;
    let _e59 = transition(vec2<f32>(_e52.x, (1f - _e55.y)));
    c_2 = _e59;
    let _e61 = c_2;
    a = clamp(_e61.w, 0f, 1f);
    let _e67 = a;
    if (_e67 > 0.00001f) {
        let _e70 = c_2;
        let _e72 = a;
        let _e74 = (_e70.xyz / vec3(_e72));
        let _e75 = a;
        local_5 = vec4<f32>(_e74.x, _e74.y, _e74.z, _e75);
    } else {
        local_5 = vec4(0f);
    }
    let _e83 = local_5;
    o_color = _e83;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}

// ZoomInCircles, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: dycm8009
// License: MIT
// ported by gre from https://gist.github.com/dycm8009/948e99b1800e81ad909a
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
var<private> ratio2_: vec2<f32>;

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

fn zoom(uv_4: vec2<f32>, amount: f32) -> vec2<f32> {
    var uv_5: vec2<f32>;
    var amount_1: f32;

    uv_5 = uv_4;
    amount_1 = amount;
    let _e17 = uv_5;
    let _e21 = amount_1;
    return (vec2(0.5f) + ((_e17 - vec2(0.5f)) * _e21));
}

fn transition(uv_6: vec2<f32>) -> vec4<f32> {
    var uv_7: vec2<f32>;
    var r: vec2<f32>;
    var pro: f32;
    var z: f32;
    var t: f32 = 0f;

    uv_7 = uv_6;
    let _e16 = uv_7;
    let _e22 = ratio2_;
    r = (2f * ((vec2<f32>(_e16.xy) - vec2(0.5f)) * _e22));
    let _e26 = progress;
    pro = (_e26 / 0.8f);
    let _e30 = pro;
    z = (_e30 * 0.2f);
    let _e36 = pro;
    if (_e36 > 1f) {
        {
            let _e40 = pro;
            z = (0.2f + ((_e40 - 1f) * 5f));
            let _e46 = progress;
            t = clamp(((_e46 - 0.8f) / 0.07f), 0f, 1f);
        }
    }
    let _e54 = r;
    let _e57 = z;
    if (length(_e54) < (0.5f + _e57)) {
        {
        }
    } else {
        let _e60 = r;
        let _e63 = z;
        if (length(_e60) < (0.8f + (_e63 * 1.5f))) {
            {
                let _e68 = uv_7;
                let _e71 = pro;
                let _e74 = zoom(_e68, (1f - (0.15f * _e71)));
                uv_7 = _e74;
                let _e75 = t;
                t = (_e75 * 0.5f);
            }
        } else {
            let _e78 = r;
            let _e81 = z;
            if (length(_e78) < (1.2f + (_e81 * 2.5f))) {
                {
                    let _e86 = uv_7;
                    let _e89 = pro;
                    let _e92 = zoom(_e86, (1f - (0.2f * _e89)));
                    uv_7 = _e92;
                    let _e93 = t;
                    t = (_e93 * 0.2f);
                }
            } else {
                {
                    let _e96 = uv_7;
                    let _e99 = pro;
                    let _e102 = zoom(_e96, (1f - (0.25f * _e99)));
                    uv_7 = _e102;
                }
            }
        }
    }
    let _e103 = uv_7;
    let _e104 = getFromColor(_e103);
    let _e105 = uv_7;
    let _e106 = getToColor(_e105);
    let _e107 = t;
    return mix(_e104, _e106, vec4(_e107));
}

fn chukcut_init_globals() {
    let _e15 = ratio;
    ratio2_ = vec2<f32>(1f, (1f / _e15));
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e13 = U;
    progress = _e13.state.x;
    let _e17 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e17);
    let _e20 = dims;
    let _e23 = dims;
    ratio = (f32(_e20.x) / f32(max(_e23.y, 1i)));
    chukcut_init_globals();
    let _e29 = v_uv_1;
    let _e32 = v_uv_1;
    let _e36 = transition(vec2<f32>(_e29.x, (1f - _e32.y)));
    c_2 = _e36;
    let _e38 = c_2;
    a = clamp(_e38.w, 0f, 1f);
    let _e44 = a;
    if (_e44 > 0.00001f) {
        let _e47 = c_2;
        let _e49 = a;
        let _e51 = (_e47.xyz / vec3(_e49));
        let _e52 = a;
        local = vec4<f32>(_e51.x, _e51.y, _e51.z, _e52);
    } else {
        local = vec4(0f);
    }
    let _e60 = local;
    o_color = _e60;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e24 = o_color;
    return FragmentOutput(_e24);
}

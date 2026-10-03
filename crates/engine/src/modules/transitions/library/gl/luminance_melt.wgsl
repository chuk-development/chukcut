// luminance_melt, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: 0gust1
// License: MIT
//My own first transition — based on crosshatch code (from pthrasher), using  simplex noise formula (copied and pasted)
//-> cooler with high contrasted images (isolated dark subject on light background f.e.)
//TODO : try to rebase it on DoomTransition (from zeh)?
//optimizations :
//luminance (see http://stackoverflow.com/questions/596216/formula-to-determine-brightness-of-rgb-color#answer-596241)
// Y = (R+R+B+G+G+G)/6
//or Y = (R+R+R+B+G+G+G+G)>>3
//direction of movement :  0 : up, 1, down
//luminance threshold
//does the movement takes effect above or below luminance threshold ?
//Random function borrowed from everywhere
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
var<private> direction: bool;
var<private> l_threshold: f32;
var<private> above: bool;
var<private> center: vec2<f32>;

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

fn rand(co: vec2<f32>) -> f32 {
    var co_1: vec2<f32>;

    co_1 = co;
    let _e17 = co_1;
    return fract((sin(dot(_e17.xy, vec2<f32>(12.9898f, 78.233f))) * 43758.547f));
}

fn mod289_(x: vec3<f32>) -> vec3<f32> {
    var x_1: vec3<f32>;

    x_1 = x;
    let _e17 = x_1;
    let _e18 = x_1;
    return (_e17 - (floor((_e18 * 0.0034602077f)) * 289f));
}

fn mod289_1(x_2: vec2<f32>) -> vec2<f32> {
    var x_3: vec2<f32>;

    x_3 = x_2;
    let _e17 = x_3;
    let _e18 = x_3;
    return (_e17 - (floor((_e18 * 0.0034602077f)) * 289f));
}

fn permute(x_4: vec3<f32>) -> vec3<f32> {
    var x_5: vec3<f32>;

    x_5 = x_4;
    let _e17 = x_5;
    let _e23 = x_5;
    let _e25 = mod289_((((_e17 * 34f) + vec3(1f)) * _e23));
    return _e25;
}

fn snoise(v: vec2<f32>) -> f32 {
    var v_1: vec2<f32>;
    var C: vec4<f32> = vec4<f32>(0.21132487f, 0.36602542f, -0.57735026f, 0.024390243f);
    var i: vec2<f32>;
    var x0_: vec2<f32>;
    var i1_: vec2<f32>;
    var local: vec2<f32>;
    var x12_: vec4<f32>;
    var p: vec3<f32>;
    var m: vec3<f32>;
    var x_6: vec3<f32>;
    var h: vec3<f32>;
    var ox: vec3<f32>;
    var a0_: vec3<f32>;
    var g: vec3<f32>;

    v_1 = v;
    let _e24 = v_1;
    let _e25 = v_1;
    let _e26 = C;
    i = floor((_e24 + vec2(dot(_e25, _e26.yy))));
    let _e33 = v_1;
    let _e34 = i;
    let _e36 = i;
    let _e37 = C;
    x0_ = ((_e33 - _e34) + vec2(dot(_e36, _e37.xx)));
    let _e44 = x0_;
    let _e46 = x0_;
    if (_e44.x > _e46.y) {
        local = vec2<f32>(1f, 0f);
    } else {
        local = vec2<f32>(0f, 1f);
    }
    let _e56 = local;
    i1_ = _e56;
    let _e57 = x0_;
    let _e59 = C;
    x12_ = (_e57.xyxy + _e59.xxzz);
    let _e63 = x12_;
    let _e65 = x12_;
    let _e67 = i1_;
    let _e68 = (_e65.xy - _e67);
    x12_.x = _e68.x;
    x12_.y = _e68.y;
    let _e73 = i;
    let _e74 = mod289_1(_e73);
    i = _e74;
    let _e75 = i;
    let _e78 = i1_;
    let _e84 = permute((vec3(_e75.y) + vec3<f32>(0f, _e78.y, 1f)));
    let _e85 = i;
    let _e90 = i1_;
    let _e95 = permute(((_e84 + vec3(_e85.x)) + vec3<f32>(0f, _e90.x, 1f)));
    p = _e95;
    let _e98 = x0_;
    let _e99 = x0_;
    let _e101 = x12_;
    let _e103 = x12_;
    let _e106 = x12_;
    let _e108 = x12_;
    m = max((vec3(0.5f) - vec3<f32>(dot(_e98, _e99), dot(_e101.xy, _e103.xy), dot(_e106.zw, _e108.zw))), vec3(0f));
    let _e118 = m;
    let _e119 = m;
    m = (_e118 * _e119);
    let _e121 = m;
    let _e122 = m;
    m = (_e121 * _e122);
    let _e125 = p;
    let _e126 = C;
    x_6 = ((2f * fract((_e125 * _e126.www))) - vec3(1f));
    let _e135 = x_6;
    h = (abs(_e135) - vec3(0.5f));
    let _e141 = x_6;
    ox = floor((_e141 + vec3(0.5f)));
    let _e147 = x_6;
    let _e148 = ox;
    a0_ = (_e147 - _e148);
    let _e151 = m;
    let _e154 = a0_;
    let _e155 = a0_;
    let _e157 = h;
    let _e158 = h;
    m = (_e151 * (vec3(1.7928429f) - (0.85373473f * ((_e154 * _e155) + (_e157 * _e158)))));
    let _e167 = a0_;
    let _e169 = x0_;
    let _e172 = h;
    let _e174 = x0_;
    g.x = ((_e167.x * _e169.x) + (_e172.x * _e174.y));
    let _e178 = g;
    let _e180 = a0_;
    let _e182 = x12_;
    let _e185 = h;
    let _e187 = x12_;
    let _e190 = ((_e180.yz * _e182.xz) + (_e185.yz * _e187.yw));
    g.y = _e190.x;
    g.z = _e190.y;
    let _e196 = m;
    let _e197 = g;
    return (130f * dot(_e196, _e197));
}

fn luminance(color: vec4<f32>) -> f32 {
    var color_1: vec4<f32>;

    color_1 = color;
    let _e17 = color_1;
    let _e21 = color_1;
    let _e26 = color_1;
    return (((_e17.x * 0.299f) + (_e21.y * 0.587f)) + (_e26.z * 0.114f));
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var p_1: vec2<f32>;
    var x_7: f32;
    var dist: f32;
    var r: f32;
    var m_1: f32;
    var local_1: f32;
    var local_2: f32;

    uv_5 = uv_4;
    let _e18 = uv_5;
    p_1 = (_e18.xy / vec2(1f));
    let _e25 = progress;
    if (_e25 == 0f) {
        {
            let _e28 = p_1;
            let _e29 = getFromColor(_e28);
            return _e29;
        }
    } else {
        let _e30 = progress;
        if (_e30 == 1f) {
            {
                let _e33 = p_1;
                let _e34 = getToColor(_e33);
                return _e34;
            }
        } else {
            {
                let _e35 = progress;
                x_7 = _e35;
                let _e37 = center;
                let _e38 = p_1;
                let _e40 = progress;
                let _e41 = p_1;
                let _e45 = snoise(vec2<f32>(_e41.x, 0f));
                dist = (distance(_e37, _e38) - (_e40 * exp(_e45)));
                let _e50 = x_7;
                let _e51 = p_1;
                let _e55 = rand(vec2<f32>(_e51.x, 0.1f));
                r = (_e50 - _e55);
                let _e59 = above;
                if _e59 {
                    {
                        let _e60 = dist;
                        let _e61 = r;
                        let _e63 = p_1;
                        let _e64 = getFromColor(_e63);
                        let _e65 = luminance(_e64);
                        let _e66 = l_threshold;
                        if ((_e60 <= _e61) && (_e65 > _e66)) {
                            local_1 = 1f;
                        } else {
                            let _e70 = progress;
                            let _e71 = progress;
                            let _e73 = progress;
                            local_1 = ((_e70 * _e71) * _e73);
                        }
                        let _e76 = local_1;
                        m_1 = _e76;
                    }
                } else {
                    {
                        let _e77 = dist;
                        let _e78 = r;
                        let _e80 = p_1;
                        let _e81 = getFromColor(_e80);
                        let _e82 = luminance(_e81);
                        let _e83 = l_threshold;
                        if ((_e77 <= _e78) && (_e82 < _e83)) {
                            local_2 = 1f;
                        } else {
                            let _e87 = progress;
                            let _e88 = progress;
                            let _e90 = progress;
                            local_2 = ((_e87 * _e88) * _e90);
                        }
                        let _e93 = local_2;
                        m_1 = _e93;
                    }
                }
                let _e94 = p_1;
                let _e95 = getFromColor(_e94);
                let _e96 = p_1;
                let _e97 = getToColor(_e96);
                let _e98 = m_1;
                return mix(_e95, _e97, vec4(_e98));
            }
        }
    }
}

fn chukcut_init_globals() {
    let _e17 = direction;
    center = vec2<f32>(1f, f32(_e17));
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_3: vec4<f32>;

    let _e16 = U;
    progress = _e16.state.x;
    let _e20 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e20);
    let _e23 = dims;
    let _e26 = dims;
    ratio = (f32(_e23.x) / f32(max(_e26.y, 1i)));
    let _e35 = U.params[0];
    direction = (_e35.x > 0.5f);
    let _e42 = U.params[1];
    l_threshold = _e42.x;
    let _e47 = U.params[2];
    above = (_e47.x > 0.5f);
    chukcut_init_globals();
    let _e51 = v_uv_1;
    let _e54 = v_uv_1;
    let _e58 = transition(vec2<f32>(_e51.x, (1f - _e54.y)));
    c_2 = _e58;
    let _e60 = c_2;
    a = clamp(_e60.w, 0f, 1f);
    let _e66 = a;
    if (_e66 > 0.00001f) {
        let _e69 = c_2;
        let _e71 = a;
        let _e73 = (_e69.xyz / vec3(_e71));
        let _e74 = a;
        local_3 = vec4<f32>(_e73.x, _e73.y, _e73.z, _e74);
    } else {
        local_3 = vec4(0f);
    }
    let _e82 = local_3;
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

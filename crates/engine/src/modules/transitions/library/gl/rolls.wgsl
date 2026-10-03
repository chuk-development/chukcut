// Rolls, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Mark Craig
// License: MIT
// mrmcsoftware on github and youtube ( http://www.youtube.com/MrMcSoftware )
// Rolls Transition by Mark Craig (Copyright © 2022)
// type (0-3): Rotate/Roll from which corner
// RotDown: if true rotate old image down, otherwise rotate old image up
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
var<private> type_41: i32;
var<private> RotDown: bool;

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
    var theta: f32;
    var c1_: f32;
    var s1_: f32;
    var iResolution: vec2<f32>;
    var uvi: vec2<f32>;
    var local: f32;
    var local_1: f32;
    var local_2: f32;
    var local_3: f32;
    var uv2_: vec2<f32>;

    uv_5 = uv_4;
    let _e19 = ratio;
    iResolution = vec2<f32>(_e19, 1f);
    let _e24 = type_41;
    if (_e24 == 0i) {
        {
            let _e27 = RotDown;
            if _e27 {
                local = 3.1415927f;
            } else {
                local = -3.1415927f;
            }
            let _e32 = local;
            let _e35 = progress;
            theta = ((_e32 / 2f) * _e35);
            let _e39 = uv_5;
            uvi.x = (1f - _e39.x);
            let _e43 = uv_5;
            uvi.y = _e43.y;
        }
    } else {
        let _e45 = type_41;
        if (_e45 == 1i) {
            {
                let _e48 = RotDown;
                if _e48 {
                    local_1 = 3.1415927f;
                } else {
                    local_1 = -3.1415927f;
                }
                let _e53 = local_1;
                let _e56 = progress;
                theta = ((_e53 / 2f) * _e56);
                let _e58 = uv_5;
                uvi = _e58;
            }
        } else {
            let _e59 = type_41;
            if (_e59 == 2i) {
                {
                    let _e62 = RotDown;
                    if _e62 {
                        local_2 = -3.1415927f;
                    } else {
                        local_2 = 3.1415927f;
                    }
                    let _e67 = local_2;
                    let _e70 = progress;
                    theta = ((_e67 / 2f) * _e70);
                    let _e73 = uv_5;
                    uvi.x = _e73.x;
                    let _e77 = uv_5;
                    uvi.y = (1f - _e77.y);
                }
            } else {
                let _e80 = type_41;
                if (_e80 == 3i) {
                    {
                        let _e83 = RotDown;
                        if _e83 {
                            local_3 = -3.1415927f;
                        } else {
                            local_3 = 3.1415927f;
                        }
                        let _e88 = local_3;
                        let _e91 = progress;
                        theta = ((_e88 / 2f) * _e91);
                        let _e94 = uv_5;
                        uvi = (vec2(1f) - _e94);
                    }
                }
            }
        }
    }
    let _e97 = theta;
    c1_ = cos(_e97);
    let _e99 = theta;
    s1_ = sin(_e99);
    let _e103 = uvi;
    let _e105 = iResolution;
    let _e108 = c1_;
    let _e110 = uvi;
    let _e112 = iResolution;
    let _e115 = s1_;
    uv2_.x = (((_e103.x * _e105.x) * _e108) - ((_e110.y * _e112.y) * _e115));
    let _e119 = uvi;
    let _e121 = iResolution;
    let _e124 = s1_;
    let _e126 = uvi;
    let _e128 = iResolution;
    let _e131 = c1_;
    uv2_.y = (((_e119.x * _e121.x) * _e124) + ((_e126.y * _e128.y) * _e131));
    let _e134 = uv2_;
    let _e138 = uv2_;
    let _e140 = iResolution;
    let _e144 = uv2_;
    let _e149 = uv2_;
    let _e151 = iResolution;
    if ((((_e134.x >= 0f) && (_e138.x <= _e140.x)) && (_e144.y >= 0f)) && (_e149.y <= _e151.y)) {
        {
            let _e155 = uv2_;
            let _e156 = iResolution;
            uv2_ = (_e155 / _e156);
            let _e158 = type_41;
            if (_e158 == 0i) {
                {
                    let _e163 = uv2_;
                    uv2_.x = (1f - _e163.x);
                }
            } else {
                let _e166 = type_41;
                if (_e166 == 2i) {
                    {
                        let _e171 = uv2_;
                        uv2_.y = (1f - _e171.y);
                    }
                } else {
                    let _e174 = type_41;
                    if (_e174 == 3i) {
                        {
                            let _e178 = uv2_;
                            uv2_ = (vec2(1f) - _e178);
                        }
                    }
                }
            }
            let _e181 = uv2_;
            let _e182 = getFromColor(_e181);
            return _e182;
        }
    }
    let _e183 = uv_5;
    let _e184 = getToColor(_e183);
    return _e184;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_4: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    type_41 = i32(_e33.x);
    let _e39 = U.params[1];
    RotDown = (_e39.x > 0.5f);
    chukcut_init_globals();
    let _e43 = v_uv_1;
    let _e46 = v_uv_1;
    let _e50 = transition(vec2<f32>(_e43.x, (1f - _e46.y)));
    c_2 = _e50;
    let _e52 = c_2;
    a = clamp(_e52.w, 0f, 1f);
    let _e58 = a;
    if (_e58 > 0.00001f) {
        let _e61 = c_2;
        let _e63 = a;
        let _e65 = (_e61.xyz / vec3(_e63));
        let _e66 = a;
        local_4 = vec4<f32>(_e65.x, _e65.y, _e65.z, _e66);
    } else {
        local_4 = vec4(0f);
    }
    let _e74 = local_4;
    o_color = _e74;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}

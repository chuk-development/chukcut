// Slides, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Mark Craig
// License: MIT
// mrmcsoftware on github and youtube ( http://www.youtube.com/MrMcSoftware )
// Slides Transition by Mark Craig (Copyright © 2022)
// type: slide to/from which edge, which corner, or center
// In: if true slide new image in, otherwise slide old image out
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
var<private> In: bool;

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
    var uv0_: vec2<f32>;
    var local: f32;
    var rad: f32;
    var xc1_: f32;
    var yc1_: f32;
    var uv2_: vec2<f32>;
    var local_1: vec4<f32>;
    var local_2: vec4<f32>;

    uv_5 = uv_4;
    let _e16 = uv_5;
    uv0_ = _e16;
    let _e18 = In;
    if _e18 {
        let _e19 = progress;
        local = _e19;
    } else {
        let _e21 = progress;
        local = (1f - _e21);
    }
    let _e24 = local;
    rad = _e24;
    let _e28 = type_41;
    if (_e28 == 0i) {
        {
            let _e32 = rad;
            xc1_ = (0.5f - (_e32 / 2f));
            yc1_ = 0f;
        }
    } else {
        let _e37 = type_41;
        if (_e37 == 1i) {
            {
                let _e41 = rad;
                xc1_ = (1f - _e41);
                let _e44 = rad;
                yc1_ = (0.5f - (_e44 / 2f));
            }
        } else {
            let _e48 = type_41;
            if (_e48 == 2i) {
                {
                    let _e52 = rad;
                    xc1_ = (0.5f - (_e52 / 2f));
                    let _e57 = rad;
                    yc1_ = (1f - _e57);
                }
            } else {
                let _e59 = type_41;
                if (_e59 == 3i) {
                    {
                        xc1_ = 0f;
                        let _e64 = rad;
                        yc1_ = (0.5f - (_e64 / 2f));
                    }
                } else {
                    let _e68 = type_41;
                    if (_e68 == 4i) {
                        {
                            let _e72 = rad;
                            xc1_ = (1f - _e72);
                            yc1_ = 0f;
                        }
                    } else {
                        let _e75 = type_41;
                        if (_e75 == 5i) {
                            {
                                let _e79 = rad;
                                xc1_ = (1f - _e79);
                                let _e82 = rad;
                                yc1_ = (1f - _e82);
                            }
                        } else {
                            let _e84 = type_41;
                            if (_e84 == 6i) {
                                {
                                    xc1_ = 0f;
                                    let _e89 = rad;
                                    yc1_ = (1f - _e89);
                                }
                            } else {
                                let _e91 = type_41;
                                if (_e91 == 7i) {
                                    {
                                        xc1_ = 0f;
                                        yc1_ = 0f;
                                    }
                                } else {
                                    let _e96 = type_41;
                                    if (_e96 == 8i) {
                                        {
                                            let _e100 = rad;
                                            xc1_ = (0.5f - (_e100 / 2f));
                                            let _e105 = rad;
                                            yc1_ = (0.5f - (_e105 / 2f));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e111 = uv_5;
    uv_5.y = (1f - _e111.y);
    let _e115 = uv_5;
    let _e117 = xc1_;
    let _e119 = uv_5;
    let _e121 = xc1_;
    let _e122 = rad;
    let _e126 = uv_5;
    let _e128 = yc1_;
    let _e131 = uv_5;
    let _e133 = yc1_;
    let _e134 = rad;
    if ((((_e115.x >= _e117) && (_e119.x <= (_e121 + _e122))) && (_e126.y >= _e128)) && (_e131.y <= (_e133 + _e134))) {
        {
            let _e138 = uv_5;
            let _e140 = xc1_;
            let _e142 = rad;
            let _e145 = uv_5;
            let _e147 = yc1_;
            let _e149 = rad;
            uv2_ = vec2<f32>(((_e138.x - _e140) / _e142), (1f - ((_e145.y - _e147) / _e149)));
            let _e153 = In;
            if _e153 {
                let _e154 = uv2_;
                let _e155 = getToColor(_e154);
                local_1 = _e155;
            } else {
                let _e156 = uv2_;
                let _e157 = getFromColor(_e156);
                local_1 = _e157;
            }
            let _e159 = local_1;
            return _e159;
        }
    }
    let _e160 = In;
    if _e160 {
        let _e161 = uv0_;
        let _e162 = getFromColor(_e161);
        local_2 = _e162;
    } else {
        let _e163 = uv0_;
        let _e164 = getToColor(_e163);
        local_2 = _e164;
    }
    let _e166 = local_2;
    return _e166;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_3: vec4<f32>;

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
    In = (_e39.x > 0.5f);
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
        local_3 = vec4<f32>(_e65.x, _e65.y, _e65.z, _e66);
    } else {
        local_3 = vec4(0f);
    }
    let _e74 = local_3;
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

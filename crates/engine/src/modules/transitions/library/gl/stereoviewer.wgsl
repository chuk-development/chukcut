// StereoViewer, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Ted Schundler
// License: BSD 2 Clause
// Tunable parameters
// How much to zoom (out) for the effect ~ 0.5 - 1.0
// Corner radius as a fraction of the image height
// Free for use and modification by anyone with credit
// Copyright (c) 2016, Theodore K Schundler
// All rights reserved.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
///////////////////////////////////////////////////////////////////////////////
// Stereo Viewer Toy Transition                                              //
//                                                                           //
// Inspired by ViewMaster / Image3D image viewer devices.                    //
// This effect is similar to what you see when you press the device's lever. //
// There is a quick zoom in / out to make the transition 'valid' for GLSL.io //
///////////////////////////////////////////////////////////////////////////////
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

const black: vec4<f32> = vec4<f32>(0f, 0f, 0f, 1f);
const c00_: vec2<f32> = vec2<f32>(0f, 0f);
const c01_: vec2<f32> = vec2<f32>(0f, 1f);
const c11_: vec2<f32> = vec2<f32>(1f, 1f);
const c10_: vec2<f32> = vec2<f32>(1f, 0f);

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
var<private> ratio_6: f32;
var<private> zoom: f32;
var<private> corner_radius: f32;

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

fn in_corner(p: vec2<f32>, corner: vec2<f32>, radius: vec2<f32>) -> bool {
    var p_1: vec2<f32>;
    var corner_1: vec2<f32>;
    var radius_1: vec2<f32>;
    var axis: vec2<f32>;

    p_1 = p;
    corner_1 = corner;
    radius_1 = radius;
    let _e25 = corner_1;
    let _e27 = corner_1;
    axis = ((c11_ - _e25) - _e27);
    let _e30 = p_1;
    let _e31 = corner_1;
    let _e32 = axis;
    let _e33 = radius_1;
    p_1 = (_e30 - (_e31 + (_e32 * _e33)));
    let _e37 = p_1;
    let _e38 = axis;
    let _e39 = radius_1;
    p_1 = (_e37 * (_e38 / _e39));
    let _e42 = p_1;
    let _e46 = p_1;
    let _e52 = p_1;
    let _e56 = p_1;
    let _e63 = p_1;
    let _e64 = p_1;
    return ((((_e42.x > 0f) && (_e46.y > -1f)) || ((_e52.y > 0f) && (_e56.x > -1f))) || (dot(_e63, _e64) < 1f));
}

fn test_rounded_mask(p_2: vec2<f32>, corner_size: vec2<f32>) -> bool {
    var p_3: vec2<f32>;
    var corner_size_1: vec2<f32>;

    p_3 = p_2;
    corner_size_1 = corner_size;
    let _e23 = p_3;
    let _e24 = corner_size_1;
    let _e25 = in_corner(_e23, c00_, _e24);
    let _e26 = p_3;
    let _e27 = corner_size_1;
    let _e28 = in_corner(_e26, c01_, _e27);
    let _e30 = p_3;
    let _e31 = corner_size_1;
    let _e32 = in_corner(_e30, c10_, _e31);
    let _e34 = p_3;
    let _e35 = corner_size_1;
    let _e36 = in_corner(_e34, c11_, _e35);
    return (((_e25 && _e28) && _e32) && _e36);
}

fn screen(a: vec4<f32>, b: vec4<f32>) -> vec4<f32> {
    var a_1: vec4<f32>;
    var b_1: vec4<f32>;

    a_1 = a;
    b_1 = b;
    let _e25 = a_1;
    let _e29 = b_1;
    return (vec4(1f) - ((vec4(1f) - _e25) * (vec4(1f) - _e29)));
}

fn unscreen(c_2: vec4<f32>) -> vec4<f32> {
    var c_3: vec4<f32>;

    c_3 = c_2;
    let _e23 = c_3;
    return (vec4(1f) - sqrt((vec4(1f) - _e23)));
}

fn sample_with_corners_from(p_4: vec2<f32>, corner_size_2: vec2<f32>) -> vec4<f32> {
    var p_5: vec2<f32>;
    var corner_size_3: vec2<f32>;

    p_5 = p_4;
    corner_size_3 = corner_size_2;
    let _e23 = p_5;
    let _e27 = zoom;
    p_5 = (((_e23 - vec2(0.5f)) / vec2(_e27)) + vec2(0.5f));
    let _e33 = p_5;
    let _e34 = corner_size_3;
    let _e35 = test_rounded_mask(_e33, _e34);
    if !(_e35) {
        {
            return black;
        }
    }
    let _e37 = p_5;
    let _e38 = getFromColor(_e37);
    let _e39 = unscreen(_e38);
    return _e39;
}

fn sample_with_corners_to(p_6: vec2<f32>, corner_size_4: vec2<f32>) -> vec4<f32> {
    var p_7: vec2<f32>;
    var corner_size_5: vec2<f32>;

    p_7 = p_6;
    corner_size_5 = corner_size_4;
    let _e23 = p_7;
    let _e27 = zoom;
    p_7 = (((_e23 - vec2(0.5f)) / vec2(_e27)) + vec2(0.5f));
    let _e33 = p_7;
    let _e34 = corner_size_5;
    let _e35 = test_rounded_mask(_e33, _e34);
    if !(_e35) {
        {
            return black;
        }
    }
    let _e37 = p_7;
    let _e38 = getToColor(_e37);
    let _e39 = unscreen(_e38);
    return _e39;
}

fn simple_sample_with_corners_from(p_8: vec2<f32>, corner_size_6: vec2<f32>, zoom_amt: f32) -> vec4<f32> {
    var p_9: vec2<f32>;
    var corner_size_7: vec2<f32>;
    var zoom_amt_1: f32;

    p_9 = p_8;
    corner_size_7 = corner_size_6;
    zoom_amt_1 = zoom_amt;
    let _e25 = p_9;
    let _e30 = zoom_amt_1;
    let _e32 = zoom;
    let _e33 = zoom_amt_1;
    p_9 = (((_e25 - vec2(0.5f)) / vec2(((1f - _e30) + (_e32 * _e33)))) + vec2(0.5f));
    let _e41 = p_9;
    let _e42 = corner_size_7;
    let _e43 = test_rounded_mask(_e41, _e42);
    if !(_e43) {
        {
            return black;
        }
    }
    let _e45 = p_9;
    let _e46 = getFromColor(_e45);
    return _e46;
}

fn simple_sample_with_corners_to(p_10: vec2<f32>, corner_size_8: vec2<f32>, zoom_amt_2: f32) -> vec4<f32> {
    var p_11: vec2<f32>;
    var corner_size_9: vec2<f32>;
    var zoom_amt_3: f32;

    p_11 = p_10;
    corner_size_9 = corner_size_8;
    zoom_amt_3 = zoom_amt_2;
    let _e25 = p_11;
    let _e30 = zoom_amt_3;
    let _e32 = zoom;
    let _e33 = zoom_amt_3;
    p_11 = (((_e25 - vec2(0.5f)) / vec2(((1f - _e30) + (_e32 * _e33)))) + vec2(0.5f));
    let _e41 = p_11;
    let _e42 = corner_size_9;
    let _e43 = test_rounded_mask(_e41, _e42);
    if !(_e43) {
        {
            return black;
        }
    }
    let _e45 = p_11;
    let _e46 = getToColor(_e45);
    return _e46;
}

fn rotate2d(angle: f32, ratio: f32) -> mat3x3<f32> {
    var angle_1: f32;
    var ratio_1: f32;
    var s: f32;
    var c_4: f32;

    angle_1 = angle;
    ratio_1 = ratio;
    let _e23 = angle_1;
    s = sin(_e23);
    let _e26 = angle_1;
    c_4 = cos(_e26);
    let _e29 = c_4;
    let _e30 = s;
    let _e32 = s;
    let _e34 = c_4;
    return mat3x3<f32>(vec3<f32>(_e29, _e30, 0f), vec3<f32>(-(_e32), _e34, 0f), vec3<f32>(0f, 0f, 1f));
}

fn translate2d(x: f32, y: f32) -> mat3x3<f32> {
    var x_1: f32;
    var y_1: f32;

    x_1 = x;
    y_1 = y;
    let _e29 = x_1;
    let _e31 = y_1;
    return mat3x3<f32>(vec3<f32>(1f, 0f, 0f), vec3<f32>(0f, 1f, 0f), vec3<f32>(-(_e29), -(_e31), 1f));
}

fn scale2d(x_2: f32, y_2: f32) -> mat3x3<f32> {
    var x_3: f32;
    var y_3: f32;

    x_3 = x_2;
    y_3 = y_2;
    let _e23 = x_3;
    let _e27 = y_3;
    return mat3x3<f32>(vec3<f32>(_e23, 0f, 0f), vec3<f32>(0f, _e27, 0f), vec3<f32>(0f, 0f, 1f));
}

fn get_cross_rotated(p3_: vec3<f32>, angle_2: f32, corner_size_10: vec2<f32>, ratio_2: f32) -> vec4<f32> {
    var p3_1: vec3<f32>;
    var angle_3: f32;
    var corner_size_11: vec2<f32>;
    var ratio_3: f32;
    var center_and_scale: mat3x3<f32>;
    var unscale_and_uncenter: mat3x3<f32>;
    var slide_left: mat3x3<f32>;
    var slide_right: mat3x3<f32>;
    var rotate: mat3x3<f32>;
    var op_a: mat3x3<f32>;
    var op_b: mat3x3<f32>;
    var a_2: vec4<f32>;
    var b_2: vec4<f32>;

    p3_1 = p3_;
    angle_3 = angle_2;
    corner_size_11 = corner_size_10;
    ratio_3 = ratio_2;
    let _e27 = angle_3;
    let _e28 = angle_3;
    angle_3 = (_e27 * _e28);
    let _e30 = angle_3;
    angle_3 = (_e30 / 2.4f);
    let _e37 = translate2d(-0.5f, -0.5f);
    let _e39 = ratio_3;
    let _e40 = scale2d(1f, _e39);
    center_and_scale = (_e37 * _e40);
    let _e45 = ratio_3;
    let _e47 = scale2d(1f, (1f / _e45));
    let _e50 = translate2d(0.5f, 0.5f);
    unscale_and_uncenter = (_e47 * _e50);
    let _e56 = translate2d(-2f, 0f);
    slide_left = _e56;
    let _e60 = translate2d(2f, 0f);
    slide_right = _e60;
    let _e62 = angle_3;
    let _e63 = ratio_3;
    let _e64 = rotate2d(_e62, _e63);
    rotate = _e64;
    let _e66 = center_and_scale;
    let _e67 = slide_right;
    let _e69 = rotate;
    let _e71 = slide_left;
    let _e73 = unscale_and_uncenter;
    op_a = ((((_e66 * _e67) * _e69) * _e71) * _e73);
    let _e76 = center_and_scale;
    let _e77 = slide_left;
    let _e79 = rotate;
    let _e81 = slide_right;
    let _e83 = unscale_and_uncenter;
    op_b = ((((_e76 * _e77) * _e79) * _e81) * _e83);
    let _e86 = op_a;
    let _e87 = p3_1;
    let _e90 = corner_size_11;
    let _e91 = sample_with_corners_from((_e86 * _e87).xy, _e90);
    a_2 = _e91;
    let _e93 = op_b;
    let _e94 = p3_1;
    let _e97 = corner_size_11;
    let _e98 = sample_with_corners_from((_e93 * _e94).xy, _e97);
    b_2 = _e98;
    let _e100 = a_2;
    let _e101 = b_2;
    let _e102 = screen(_e100, _e101);
    return _e102;
}

fn get_cross_masked(p3_2: vec3<f32>, angle_4: f32, corner_size_12: vec2<f32>, ratio_4: f32) -> vec4<f32> {
    var p3_3: vec3<f32>;
    var angle_5: f32;
    var corner_size_13: vec2<f32>;
    var ratio_5: f32;
    var img: vec4<f32>;
    var center_and_scale_1: mat3x3<f32>;
    var unscale_and_uncenter_1: mat3x3<f32>;
    var slide_left_1: mat3x3<f32>;
    var slide_right_1: mat3x3<f32>;
    var rotate_1: mat3x3<f32>;
    var op_a_1: mat3x3<f32>;
    var op_b_1: mat3x3<f32>;
    var mask_a: bool;
    var mask_b: bool;
    var local: vec4<f32>;
    var local_1: vec4<f32>;

    p3_3 = p3_2;
    angle_5 = angle_4;
    corner_size_13 = corner_size_12;
    ratio_5 = ratio_4;
    let _e28 = angle_5;
    angle_5 = (1f - _e28);
    let _e30 = angle_5;
    let _e31 = angle_5;
    angle_5 = (_e30 * _e31);
    let _e33 = angle_5;
    angle_5 = (_e33 / 2.4f);
    let _e41 = translate2d(-0.5f, -0.5f);
    let _e43 = ratio_5;
    let _e44 = scale2d(1f, _e43);
    center_and_scale_1 = (_e41 * _e44);
    let _e48 = zoom;
    let _e51 = zoom;
    let _e52 = ratio_5;
    let _e55 = scale2d((1f / _e48), (1f / (_e51 * _e52)));
    let _e58 = translate2d(0.5f, 0.5f);
    unscale_and_uncenter_1 = (_e55 * _e58);
    let _e64 = translate2d(-2f, 0f);
    slide_left_1 = _e64;
    let _e68 = translate2d(2f, 0f);
    slide_right_1 = _e68;
    let _e70 = angle_5;
    let _e71 = ratio_5;
    let _e72 = rotate2d(_e70, _e71);
    rotate_1 = _e72;
    let _e74 = center_and_scale_1;
    let _e75 = slide_right_1;
    let _e77 = rotate_1;
    let _e79 = slide_left_1;
    let _e81 = unscale_and_uncenter_1;
    op_a_1 = ((((_e74 * _e75) * _e77) * _e79) * _e81);
    let _e84 = center_and_scale_1;
    let _e85 = slide_left_1;
    let _e87 = rotate_1;
    let _e89 = slide_right_1;
    let _e91 = unscale_and_uncenter_1;
    op_b_1 = ((((_e84 * _e85) * _e87) * _e89) * _e91);
    let _e94 = op_a_1;
    let _e95 = p3_3;
    let _e98 = corner_size_13;
    let _e99 = test_rounded_mask((_e94 * _e95).xy, _e98);
    mask_a = _e99;
    let _e101 = op_b_1;
    let _e102 = p3_3;
    let _e105 = corner_size_13;
    let _e106 = test_rounded_mask((_e101 * _e102).xy, _e105);
    mask_b = _e106;
    let _e108 = mask_a;
    let _e109 = mask_b;
    if (_e108 || _e109) {
        {
            let _e111 = p3_3;
            let _e113 = corner_size_13;
            let _e114 = sample_with_corners_to(_e111.xy, _e113);
            img = _e114;
            let _e115 = mask_a;
            if _e115 {
                let _e116 = img;
                local = _e116;
            } else {
                local = black;
            }
            let _e118 = local;
            let _e119 = mask_b;
            if _e119 {
                let _e120 = img;
                local_1 = _e120;
            } else {
                local_1 = black;
            }
            let _e122 = local_1;
            let _e123 = screen(_e118, _e122);
            return _e123;
        }
    } else {
        {
            return black;
        }
    }
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var a_3: f32;
    var p_12: vec2<f32>;
    var p3_4: vec3<f32>;
    var corner_size_14: vec2<f32>;

    uv_5 = uv_4;
    let _e22 = uv_5;
    p_12 = (_e22.xy / vec2(1f));
    let _e29 = p_12;
    let _e30 = _e29.xy;
    p3_4 = vec3<f32>(_e30.x, _e30.y, 1f);
    let _e36 = corner_radius;
    let _e37 = ratio_6;
    let _e39 = corner_radius;
    corner_size_14 = vec2<f32>((_e36 / _e37), _e39);
    let _e42 = progress;
    if (_e42 <= 0f) {
        {
            let _e45 = p_12;
            let _e46 = getFromColor(_e45);
            return _e46;
        }
    } else {
        let _e47 = progress;
        if (_e47 < 0.1f) {
            {
                let _e50 = progress;
                a_3 = (_e50 / 0.1f);
                let _e53 = p_12;
                let _e54 = corner_size_14;
                let _e55 = a_3;
                let _e57 = a_3;
                let _e58 = simple_sample_with_corners_from(_e53, (_e54 * _e55), _e57);
                return _e58;
            }
        } else {
            let _e59 = progress;
            if (_e59 < 0.48f) {
                {
                    let _e62 = progress;
                    a_3 = ((_e62 - 0.1f) / 0.38f);
                    let _e67 = p3_4;
                    let _e68 = a_3;
                    let _e69 = corner_size_14;
                    let _e70 = ratio_6;
                    let _e71 = get_cross_rotated(_e67, _e68, _e69, _e70);
                    return _e71;
                }
            } else {
                let _e72 = progress;
                if (_e72 < 0.9f) {
                    {
                        let _e75 = p3_4;
                        let _e76 = progress;
                        let _e81 = corner_size_14;
                        let _e82 = ratio_6;
                        let _e83 = get_cross_masked(_e75, ((_e76 - 0.52f) / 0.38f), _e81, _e82);
                        return _e83;
                    }
                } else {
                    let _e84 = progress;
                    if (_e84 < 1f) {
                        {
                            let _e88 = progress;
                            a_3 = ((1f - _e88) / 0.1f);
                            let _e92 = p_12;
                            let _e93 = corner_size_14;
                            let _e94 = a_3;
                            let _e96 = a_3;
                            let _e97 = simple_sample_with_corners_to(_e92, (_e93 * _e94), _e96);
                            return _e97;
                        }
                    } else {
                        {
                            let _e98 = p_12;
                            let _e99 = getToColor(_e98);
                            return _e99;
                        }
                    }
                }
            }
        }
    }
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_5: vec4<f32>;
    var a_4: f32;
    var local_2: vec4<f32>;

    let _e19 = U;
    progress = _e19.state.x;
    let _e23 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e23);
    let _e26 = dims;
    let _e29 = dims;
    ratio_6 = (f32(_e26.x) / f32(max(_e29.y, 1i)));
    let _e38 = U.params[0];
    zoom = _e38.x;
    let _e43 = U.params[1];
    corner_radius = _e43.x;
    chukcut_init_globals();
    let _e45 = v_uv_1;
    let _e48 = v_uv_1;
    let _e52 = transition(vec2<f32>(_e45.x, (1f - _e48.y)));
    c_5 = _e52;
    let _e54 = c_5;
    a_4 = clamp(_e54.w, 0f, 1f);
    let _e60 = a_4;
    if (_e60 > 0.00001f) {
        let _e63 = c_5;
        let _e65 = a_4;
        let _e67 = (_e63.xyz / vec3(_e65));
        let _e68 = a_4;
        local_2 = vec4<f32>(_e67.x, _e67.y, _e67.z, _e68);
    } else {
        local_2 = vec4(0f);
    }
    let _e76 = local_2;
    o_color = _e76;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e36 = o_color;
    return FragmentOutput(_e36);
}

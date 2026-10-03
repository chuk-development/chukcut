// InvertedPageCurl, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Hewlett-Packard
// License: BSD 3 Clause
// Adapted by Sergey Kosarevsky from:
// http://rectalogic.github.io/webvfx/examples_2transition-shader-pagecurl_8html-example.html
//
// Copyright (c) 2010 Hewlett-Packard Development Company, L.P. All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are
// met:
//
// * Redistributions of source code must retain the above copyright
// notice, this list of conditions and the following disclaimer.
// * Redistributions in binary form must reproduce the above
// copyright notice, this list of conditions and the following disclaimer
// in the documentation and/or other materials provided with the
// distribution.
// * Neither the name of Hewlett-Packard nor the names of its
// contributors may be used to endorse or promote products derived from
// this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
// "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
// LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
// A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
// OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
// LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
// DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
// THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
// in vec2 texCoord;
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

const MIN_AMOUNT: f32 = -0.16f;
const MAX_AMOUNT: f32 = 1.5f;
const PI: f32 = 3.1415927f;
const scale: f32 = 512f;
const sharpness: f32 = 3f;
const cylinderRadius: f32 = 0.15915494f;

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
var<private> amount: f32;
var<private> cylinderCenter: f32;
var<private> cylinderAngle: f32;

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

fn hitPoint(hitAngle: f32, yc: f32, point: vec3<f32>, rrotation: mat3x3<f32>) -> vec3<f32> {
    var hitAngle_1: f32;
    var yc_1: f32;
    var point_1: vec3<f32>;
    var rrotation_1: mat3x3<f32>;
    var hitPoint_1: f32;

    hitAngle_1 = hitAngle;
    yc_1 = yc;
    point_1 = point;
    rrotation_1 = rrotation;
    let _e29 = hitAngle_1;
    hitPoint_1 = (_e29 / 6.2831855f);
    let _e36 = hitPoint_1;
    point_1.y = _e36;
    let _e37 = rrotation_1;
    let _e38 = point_1;
    return (_e37 * _e38);
}

fn antiAlias(color1_: vec4<f32>, color2_: vec4<f32>, distanc: f32) -> vec4<f32> {
    var color1_1: vec4<f32>;
    var color2_1: vec4<f32>;
    var distanc_1: f32;
    var dd: f32;

    color1_1 = color1_;
    color2_1 = color2_;
    distanc_1 = distanc;
    let _e27 = distanc_1;
    distanc_1 = (_e27 * scale);
    let _e29 = distanc_1;
    if (_e29 < 0f) {
        let _e32 = color2_1;
        return _e32;
    }
    let _e33 = distanc_1;
    if (_e33 > 2f) {
        let _e36 = color1_1;
        return _e36;
    }
    let _e38 = distanc_1;
    dd = pow((1f - (_e38 / 2f)), sharpness);
    let _e44 = color2_1;
    let _e45 = color1_1;
    let _e47 = dd;
    let _e49 = color1_1;
    return (((_e44 - _e45) * _e47) + _e49);
}

fn distanceToEdge(point_2: vec3<f32>) -> f32 {
    var point_3: vec3<f32>;
    var local: f32;
    var dx: f32;
    var local_1: f32;
    var dy: f32;

    point_3 = point_2;
    let _e23 = point_3;
    if (_e23.x > 0.5f) {
        let _e28 = point_3;
        local = (1f - _e28.x);
    } else {
        let _e31 = point_3;
        local = _e31.x;
    }
    let _e34 = local;
    dx = abs(_e34);
    let _e37 = point_3;
    if (_e37.y > 0.5f) {
        let _e42 = point_3;
        local_1 = (1f - _e42.y);
    } else {
        let _e45 = point_3;
        local_1 = _e45.y;
    }
    let _e48 = local_1;
    dy = abs(_e48);
    let _e51 = point_3;
    if (_e51.x < 0f) {
        let _e55 = point_3;
        dx = -(_e55.x);
    }
    let _e58 = point_3;
    if (_e58.x > 1f) {
        let _e62 = point_3;
        dx = (_e62.x - 1f);
    }
    let _e66 = point_3;
    if (_e66.y < 0f) {
        let _e70 = point_3;
        dy = -(_e70.y);
    }
    let _e73 = point_3;
    if (_e73.y > 1f) {
        let _e77 = point_3;
        dy = (_e77.y - 1f);
    }
    let _e81 = point_3;
    let _e85 = point_3;
    let _e90 = point_3;
    let _e94 = point_3;
    if (((_e81.x < 0f) || (_e85.x > 1f)) && ((_e90.y < 0f) || (_e94.y > 1f))) {
        let _e100 = dx;
        let _e101 = dx;
        let _e103 = dy;
        let _e104 = dy;
        return sqrt(((_e100 * _e101) + (_e103 * _e104)));
    }
    let _e108 = dx;
    let _e109 = dy;
    return min(_e108, _e109);
}

fn seeThrough(yc_2: f32, p: vec2<f32>, rotation: mat3x3<f32>, rrotation_2: mat3x3<f32>) -> vec4<f32> {
    var yc_3: f32;
    var p_1: vec2<f32>;
    var rotation_1: mat3x3<f32>;
    var rrotation_3: mat3x3<f32>;
    var hitAngle_2: f32;
    var point_4: vec3<f32>;
    var color: vec4<f32>;
    var tcolor: vec4<f32> = vec4(0f);

    yc_3 = yc_2;
    p_1 = p;
    rotation_1 = rotation;
    rrotation_3 = rrotation_2;
    let _e29 = yc_3;
    let _e36 = cylinderAngle;
    hitAngle_2 = (PI - (acos(clamp((_e29 / cylinderRadius), -1f, 1f)) - _e36));
    let _e40 = hitAngle_2;
    let _e41 = yc_3;
    let _e42 = rotation_1;
    let _e43 = p_1;
    let _e49 = rrotation_3;
    let _e50 = hitPoint(_e40, _e41, (_e42 * vec3<f32>(_e43.x, _e43.y, 1f)), _e49);
    point_4 = _e50;
    let _e52 = yc_3;
    let _e55 = point_4;
    let _e59 = point_4;
    let _e64 = point_4;
    let _e69 = point_4;
    if ((_e52 <= 0f) && ((((_e55.x < 0f) || (_e59.y < 0f)) || (_e64.x > 1f)) || (_e69.y > 1f))) {
        {
            let _e75 = p_1;
            let _e76 = getToColor(_e75);
            return _e76;
        }
    }
    let _e77 = yc_3;
    if (_e77 > 0f) {
        let _e80 = p_1;
        let _e81 = getFromColor(_e80);
        return _e81;
    }
    let _e82 = point_4;
    let _e84 = getFromColor(_e82.xy);
    color = _e84;
    let _e89 = color;
    let _e90 = tcolor;
    let _e91 = point_4;
    let _e92 = distanceToEdge(_e91);
    let _e93 = antiAlias(_e89, _e90, _e92);
    return _e93;
}

fn seeThroughWithShadow(yc_4: f32, p_2: vec2<f32>, point_5: vec3<f32>, rotation_2: mat3x3<f32>, rrotation_4: mat3x3<f32>) -> vec4<f32> {
    var yc_5: f32;
    var p_3: vec2<f32>;
    var point_6: vec3<f32>;
    var rotation_3: mat3x3<f32>;
    var rrotation_5: mat3x3<f32>;
    var shadow: f32;
    var shadowColor: vec4<f32>;

    yc_5 = yc_4;
    p_3 = p_2;
    point_6 = point_5;
    rotation_3 = rotation_2;
    rrotation_5 = rrotation_4;
    let _e31 = point_6;
    let _e32 = distanceToEdge(_e31);
    shadow = (_e32 * 30f);
    let _e37 = shadow;
    shadow = ((1f - _e37) / 3f);
    let _e41 = shadow;
    if (_e41 < 0f) {
        shadow = 0f;
    } else {
        let _e45 = shadow;
        let _e46 = amount;
        shadow = (_e45 * _e46);
    }
    let _e48 = yc_5;
    let _e49 = p_3;
    let _e50 = rotation_3;
    let _e51 = rrotation_5;
    let _e52 = seeThrough(_e48, _e49, _e50, _e51);
    shadowColor = _e52;
    let _e55 = shadowColor;
    let _e57 = shadow;
    shadowColor.x = (_e55.x - _e57);
    let _e60 = shadowColor;
    let _e62 = shadow;
    shadowColor.y = (_e60.y - _e62);
    let _e65 = shadowColor;
    let _e67 = shadow;
    shadowColor.z = (_e65.z - _e67);
    let _e69 = shadowColor;
    return _e69;
}

fn backside(yc_6: f32, point_7: vec3<f32>) -> vec4<f32> {
    var yc_7: f32;
    var point_8: vec3<f32>;
    var color_1: vec4<f32>;
    var gray: f32;

    yc_7 = yc_6;
    point_8 = point_7;
    let _e25 = point_8;
    let _e27 = getFromColor(_e25.xy);
    color_1 = _e27;
    let _e29 = color_1;
    let _e31 = color_1;
    let _e34 = color_1;
    gray = (((_e29.x + _e31.z) + _e34.y) / 15f);
    let _e40 = gray;
    let _e46 = yc_7;
    gray = (_e40 + (0.8f * ((pow(max(0f, (1f - abs((_e46 / cylinderRadius)))), 0.2f) / 2f) + 0.5f)));
    let _e63 = color_1;
    let _e65 = gray;
    let _e66 = vec3(_e65);
    color_1.x = _e66.x;
    color_1.y = _e66.y;
    color_1.z = _e66.z;
    let _e73 = color_1;
    return _e73;
}

fn behindSurface(p_4: vec2<f32>, yc_8: f32, point_9: vec3<f32>, rrotation_6: mat3x3<f32>) -> vec4<f32> {
    var p_5: vec2<f32>;
    var yc_9: f32;
    var point_10: vec3<f32>;
    var rrotation_7: mat3x3<f32>;
    var local_2: f32;
    var safeAmount: f32;
    var shado: f32;
    var hitAngle_3: f32;
    var dx_1: f32;
    var dy_1: f32;
    var nyc: f32;

    p_5 = p_4;
    yc_9 = yc_8;
    point_10 = point_9;
    rrotation_7 = rrotation_6;
    let _e29 = amount;
    if (_e29 >= 0f) {
        let _e32 = amount;
        local_2 = max(_e32, 0.0001f);
    } else {
        let _e35 = amount;
        local_2 = min(_e35, -0.0001f);
    }
    let _e40 = local_2;
    safeAmount = _e40;
    let _e45 = yc_9;
    let _e47 = safeAmount;
    shado = ((1f - (((-0.15915494f - _e45) / _e47) * 7f)) / 6f);
    let _e55 = shado;
    let _e57 = point_10;
    shado = (_e55 * (1f - abs((_e57.x - 0.5f))));
    let _e68 = yc_9;
    yc_9 = (-0.31830987f - _e68);
    let _e70 = yc_9;
    let _e77 = cylinderAngle;
    hitAngle_3 = ((acos(clamp((_e70 / cylinderRadius), -1f, 1f)) + _e77) - PI);
    let _e81 = hitAngle_3;
    let _e82 = yc_9;
    let _e83 = point_10;
    let _e84 = rrotation_7;
    let _e85 = hitPoint(_e81, _e82, _e83, _e84);
    point_10 = _e85;
    let _e86 = yc_9;
    let _e89 = point_10;
    let _e94 = point_10;
    let _e99 = point_10;
    let _e104 = point_10;
    let _e109 = hitAngle_3;
    let _e111 = amount;
    if ((((((_e86 < 0f) && (_e89.x >= 0f)) && (_e94.y >= 0f)) && (_e99.x <= 1f)) && (_e104.y <= 1f)) && ((_e109 < PI) || (_e111 > 0.5f))) {
        {
            let _e116 = point_10;
            dx_1 = (_e116.x - 0.5f);
            let _e121 = point_10;
            dy_1 = (_e121.y - 0.5f);
            let _e127 = dx_1;
            let _e128 = dx_1;
            let _e130 = dy_1;
            let _e131 = dy_1;
            shado = (1f - (sqrt(((_e127 * _e128) + (_e130 * _e131))) / 0.71f));
            let _e140 = yc_9;
            nyc = (-(_e140) / cylinderRadius);
            let _e144 = shado;
            let _e145 = nyc;
            let _e146 = nyc;
            let _e148 = nyc;
            shado = (_e144 * ((_e145 * _e146) * _e148));
            let _e151 = shado;
            shado = (_e151 * 0.5f);
        }
    } else {
        {
            shado = 0f;
        }
    }
    let _e155 = p_5;
    let _e156 = getToColor(_e155);
    let _e158 = shado;
    let _e160 = (_e156.xyz - vec3(_e158));
    return vec4<f32>(_e160.x, _e160.y, _e160.z, 1f);
}

fn transition(p_6: vec2<f32>) -> vec4<f32> {
    var p_7: vec2<f32>;
    var angle: f32 = 1.7453293f;
    var c_2: f32;
    var s: f32;
    var rotation_4: mat3x3<f32>;
    var rrotation_8: mat3x3<f32>;
    var point_11: vec3<f32>;
    var yc_10: f32;
    var hitAngle_4: f32;
    var hitAngleMod: f32;
    var color_2: vec4<f32>;
    var otherColor: vec4<f32>;
    var dx2_: f32;
    var dy2_: f32;
    var shado_1: f32;
    var nyc2_: f32;
    var cl: vec4<f32>;
    var dist: f32;

    p_7 = p_6;
    let _e23 = progress;
    amount = ((_e23 * 1.66f) + MIN_AMOUNT);
    let _e29 = amount;
    cylinderCenter = _e29;
    let _e33 = amount;
    cylinderAngle = (6.2831855f * _e33);
    let _e41 = angle;
    c_2 = cos(-(_e41));
    let _e45 = angle;
    s = sin(-(_e45));
    let _e49 = c_2;
    let _e50 = s;
    let _e52 = s;
    let _e54 = c_2;
    rotation_4 = mat3x3<f32>(vec3<f32>(_e49, _e50, 0f), vec3<f32>(-(_e52), _e54, 0f), vec3<f32>(-0.801f, 0.89f, 1f));
    let _e68 = angle;
    c_2 = cos(_e68);
    let _e70 = angle;
    s = sin(_e70);
    let _e72 = c_2;
    let _e73 = s;
    let _e75 = s;
    let _e77 = c_2;
    rrotation_8 = mat3x3<f32>(vec3<f32>(_e72, _e73, 0f), vec3<f32>(-(_e75), _e77, 0f), vec3<f32>(0.985f, 0.985f, 1f));
    let _e90 = rotation_4;
    let _e91 = p_7;
    point_11 = (_e90 * vec3<f32>(_e91.x, _e91.y, 1f));
    let _e98 = point_11;
    let _e100 = cylinderCenter;
    yc_10 = (_e98.y - _e100);
    let _e103 = yc_10;
    if (_e103 < -0.15915494f) {
        {
            let _e107 = p_7;
            let _e108 = yc_10;
            let _e109 = point_11;
            let _e110 = rrotation_8;
            let _e111 = behindSurface(_e107, _e108, _e109, _e110);
            return _e111;
        }
    }
    let _e112 = yc_10;
    if (_e112 > cylinderRadius) {
        {
            let _e114 = p_7;
            let _e115 = getFromColor(_e114);
            return _e115;
        }
    }
    let _e116 = yc_10;
    let _e123 = cylinderAngle;
    hitAngle_4 = ((acos(clamp((_e116 / cylinderRadius), -1f, 1f)) + _e123) - PI);
    let _e127 = hitAngle_4;
    hitAngleMod = (_e127 - (floor((_e127 / 6.2831855f)) * 6.2831855f));
    let _e136 = hitAngleMod;
    let _e138 = amount;
    let _e142 = hitAngleMod;
    let _e147 = amount;
    if (((_e136 > PI) && (_e138 < 0.5f)) || ((_e142 > 1.5707964f) && (_e147 < 0f))) {
        {
            let _e152 = yc_10;
            let _e153 = p_7;
            let _e154 = rotation_4;
            let _e155 = rrotation_8;
            let _e156 = seeThrough(_e152, _e153, _e154, _e155);
            return _e156;
        }
    }
    let _e157 = hitAngle_4;
    let _e158 = yc_10;
    let _e159 = point_11;
    let _e160 = rrotation_8;
    let _e161 = hitPoint(_e157, _e158, _e159, _e160);
    point_11 = _e161;
    let _e162 = point_11;
    let _e166 = point_11;
    let _e171 = point_11;
    let _e176 = point_11;
    if ((((_e162.x < 0f) || (_e166.y < 0f)) || (_e171.x > 1f)) || (_e176.y > 1f)) {
        {
            let _e181 = yc_10;
            let _e182 = p_7;
            let _e183 = point_11;
            let _e184 = rotation_4;
            let _e185 = rrotation_8;
            let _e186 = seeThroughWithShadow(_e181, _e182, _e183, _e184, _e185);
            return _e186;
        }
    }
    let _e187 = yc_10;
    let _e188 = point_11;
    let _e189 = backside(_e187, _e188);
    color_2 = _e189;
    let _e192 = yc_10;
    if (_e192 < 0f) {
        {
            let _e195 = point_11;
            dx2_ = (_e195.x - 0.5f);
            let _e200 = point_11;
            dy2_ = (_e200.y - 0.5f);
            let _e206 = dx2_;
            let _e207 = dx2_;
            let _e209 = dy2_;
            let _e210 = dy2_;
            shado_1 = (1f - (sqrt(((_e206 * _e207) + (_e209 * _e210))) / 0.71f));
            let _e218 = yc_10;
            nyc2_ = (-(_e218) / cylinderRadius);
            let _e222 = shado_1;
            let _e223 = nyc2_;
            let _e224 = nyc2_;
            let _e226 = nyc2_;
            shado_1 = (_e222 * ((_e223 * _e224) * _e226));
            let _e229 = shado_1;
            shado_1 = (_e229 * 0.5f);
            let _e235 = shado_1;
            otherColor = vec4<f32>(0f, 0f, 0f, _e235);
        }
    } else {
        {
            let _e237 = p_7;
            let _e238 = getFromColor(_e237);
            otherColor = _e238;
        }
    }
    let _e239 = color_2;
    let _e240 = otherColor;
    let _e241 = yc_10;
    let _e244 = antiAlias(_e239, _e240, (cylinderRadius - abs(_e241)));
    color_2 = _e244;
    let _e245 = yc_10;
    let _e246 = p_7;
    let _e247 = point_11;
    let _e248 = rotation_4;
    let _e249 = rrotation_8;
    let _e250 = seeThroughWithShadow(_e245, _e246, _e247, _e248, _e249);
    cl = _e250;
    let _e252 = point_11;
    let _e253 = distanceToEdge(_e252);
    dist = _e253;
    let _e255 = color_2;
    let _e256 = cl;
    let _e257 = dist;
    let _e258 = antiAlias(_e255, _e256, _e257);
    return _e258;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
    var a: f32;
    var local_3: vec4<f32>;

    let _e21 = U;
    progress = _e21.state.x;
    let _e25 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e25);
    let _e28 = dims;
    let _e31 = dims;
    ratio = (f32(_e28.x) / f32(max(_e31.y, 1i)));
    chukcut_init_globals();
    let _e37 = v_uv_1;
    let _e40 = v_uv_1;
    let _e44 = transition(vec2<f32>(_e37.x, (1f - _e40.y)));
    c_3 = _e44;
    let _e46 = c_3;
    a = clamp(_e46.w, 0f, 1f);
    let _e52 = a;
    if (_e52 > 0.00001f) {
        let _e55 = c_3;
        let _e57 = a;
        let _e59 = (_e55.xyz / vec3(_e57));
        let _e60 = a;
        local_3 = vec4<f32>(_e59.x, _e59.y, _e59.z, _e60);
    } else {
        local_3 = vec4(0f);
    }
    let _e68 = local_3;
    o_color = _e68;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e40 = o_color;
    return FragmentOutput(_e40);
}

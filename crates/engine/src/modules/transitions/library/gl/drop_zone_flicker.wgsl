// Drop_Zone_Flicker, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: bread
// License: MIT
// Drop_Zone_Flicker.glsl
// gl-transitions compatible: progress, ratio, getFromColor, getToColor
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
var<private> frameRate: f32;
var<private> rgbOffset: f32;
var<private> blockAmount: f32;
var<private> ghostAmount: f32;
var<private> redCyan: f32;
var<private> scanline: f32;

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

fn sat(x: f32) -> f32 {
    var x_1: f32;

    x_1 = x;
    let _e20 = x_1;
    return clamp(_e20, 0f, 1f);
}

fn hash12_(p: vec2<f32>) -> f32 {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e20 = p_1;
    p_1 = fract((_e20 * vec2<f32>(443.8975f, 397.2973f)));
    let _e26 = p_1;
    let _e27 = p_1;
    let _e28 = p_1;
    p_1 = (_e26 + vec2(dot(_e27, (_e28.yx + vec2(19.19f)))));
    let _e36 = p_1;
    let _e38 = p_1;
    return fract((_e36.x * _e38.y));
}

fn safeUv(uv_4: vec2<f32>) -> vec2<f32> {
    var uv_5: vec2<f32>;

    uv_5 = uv_4;
    let _e20 = uv_5;
    return clamp(_e20, vec2(0.001f), vec2(0.999f));
}

fn frameReveal(f: f32) -> f32 {
    var f_1: f32;

    f_1 = f;
    let _e20 = f_1;
    if (_e20 < 0.5f) {
        return 0f;
    }
    let _e24 = f_1;
    if (_e24 < 1.5f) {
        return 0.28f;
    }
    let _e28 = f_1;
    if (_e28 < 2.5f) {
        return 0.43f;
    }
    let _e32 = f_1;
    if (_e32 < 3.5f) {
        return 0.46f;
    }
    let _e36 = f_1;
    if (_e36 < 4.5f) {
        return 0.38f;
    }
    let _e40 = f_1;
    if (_e40 < 5.5f) {
        return 0.48f;
    }
    let _e44 = f_1;
    if (_e44 < 6.5f) {
        return 0.26f;
    }
    let _e48 = f_1;
    if (_e48 < 7.5f) {
        return 0.1f;
    }
    let _e52 = f_1;
    if (_e52 < 8.5f) {
        return 0f;
    }
    let _e56 = f_1;
    if (_e56 < 9.5f) {
        return 0f;
    }
    let _e60 = f_1;
    if (_e60 < 10.5f) {
        return 0.3f;
    }
    let _e64 = f_1;
    if (_e64 < 11.5f) {
        return 0.58f;
    }
    let _e68 = f_1;
    if (_e68 < 12.5f) {
        return 1f;
    }
    let _e72 = f_1;
    if (_e72 < 13.5f) {
        return 0.7f;
    }
    let _e76 = f_1;
    if (_e76 < 14.5f) {
        return 0.42f;
    }
    let _e80 = f_1;
    if (_e80 < 15.5f) {
        return 0.55f;
    }
    let _e84 = f_1;
    if (_e84 < 16.5f) {
        return 0.72f;
    }
    let _e88 = f_1;
    if (_e88 < 17.5f) {
        return 0.88f;
    }
    let _e92 = f_1;
    if (_e92 < 18.5f) {
        return 0.34f;
    }
    let _e96 = f_1;
    if (_e96 < 19.5f) {
        return 0.48f;
    }
    let _e100 = f_1;
    if (_e100 < 20.5f) {
        return 0.56f;
    }
    let _e104 = f_1;
    if (_e104 < 21.5f) {
        return 0.76f;
    }
    let _e108 = f_1;
    if (_e108 < 22.5f) {
        return 0.93f;
    }
    return 1f;
}

fn frameGlitch(f_2: f32) -> f32 {
    var f_3: f32;

    f_3 = f_2;
    let _e20 = f_3;
    if (_e20 < 0.5f) {
        return 0f;
    }
    let _e24 = f_3;
    if (_e24 < 6.5f) {
        return 0.92f;
    }
    let _e28 = f_3;
    if (_e28 < 10.5f) {
        return 0.38f;
    }
    let _e32 = f_3;
    if (_e32 < 11.5f) {
        return 0.78f;
    }
    let _e36 = f_3;
    if (_e36 < 12.5f) {
        return 0.12f;
    }
    let _e40 = f_3;
    if (_e40 < 17.5f) {
        return 0.74f;
    }
    let _e44 = f_3;
    if (_e44 < 22.5f) {
        return 0.88f;
    }
    let _e48 = f_3;
    if (_e48 < 23.5f) {
        return 0.28f;
    }
    return 0f;
}

fn chromaFrom(uv_6: vec2<f32>, amt: f32) -> vec4<f32> {
    var uv_7: vec2<f32>;
    var amt_1: f32;
    var o: vec2<f32>;

    uv_7 = uv_6;
    amt_1 = amt;
    let _e22 = amt_1;
    o = vec2<f32>(_e22, 0f);
    let _e26 = uv_7;
    let _e27 = o;
    let _e29 = safeUv((_e26 + _e27));
    let _e30 = getFromColor(_e29);
    let _e32 = uv_7;
    let _e33 = safeUv(_e32);
    let _e34 = getFromColor(_e33);
    let _e36 = uv_7;
    let _e37 = o;
    let _e39 = safeUv((_e36 - _e37));
    let _e40 = getFromColor(_e39);
    return vec4<f32>(_e30.x, _e34.y, _e40.z, 1f);
}

fn chromaTo(uv_8: vec2<f32>, amt_2: f32) -> vec4<f32> {
    var uv_9: vec2<f32>;
    var amt_3: f32;
    var o_1: vec2<f32>;

    uv_9 = uv_8;
    amt_3 = amt_2;
    let _e22 = amt_3;
    o_1 = vec2<f32>(_e22, 0f);
    let _e26 = uv_9;
    let _e27 = o_1;
    let _e29 = safeUv((_e26 - _e27));
    let _e30 = getToColor(_e29);
    let _e32 = uv_9;
    let _e33 = safeUv(_e32);
    let _e34 = getToColor(_e33);
    let _e36 = uv_9;
    let _e37 = o_1;
    let _e39 = safeUv((_e36 + _e37));
    let _e40 = getToColor(_e39);
    return vec4<f32>(_e30.x, _e34.y, _e40.z, 1f);
}

fn blockMask(uv_10: vec2<f32>, f_4: f32) -> f32 {
    var uv_11: vec2<f32>;
    var f_5: f32;
    var big: vec2<f32>;
    var small: vec2<f32>;
    var wide: f32;
    var chunks: f32;
    var verticalCut: f32;

    uv_11 = uv_10;
    f_5 = f_4;
    let _e22 = uv_11;
    big = floor((_e22 * vec2<f32>(4f, 2f)));
    let _e29 = uv_11;
    small = floor((_e29 * vec2<f32>(8f, 4f)));
    let _e37 = big;
    let _e39 = f_5;
    let _e43 = hash12_(vec2<f32>(_e37.x, (_e39 * 1.37f)));
    wide = step(0.48f, _e43);
    let _e47 = small;
    let _e48 = f_5;
    let _e51 = f_5;
    let _e56 = hash12_((_e47 + vec2<f32>((_e48 * 2.11f), (_e51 * 0.73f))));
    chunks = step(0.56f, _e56);
    let _e62 = uv_11;
    let _e66 = f_5;
    let _e69 = hash12_(vec2<f32>(_e66, 4.7f));
    verticalCut = smoothstep(-0.035f, 0.035f, (_e62.x - mix(0.18f, 0.78f, _e69)));
    let _e74 = wide;
    let _e75 = chunks;
    let _e80 = verticalCut;
    let _e84 = sat(((mix(_e74, _e75, 0.38f) * 0.72f) + (_e80 * 0.28f)));
    return _e84;
}

fn transition(uv_12: vec2<f32>) -> vec4<f32> {
    var uv_13: vec2<f32>;
    var f_6: f32;
    var reveal: f32;
    var glitch: f32;
    var rnd: f32;
    var jitter: vec2<f32>;
    var fromUv: vec2<f32>;
    var toUv: vec2<f32>;
    var block: f32;
    var localReveal: f32;
    var oldClip: vec4<f32>;
    var newClip: vec4<f32>;
    var color: vec4<f32>;
    var leftWash: f32;
    var cyanWash: f32;
    var redGhost: vec3<f32>;
    var cyanGhost: vec3<f32>;
    var oldReturn: f32;
    var lines: f32;
    var exposurePulse: f32;

    uv_13 = uv_12;
    let _e20 = progress;
    if (_e20 <= 0f) {
        let _e23 = uv_13;
        let _e24 = getFromColor(_e23);
        return _e24;
    }
    let _e25 = progress;
    if (_e25 >= 1f) {
        let _e28 = uv_13;
        let _e29 = getToColor(_e28);
        return _e29;
    }
    let _e30 = progress;
    let _e31 = frameRate;
    f_6 = floor((_e30 * _e31));
    let _e35 = f_6;
    let _e36 = frameReveal(_e35);
    reveal = _e36;
    let _e38 = f_6;
    let _e39 = frameGlitch(_e38);
    glitch = _e39;
    let _e41 = f_6;
    let _e44 = hash12_(vec2<f32>(_e41, 9.13f));
    rnd = _e44;
    let _e46 = rnd;
    let _e51 = f_6;
    let _e54 = hash12_(vec2<f32>(_e51, 2.71f));
    let _e60 = glitch;
    jitter = (vec2<f32>(((_e46 - 0.5f) * 0.042f), ((_e54 - 0.5f) * 0.01f)) * _e60);
    let _e63 = uv_13;
    let _e64 = jitter;
    let _e66 = safeUv((_e63 + _e64));
    fromUv = _e66;
    let _e68 = uv_13;
    let _e69 = jitter;
    let _e73 = safeUv((_e68 - (_e69 * 0.55f)));
    toUv = _e73;
    let _e75 = uv_13;
    let _e76 = f_6;
    let _e77 = blockMask(_e75, _e76);
    block = _e77;
    let _e79 = reveal;
    let _e80 = block;
    let _e83 = blockAmount;
    let _e85 = glitch;
    let _e88 = f_6;
    let _e89 = uv_13;
    let _e95 = hash12_(vec2<f32>(_e88, floor((_e89.y * 9f))));
    let _e100 = glitch;
    let _e103 = sat(((_e79 + (((_e80 - 0.5f) * _e83) * _e85)) + (((_e95 - 0.5f) * 0.18f) * _e100)));
    localReveal = _e103;
    let _e107 = localReveal;
    localReveal = smoothstep(0.22f, 0.78f, _e107);
    let _e109 = fromUv;
    let _e110 = rgbOffset;
    let _e111 = glitch;
    let _e113 = chromaFrom(_e109, (_e110 * _e111));
    oldClip = _e113;
    let _e115 = toUv;
    let _e116 = rgbOffset;
    let _e117 = glitch;
    let _e119 = chromaTo(_e115, (_e116 * _e117));
    newClip = _e119;
    let _e121 = oldClip;
    let _e122 = newClip;
    let _e123 = localReveal;
    color = mix(_e121, _e122, vec4(_e123));
    let _e130 = uv_13;
    let _e134 = glitch;
    leftWash = ((1f - smoothstep(0.1f, 0.78f, _e130.x)) * _e134);
    let _e139 = uv_13;
    let _e145 = uv_13;
    let _e150 = glitch;
    cyanWash = ((smoothstep(0.08f, 0.62f, _e139.x) * (1f - smoothstep(0.86f, 1f, _e145.x))) * _e150);
    let _e153 = oldClip;
    redGhost = (_e153.xyz * vec3<f32>(1.34f, 0.42f, 0.38f));
    let _e161 = newClip;
    cyanGhost = (_e161.xyz * vec3<f32>(0.48f, 1.12f, 1.24f));
    let _e169 = color;
    let _e171 = color;
    let _e173 = redGhost;
    let _e174 = leftWash;
    let _e175 = redCyan;
    let _e180 = mix(_e171.xyz, _e173, vec3(((_e174 * _e175) * 0.42f)));
    color.x = _e180.x;
    color.y = _e180.y;
    color.z = _e180.z;
    let _e187 = color;
    let _e189 = color;
    let _e191 = cyanGhost;
    let _e192 = cyanWash;
    let _e193 = redCyan;
    let _e198 = mix(_e189.xyz, _e191, vec3(((_e192 * _e193) * 0.3f)));
    color.x = _e198.x;
    color.y = _e198.y;
    color.z = _e198.z;
    let _e205 = glitch;
    let _e209 = reveal;
    oldReturn = (_e205 * (1f - smoothstep(0.94f, 1f, _e209)));
    let _e214 = oldReturn;
    let _e218 = localReveal;
    oldReturn = (_e214 * (0.18f + (0.52f * (1f - (abs((_e218 - 0.5f)) * 2f)))));
    let _e228 = color;
    let _e230 = color;
    let _e232 = oldClip;
    let _e234 = oldReturn;
    let _e235 = ghostAmount;
    let _e238 = mix(_e230.xyz, _e232.xyz, vec3((_e234 * _e235)));
    color.x = _e238.x;
    color.y = _e238.y;
    color.z = _e238.z;
    let _e245 = uv_13;
    let _e247 = f_6;
    lines = sin(((_e245.y + (_e247 * 0.017f)) * 1080f));
    let _e255 = color;
    let _e257 = color;
    let _e259 = lines;
    let _e260 = scanline;
    let _e262 = glitch;
    let _e265 = (_e257.xyz + vec3(((_e259 * _e260) * _e262)));
    color.x = _e265.x;
    color.y = _e265.y;
    color.z = _e265.z;
    let _e272 = f_6;
    let _e275 = hash12_(vec2<f32>(_e272, 12.4f));
    let _e280 = glitch;
    exposurePulse = (((_e275 - 0.35f) * 0.1f) * _e280);
    let _e283 = color;
    let _e285 = color;
    let _e287 = exposurePulse;
    let _e289 = (_e285.xyz + vec3(_e287));
    color.x = _e289.x;
    color.y = _e289.y;
    color.z = _e289.z;
    let _e296 = color;
    let _e298 = sat(_e296.x);
    let _e299 = color;
    let _e301 = sat(_e299.y);
    let _e302 = color;
    let _e304 = sat(_e302.z);
    return vec4<f32>(_e298, _e301, _e304, 1f);
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e18 = U;
    progress = _e18.state.x;
    let _e22 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e22);
    let _e25 = dims;
    let _e28 = dims;
    ratio = (f32(_e25.x) / f32(max(_e28.y, 1i)));
    let _e37 = U.params[0];
    frameRate = _e37.x;
    let _e42 = U.params[1];
    rgbOffset = _e42.x;
    let _e47 = U.params[2];
    blockAmount = _e47.x;
    let _e52 = U.params[3];
    ghostAmount = _e52.x;
    let _e57 = U.params[4];
    redCyan = _e57.x;
    let _e62 = U.params[5];
    scanline = _e62.x;
    chukcut_init_globals();
    let _e64 = v_uv_1;
    let _e67 = v_uv_1;
    let _e71 = transition(vec2<f32>(_e64.x, (1f - _e67.y)));
    c_2 = _e71;
    let _e73 = c_2;
    a = clamp(_e73.w, 0f, 1f);
    let _e79 = a;
    if (_e79 > 0.00001f) {
        let _e82 = c_2;
        let _e84 = a;
        let _e86 = (_e82.xyz / vec3(_e84));
        let _e87 = a;
        local = vec4<f32>(_e86.x, _e86.y, _e86.z, _e87);
    } else {
        local = vec4(0f);
    }
    let _e95 = local;
    o_color = _e95;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e34 = o_color;
    return FragmentOutput(_e34);
}

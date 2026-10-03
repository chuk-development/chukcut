// StripDatamoshGlitch, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: bread
// License: MIT
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
var<private> strength: f32;
var<private> horizontalBars: f32;
var<private> verticalSlits: f32;
var<private> tear: f32;
var<private> chroma: f32;
var<private> residue: f32;
var<private> noiseAmount: f32;
var<private> scanAmount: f32;
var<private> flashAmount: f32;

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

fn hash(n: f32) -> f32 {
    var n_1: f32;

    n_1 = n;
    let _e24 = n_1;
    return fract((sin(_e24) * 43758.547f));
}

fn hash_1(p: vec2<f32>) -> f32 {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e24 = p_1;
    return fract((sin(dot(_e24, vec2<f32>(127.1f, 311.7f))) * 43758.547f));
}

fn sat(v: f32) -> f32 {
    var v_1: f32;

    v_1 = v;
    let _e24 = v_1;
    return clamp(_e24, 0f, 1f);
}

fn burst() -> f32 {
    let _e23 = progress;
    let _e29 = strength;
    return (pow(max(0f, sin((_e23 * PI))), 0.42f) * _e29);
}

fn safeUv(uv_4: vec2<f32>) -> vec2<f32> {
    var uv_5: vec2<f32>;

    uv_5 = uv_4;
    let _e24 = uv_5;
    return clamp(_e24, vec2(0f), vec2(1f));
}

fn stripeY(uv_6: vec2<f32>, density: f32, seed: f32, minWidth: f32, maxWidth: f32) -> f32 {
    var uv_7: vec2<f32>;
    var density_1: f32;
    var seed_1: f32;
    var minWidth_1: f32;
    var maxWidth_1: f32;
    var y: f32;
    var id: f32;
    var f: f32;
    var c_2: f32;
    var w: f32;

    uv_7 = uv_6;
    density_1 = density;
    seed_1 = seed;
    minWidth_1 = minWidth;
    maxWidth_1 = maxWidth;
    let _e32 = uv_7;
    let _e34 = density_1;
    let _e36 = seed_1;
    y = ((_e32.y * _e34) + (_e36 * 0.137f));
    let _e41 = y;
    id = floor(_e41);
    let _e44 = y;
    f = fract(_e44);
    let _e47 = id;
    let _e48 = seed_1;
    let _e50 = hash_1(vec2<f32>(_e47, _e48));
    c_2 = _e50;
    let _e52 = minWidth_1;
    let _e53 = maxWidth_1;
    let _e54 = id;
    let _e57 = seed_1;
    let _e61 = hash_1(vec2<f32>((_e54 + 9.17f), (_e57 + 2.31f)));
    w = mix(_e52, _e53, _e61);
    let _e65 = w;
    let _e66 = w;
    let _e69 = f;
    let _e70 = c_2;
    return (1f - smoothstep(_e65, (_e66 + 0.018f), abs((_e69 - _e70))));
}

fn stripeX(uv_8: vec2<f32>, density_2: f32, seed_2: f32, minWidth_2: f32, maxWidth_2: f32) -> f32 {
    var uv_9: vec2<f32>;
    var density_3: f32;
    var seed_3: f32;
    var minWidth_3: f32;
    var maxWidth_3: f32;
    var x: f32;
    var id_1: f32;
    var f_1: f32;
    var c_3: f32;
    var w_1: f32;

    uv_9 = uv_8;
    density_3 = density_2;
    seed_3 = seed_2;
    minWidth_3 = minWidth_2;
    maxWidth_3 = maxWidth_2;
    let _e32 = uv_9;
    let _e34 = density_3;
    let _e36 = seed_3;
    x = ((_e32.x * _e34) + (_e36 * 0.091f));
    let _e41 = x;
    id_1 = floor(_e41);
    let _e44 = x;
    f_1 = fract(_e44);
    let _e47 = id_1;
    let _e48 = seed_3;
    let _e52 = hash_1(vec2<f32>(_e47, (_e48 + 41f)));
    c_3 = _e52;
    let _e54 = minWidth_3;
    let _e55 = maxWidth_3;
    let _e56 = id_1;
    let _e59 = seed_3;
    let _e63 = hash_1(vec2<f32>((_e56 + 4.7f), (_e59 + 8.9f)));
    w_1 = mix(_e54, _e55, _e63);
    let _e67 = w_1;
    let _e68 = w_1;
    let _e71 = f_1;
    let _e72 = c_3;
    return (1f - smoothstep(_e67, (_e68 + 0.012f), abs((_e71 - _e72))));
}

fn brokenGate(uv_10: vec2<f32>, row: f32, rnd: f32, frame: f32) -> f32 {
    var uv_11: vec2<f32>;
    var row_1: f32;
    var rnd_1: f32;
    var frame_1: f32;
    var segs: f32;
    var seg: f32;

    uv_11 = uv_10;
    row_1 = row;
    rnd_1 = rnd;
    frame_1 = frame;
    let _e32 = row_1;
    let _e33 = frame_1;
    let _e37 = hash_1(vec2<f32>(_e32, (_e33 + 44f)));
    segs = mix(1f, 9f, _e37);
    let _e40 = uv_11;
    let _e42 = segs;
    seg = floor((_e40.x * _e42));
    let _e47 = seg;
    let _e48 = row_1;
    let _e49 = frame_1;
    let _e53 = rnd_1;
    let _e56 = hash_1(vec2<f32>(_e47, ((_e48 + (_e49 * 3f)) + _e53)));
    return step(0.16f, _e56);
}

fn horizontalMask(uv_12: vec2<f32>, frame_2: f32) -> f32 {
    var uv_13: vec2<f32>;
    var frame_3: f32;
    var r1_: f32;
    var r2_: f32;
    var r3_: f32;
    var thick: f32;
    var mid: f32;
    var hair: f32;

    uv_13 = uv_12;
    frame_3 = frame_2;
    let _e26 = uv_13;
    let _e28 = frame_3;
    let _e29 = hash(_e28);
    let _e33 = horizontalBars;
    r1_ = floor((((_e26.y + (_e29 * 0.031f)) * _e33) * 0.38f));
    let _e39 = uv_13;
    let _e41 = frame_3;
    let _e44 = hash((_e41 + 2f));
    let _e48 = horizontalBars;
    r2_ = floor(((_e39.y + (_e44 * 0.013f)) * _e48));
    let _e52 = uv_13;
    let _e54 = frame_3;
    let _e57 = hash((_e54 + 7f));
    let _e61 = horizontalBars;
    r3_ = floor((((_e52.y + (_e57 * 0.006f)) * _e61) * 3.4f));
    let _e67 = uv_13;
    let _e68 = horizontalBars;
    let _e71 = frame_3;
    let _e76 = stripeY(_e67, (_e68 * 0.38f), (_e71 + 1f), 0.035f, 0.22f);
    thick = _e76;
    let _e78 = uv_13;
    let _e79 = horizontalBars;
    let _e80 = frame_3;
    let _e85 = stripeY(_e78, _e79, (_e80 + 4f), 0.014f, 0.11f);
    mid = _e85;
    let _e87 = uv_13;
    let _e88 = horizontalBars;
    let _e91 = frame_3;
    let _e96 = stripeY(_e87, (_e88 * 3.4f), (_e91 + 9f), 0.004f, 0.035f);
    hair = _e96;
    let _e98 = thick;
    let _e100 = r1_;
    let _e101 = frame_3;
    let _e105 = hash_1(vec2<f32>(_e100, (_e101 + 10f)));
    thick = (_e98 * step(0.42f, _e105));
    let _e108 = mid;
    let _e110 = r2_;
    let _e111 = frame_3;
    let _e115 = hash_1(vec2<f32>(_e110, (_e111 + 20f)));
    mid = (_e108 * step(0.48f, _e115));
    let _e118 = hair;
    let _e120 = r3_;
    let _e121 = frame_3;
    let _e125 = hash_1(vec2<f32>(_e120, (_e121 + 30f)));
    hair = (_e118 * step(0.62f, _e125));
    let _e128 = thick;
    let _e129 = uv_13;
    let _e130 = r1_;
    let _e131 = r1_;
    let _e132 = frame_3;
    let _e134 = hash_1(vec2<f32>(_e131, _e132));
    let _e135 = frame_3;
    let _e136 = brokenGate(_e129, _e130, _e134, _e135);
    thick = (_e128 * _e136);
    let _e138 = mid;
    let _e139 = uv_13;
    let _e140 = r2_;
    let _e141 = r2_;
    let _e142 = frame_3;
    let _e144 = hash_1(vec2<f32>(_e141, _e142));
    let _e145 = frame_3;
    let _e148 = brokenGate(_e139, _e140, _e144, (_e145 + 3f));
    mid = (_e138 * _e148);
    let _e150 = thick;
    let _e151 = mid;
    let _e152 = hair;
    let _e155 = sat(max(_e150, max(_e151, _e152)));
    return _e155;
}

fn verticalMask(uv_14: vec2<f32>, frame_4: f32) -> f32 {
    var uv_15: vec2<f32>;
    var frame_5: f32;
    var col: f32;
    var slit: f32;

    uv_15 = uv_14;
    frame_5 = frame_4;
    let _e26 = uv_15;
    let _e28 = frame_5;
    let _e31 = hash((_e28 + 12f));
    let _e35 = verticalSlits;
    col = floor(((_e26.x + (_e31 * 0.017f)) * _e35));
    let _e39 = uv_15;
    let _e40 = verticalSlits;
    let _e41 = frame_5;
    let _e46 = stripeX(_e39, _e40, (_e41 + 13f), 0.01f, 0.075f);
    slit = _e46;
    let _e48 = slit;
    let _e50 = col;
    let _e51 = frame_5;
    let _e55 = hash_1(vec2<f32>(_e50, (_e51 + 19f)));
    slit = (_e48 * step(0.66f, _e55));
    let _e58 = slit;
    let _e59 = sat(_e58);
    return _e59;
}

fn chromaFrom(uv_16: vec2<f32>, s: vec2<f32>) -> vec4<f32> {
    var uv_17: vec2<f32>;
    var s_1: vec2<f32>;

    uv_17 = uv_16;
    s_1 = s;
    let _e26 = uv_17;
    let _e27 = safeUv(_e26);
    uv_17 = _e27;
    let _e28 = uv_17;
    let _e29 = s_1;
    let _e31 = safeUv((_e28 + _e29));
    let _e32 = getFromColor(_e31);
    let _e34 = uv_17;
    let _e35 = getFromColor(_e34);
    let _e37 = uv_17;
    let _e38 = s_1;
    let _e40 = safeUv((_e37 - _e38));
    let _e41 = getFromColor(_e40);
    return vec4<f32>(_e32.x, _e35.y, _e41.z, 1f);
}

fn chromaTo(uv_18: vec2<f32>, s_2: vec2<f32>) -> vec4<f32> {
    var uv_19: vec2<f32>;
    var s_3: vec2<f32>;

    uv_19 = uv_18;
    s_3 = s_2;
    let _e26 = uv_19;
    let _e27 = safeUv(_e26);
    uv_19 = _e27;
    let _e28 = uv_19;
    let _e29 = s_3;
    let _e31 = safeUv((_e28 - _e29));
    let _e32 = getToColor(_e31);
    let _e34 = uv_19;
    let _e35 = getToColor(_e34);
    let _e37 = uv_19;
    let _e38 = s_3;
    let _e40 = safeUv((_e37 + _e38));
    let _e41 = getToColor(_e40);
    return vec4<f32>(_e32.x, _e35.y, _e41.z, 1f);
}

fn distortUv(uv_20: vec2<f32>, dir: f32, b: f32, h: f32, v_2: f32, frame_6: f32) -> vec2<f32> {
    var uv_21: vec2<f32>;
    var dir_1: f32;
    var b_1: f32;
    var h_1: f32;
    var v_3: f32;
    var frame_7: f32;
    var row_2: f32;
    var col_1: f32;
    var rowRnd: f32;
    var colRnd: f32;
    var xTear: f32;
    var yDrag: f32;
    var micro: f32;

    uv_21 = uv_20;
    dir_1 = dir;
    b_1 = b;
    h_1 = h;
    v_3 = v_2;
    frame_7 = frame_6;
    let _e34 = uv_21;
    let _e36 = horizontalBars;
    row_2 = floor((_e34.y * _e36));
    let _e40 = uv_21;
    let _e42 = verticalSlits;
    col_1 = floor((_e40.x * _e42));
    let _e46 = row_2;
    let _e47 = frame_7;
    let _e49 = hash_1(vec2<f32>(_e46, _e47));
    rowRnd = _e49;
    let _e51 = col_1;
    let _e52 = frame_7;
    let _e56 = hash_1(vec2<f32>(_e51, (_e52 + 27f)));
    colRnd = _e56;
    let _e58 = rowRnd;
    let _e63 = tear;
    let _e65 = b_1;
    let _e67 = h_1;
    xTear = (((((_e58 - 0.5f) * 2f) * _e63) * _e65) * _e67);
    let _e70 = xTear;
    let _e71 = uv_21;
    let _e75 = progress;
    let _e82 = b_1;
    xTear = (_e70 + ((sin(((_e71.y * 120f) + (_e75 * 95f))) * 0.006f) * _e82));
    let _e85 = colRnd;
    let _e90 = b_1;
    let _e92 = v_3;
    yDrag = ((((_e85 - 0.5f) * 0.13f) * _e90) * _e92);
    let _e95 = row_2;
    let _e96 = col_1;
    let _e97 = frame_7;
    let _e100 = hash_1(vec2<f32>(_e95, (_e96 + _e97)));
    let _e105 = b_1;
    let _e107 = h_1;
    let _e108 = v_3;
    micro = ((((_e100 - 0.5f) * 0.018f) * _e105) * max(_e107, _e108));
    let _e112 = uv_21;
    let _e113 = xTear;
    let _e114 = dir_1;
    let _e116 = micro;
    let _e118 = yDrag;
    return (_e112 + vec2<f32>(((_e113 * _e114) + _e116), _e118));
}

fn transition(uv_22: vec2<f32>) -> vec4<f32> {
    var uv_23: vec2<f32>;
    var b_2: f32;
    var frame_8: f32;
    var h_2: f32;
    var v_4: f32;
    var glitch: f32;
    var row_3: f32;
    var rowRnd_1: f32;
    var bandDelay: f32;
    var reveal: f32;
    var split: vec2<f32>;
    var fromUv: vec2<f32>;
    var toUv: vec2<f32>;
    var color: vec4<f32>;
    var smearUv: vec2<f32>;
    var sliceReveal: f32;
    var sliceColor: vec4<f32>;
    var hairLine: f32;
    var scan: f32;
    var nCell: vec2<f32>;
    var n_2: f32;
    var luma: f32;
    var strobe: f32;

    uv_23 = uv_22;
    let _e24 = progress;
    if (_e24 <= 0f) {
        let _e27 = uv_23;
        let _e28 = getFromColor(_e27);
        return _e28;
    }
    let _e29 = progress;
    if (_e29 >= 1f) {
        let _e32 = uv_23;
        let _e33 = getToColor(_e32);
        return _e33;
    }
    let _e34 = burst();
    b_2 = _e34;
    let _e36 = progress;
    frame_8 = floor((_e36 * 30f));
    let _e41 = uv_23;
    let _e42 = frame_8;
    let _e43 = horizontalMask(_e41, _e42);
    h_2 = _e43;
    let _e45 = uv_23;
    let _e46 = frame_8;
    let _e47 = verticalMask(_e45, _e46);
    v_4 = _e47;
    let _e49 = h_2;
    let _e50 = v_4;
    let _e54 = sat(max(_e49, (_e50 * 0.75f)));
    glitch = _e54;
    let _e56 = uv_23;
    let _e58 = horizontalBars;
    row_3 = floor((_e56.y * _e58));
    let _e62 = row_3;
    let _e63 = frame_8;
    let _e67 = hash_1(vec2<f32>(_e62, (_e63 + 5f)));
    rowRnd_1 = _e67;
    let _e69 = rowRnd_1;
    let _e74 = h_2;
    bandDelay = (((_e69 - 0.5f) * 0.3f) * _e74);
    let _e79 = progress;
    let _e80 = bandDelay;
    reveal = smoothstep(0.18f, 0.84f, (_e79 + _e80));
    let _e84 = chroma;
    let _e85 = b_2;
    let _e89 = glitch;
    let _e93 = chroma;
    let _e96 = b_2;
    let _e98 = v_4;
    split = vec2<f32>(((_e84 * _e85) * (1f + (1.7f * _e89))), (((_e93 * 0.22f) * _e96) * _e98));
    let _e102 = uv_23;
    let _e104 = b_2;
    let _e105 = h_2;
    let _e106 = v_4;
    let _e107 = frame_8;
    let _e108 = distortUv(_e102, 1f, _e104, _e105, _e106, _e107);
    fromUv = _e108;
    let _e110 = uv_23;
    let _e113 = b_2;
    let _e114 = h_2;
    let _e115 = v_4;
    let _e116 = frame_8;
    let _e117 = distortUv(_e110, -1f, _e113, _e114, _e115, _e116);
    toUv = _e117;
    let _e119 = fromUv;
    let _e120 = split;
    let _e121 = chromaFrom(_e119, _e120);
    let _e122 = toUv;
    let _e123 = split;
    let _e124 = chromaTo(_e122, _e123);
    let _e125 = reveal;
    color = mix(_e121, _e124, vec4(_e125));
    let _e129 = uv_23;
    smearUv = _e129;
    let _e132 = smearUv;
    let _e134 = rowRnd_1;
    let _e139 = b_2;
    let _e141 = h_2;
    smearUv.x = (_e132.x + ((((_e134 - 0.5f) * 0.46f) * _e139) * _e141));
    let _e145 = smearUv;
    let _e147 = row_3;
    let _e148 = frame_8;
    let _e152 = hash_1(vec2<f32>(_e147, (_e148 + 31f)));
    let _e157 = b_2;
    let _e159 = h_2;
    smearUv.y = (_e145.y + ((((_e152 - 0.5f) * 0.045f) * _e157) * _e159));
    let _e164 = progress;
    let _e165 = rowRnd_1;
    sliceReveal = smoothstep(0.28f, 0.78f, (_e164 + ((_e165 - 0.5f) * 0.22f)));
    let _e173 = smearUv;
    let _e174 = split;
    let _e177 = chromaFrom(_e173, (_e174 * 1.65f));
    let _e178 = smearUv;
    let _e179 = rowRnd_1;
    let _e184 = b_2;
    let _e189 = split;
    let _e192 = chromaTo((_e178 - vec2<f32>((((_e179 - 0.5f) * 0.18f) * _e184), 0f)), (_e189 * 1.65f));
    let _e193 = sliceReveal;
    sliceColor = mix(_e177, _e192, vec4(_e193));
    let _e197 = color;
    let _e198 = sliceColor;
    let _e199 = h_2;
    let _e200 = b_2;
    let _e202 = residue;
    color = mix(_e197, _e198, vec4(((_e199 * _e200) * _e202)));
    let _e206 = uv_23;
    let _e208 = frame_8;
    let _e213 = stripeY(_e206, 190f, (_e208 + 55f), 0.002f, 0.012f);
    hairLine = _e213;
    let _e215 = hairLine;
    let _e217 = uv_23;
    let _e222 = frame_8;
    let _e226 = hash_1(vec2<f32>(floor((_e217.y * 190f)), (_e222 + 56f)));
    hairLine = (_e215 * step(0.7f, _e226));
    let _e229 = color;
    let _e231 = color;
    let _e237 = hairLine;
    let _e239 = b_2;
    let _e243 = (_e231.xyz + (((vec3<f32>(0.72f, 0.9f, 1f) * _e237) * _e239) * 0.28f));
    color.x = _e243.x;
    color.y = _e243.y;
    color.z = _e243.z;
    let _e252 = uv_23;
    let _e256 = progress;
    scan = (0.5f + (0.5f * sin(((_e252.y * 980f) + (_e256 * 130f)))));
    let _e264 = color;
    let _e266 = color;
    let _e269 = scanAmount;
    let _e270 = b_2;
    let _e272 = scan;
    let _e275 = (_e266.xyz * (1f - ((_e269 * _e270) * _e272)));
    color.x = _e275.x;
    color.y = _e275.y;
    color.z = _e275.z;
    let _e282 = uv_23;
    let _e284 = ratio;
    nCell = floor((_e282 * vec2<f32>((360f * _e284), 210f)));
    let _e291 = nCell;
    let _e292 = frame_8;
    let _e295 = frame_8;
    let _e300 = hash_1((_e291 + vec2<f32>((_e292 * 7f), (_e295 * 13f))));
    n_2 = _e300;
    let _e302 = color;
    let _e304 = color;
    let _e306 = n_2;
    let _e309 = noiseAmount;
    let _e311 = b_2;
    let _e314 = glitch;
    let _e318 = (_e304.xyz + vec3(((((_e306 - 0.5f) * _e309) * _e311) * (0.55f + _e314))));
    color.x = _e318.x;
    color.y = _e318.y;
    color.z = _e318.z;
    let _e325 = color;
    luma = dot(_e325.xyz, vec3<f32>(0.299f, 0.587f, 0.114f));
    let _e333 = color;
    let _e335 = color;
    let _e337 = luma;
    let _e340 = b_2;
    let _e342 = glitch;
    let _e345 = mix(_e335.xyz, vec3(_e337), vec3(((0.18f * _e340) * _e342)));
    color.x = _e345.x;
    color.y = _e345.y;
    color.z = _e345.z;
    let _e353 = frame_8;
    let _e356 = hash_1(vec2<f32>(_e353, 3.14f));
    let _e358 = b_2;
    strobe = (step(0.78f, _e356) * pow(_e358, 1.65f));
    let _e363 = color;
    let _e365 = color;
    let _e367 = strobe;
    let _e368 = flashAmount;
    let _e371 = (_e365.xyz + vec3((_e367 * _e368)));
    color.x = _e371.x;
    color.y = _e371.y;
    color.z = _e371.z;
    let _e378 = color;
    let _e384 = clamp(_e378.xyz, vec3(0f), vec3(1f));
    return vec4<f32>(_e384.x, _e384.y, _e384.z, 1f);
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_4: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e22 = U;
    progress = _e22.state.x;
    let _e26 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e26);
    let _e29 = dims;
    let _e32 = dims;
    ratio = (f32(_e29.x) / f32(max(_e32.y, 1i)));
    let _e41 = U.params[0];
    strength = _e41.x;
    let _e46 = U.params[1];
    horizontalBars = _e46.x;
    let _e51 = U.params[2];
    verticalSlits = _e51.x;
    let _e56 = U.params[3];
    tear = _e56.x;
    let _e61 = U.params[4];
    chroma = _e61.x;
    let _e66 = U.params[5];
    residue = _e66.x;
    let _e71 = U.params[6];
    noiseAmount = _e71.x;
    let _e76 = U.params[7];
    scanAmount = _e76.x;
    let _e81 = U.params[8];
    flashAmount = _e81.x;
    chukcut_init_globals();
    let _e83 = v_uv_1;
    let _e86 = v_uv_1;
    let _e90 = transition(vec2<f32>(_e83.x, (1f - _e86.y)));
    c_4 = _e90;
    let _e92 = c_4;
    a = clamp(_e92.w, 0f, 1f);
    let _e98 = a;
    if (_e98 > 0.00001f) {
        let _e101 = c_4;
        let _e103 = a;
        let _e105 = (_e101.xyz / vec3(_e103));
        let _e106 = a;
        local = vec4<f32>(_e105.x, _e105.y, _e105.z, _e106);
    } else {
        local = vec4(0f);
    }
    let _e114 = local;
    o_color = _e114;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e42 = o_color;
    return FragmentOutput(_e42);
}

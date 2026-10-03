// dissolve, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: hjm1fb
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
var<private> uLineWidth: f32;
var<private> uSpreadClr: vec3<f32>;
var<private> uHotClr: vec3<f32>;
var<private> uPow: f32;
var<private> uIntensity: f32;

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

fn hash(p: vec2<f32>) -> vec2<f32> {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e19 = p_1;
    let _e24 = p_1;
    p_1 = vec2<f32>(dot(_e19, vec2<f32>(127.1f, 311.7f)), dot(_e24, vec2<f32>(269.5f, 183.3f)));
    let _e33 = p_1;
    return (vec2(-1f) + (2f * fract((sin(_e33) * 43758.547f))));
}

fn noise(p_2: vec2<f32>) -> f32 {
    var p_3: vec2<f32>;
    var K1_: f32 = 0.36602542f;
    var K2_: f32 = 0.21132487f;
    var i: vec2<f32>;
    var a: vec2<f32>;
    var m: f32;
    var o: vec2<f32>;
    var b: vec2<f32>;
    var c_2: vec2<f32>;
    var h: vec3<f32>;
    var n: vec3<f32>;

    p_3 = p_2;
    let _e23 = p_3;
    let _e24 = p_3;
    let _e26 = p_3;
    let _e29 = K1_;
    i = floor((_e23 + vec2(((_e24.x + _e26.y) * _e29))));
    let _e35 = p_3;
    let _e36 = i;
    let _e38 = i;
    let _e40 = i;
    let _e43 = K2_;
    a = ((_e35 - _e36) + vec2(((_e38.x + _e40.y) * _e43)));
    let _e48 = a;
    let _e50 = a;
    m = step(_e48.y, _e50.x);
    let _e54 = m;
    let _e56 = m;
    o = vec2<f32>(_e54, (1f - _e56));
    let _e60 = a;
    let _e61 = o;
    let _e63 = K2_;
    b = ((_e60 - _e61) + vec2(_e63));
    let _e67 = a;
    let _e72 = K2_;
    c_2 = ((_e67 - vec2(1f)) + vec2((2f * _e72)));
    let _e78 = a;
    let _e79 = a;
    let _e81 = b;
    let _e82 = b;
    let _e84 = c_2;
    let _e85 = c_2;
    h = max((vec3(0.5f) - vec3<f32>(dot(_e78, _e79), dot(_e81, _e82), dot(_e84, _e85))), vec3(0f));
    let _e94 = h;
    let _e95 = h;
    let _e97 = h;
    let _e99 = h;
    let _e101 = a;
    let _e102 = i;
    let _e106 = hash((_e102 + vec2(0f)));
    let _e108 = b;
    let _e109 = i;
    let _e110 = o;
    let _e112 = hash((_e109 + _e110));
    let _e114 = c_2;
    let _e115 = i;
    let _e119 = hash((_e115 + vec2(1f)));
    n = ((((_e94 * _e95) * _e97) * _e99) * vec3<f32>(dot(_e101, _e106), dot(_e108, _e112), dot(_e114, _e119)));
    let _e124 = n;
    return dot(_e124, vec3(70f));
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var from_: vec4<f32>;
    var to: vec4<f32>;
    var outColor: vec4<f32>;
    var burn: f32;
    var show: f32;
    var factor: f32;
    var burnColor: vec3<f32>;
    var finalRGB: vec3<f32>;

    uv_5 = uv_4;
    let _e19 = uv_5;
    let _e20 = getFromColor(_e19);
    from_ = _e20;
    let _e22 = uv_5;
    let _e23 = getToColor(_e22);
    to = _e23;
    let _e30 = from_;
    let _e34 = from_;
    let _e39 = from_;
    burn = (0.5f + (0.5f * (((0.299f * _e30.x) + (0.587f * _e34.y)) + (0.114f * _e39.z))));
    let _e45 = burn;
    let _e46 = progress;
    show = (_e45 - _e46);
    let _e49 = show;
    if (_e49 < 0.001f) {
        {
            let _e52 = to;
            outColor = _e52;
        }
    } else {
        {
            let _e55 = uLineWidth;
            let _e56 = show;
            factor = (1f - smoothstep(0f, _e55, _e56));
            let _e60 = uSpreadClr;
            let _e61 = uHotClr;
            let _e62 = factor;
            burnColor = mix(_e60, _e61, vec3(_e62));
            let _e66 = burnColor;
            let _e67 = uPow;
            let _e70 = uIntensity;
            burnColor = (pow(_e66, vec3(_e67)) * _e70);
            let _e72 = from_;
            let _e74 = burnColor;
            let _e75 = factor;
            let _e77 = progress;
            finalRGB = mix(_e72.xyz, _e74, vec3((_e75 * step(0.0001f, _e77))));
            let _e83 = finalRGB;
            let _e84 = from_;
            let _e86 = (_e83 * _e84.w);
            let _e87 = from_;
            outColor = vec4<f32>(_e86.x, _e86.y, _e86.z, _e87.w);
        }
    }
    let _e93 = outColor;
    return _e93;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
    var a_1: f32;
    var local: vec4<f32>;

    let _e17 = U;
    progress = _e17.state.x;
    let _e21 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e21);
    let _e24 = dims;
    let _e27 = dims;
    ratio = (f32(_e24.x) / f32(max(_e27.y, 1i)));
    let _e36 = U.params[0];
    uLineWidth = _e36.x;
    let _e41 = U.params[1];
    uSpreadClr = _e41.xyz;
    let _e46 = U.params[2];
    uHotClr = _e46.xyz;
    let _e51 = U.params[3];
    uPow = _e51.x;
    let _e56 = U.params[4];
    uIntensity = _e56.x;
    chukcut_init_globals();
    let _e58 = v_uv_1;
    let _e61 = v_uv_1;
    let _e65 = transition(vec2<f32>(_e58.x, (1f - _e61.y)));
    c_3 = _e65;
    let _e67 = c_3;
    a_1 = clamp(_e67.w, 0f, 1f);
    let _e73 = a_1;
    if (_e73 > 0.00001f) {
        let _e76 = c_3;
        let _e78 = a_1;
        let _e80 = (_e76.xyz / vec3(_e78));
        let _e81 = a_1;
        local = vec4<f32>(_e80.x, _e80.y, _e80.z, _e81);
    } else {
        local = vec4(0f);
    }
    let _e89 = local;
    o_color = _e89;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e32 = o_color;
    return FragmentOutput(_e32);
}

// Revolve_Left, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: bread
// License: MIT
// gl-transitions v1 compatible
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
var<private> center: vec2<f32>;
var<private> direction: f32;
var<private> maxRotation: f32;
var<private> peakZoom: f32;
var<private> swirl: f32;
var<private> barrel: f32;
var<private> motionBlur: f32;
var<private> switchStart: f32;
var<private> switchEnd: f32;
var<private> shadow: f32;

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
    let _e24 = x_1;
    return clamp(_e24, 0f, 1f);
}

fn ease(x_2: f32) -> f32 {
    var x_3: f32;

    x_3 = x_2;
    let _e24 = x_3;
    let _e25 = sat(_e24);
    x_3 = _e25;
    let _e26 = x_3;
    let _e27 = x_3;
    let _e31 = x_3;
    return ((_e26 * _e27) * (3f - (2f * _e31)));
}

fn revolveEnvelope(t: f32) -> f32 {
    var t_1: f32;
    var rise: f32;
    var fall: f32;

    t_1 = t;
    let _e24 = t_1;
    let _e29 = ease(((_e24 - 0.1f) / 0.33f));
    rise = _e29;
    let _e32 = t_1;
    let _e37 = ease(((_e32 - 0.43f) / 0.29f));
    fall = (1f - _e37);
    let _e40 = rise;
    let _e41 = fall;
    return (_e40 * _e41);
}

fn rotate2_(p: vec2<f32>, a: f32) -> vec2<f32> {
    var p_1: vec2<f32>;
    var a_1: f32;
    var s: f32;
    var c_2: f32;

    p_1 = p;
    a_1 = a;
    let _e26 = a_1;
    s = sin(_e26);
    let _e29 = a_1;
    c_2 = cos(_e29);
    let _e32 = c_2;
    let _e33 = p_1;
    let _e36 = s;
    let _e37 = p_1;
    let _e41 = s;
    let _e42 = p_1;
    let _e45 = c_2;
    let _e46 = p_1;
    return vec2<f32>(((_e32 * _e33.x) - (_e36 * _e37.y)), ((_e41 * _e42.x) + (_e45 * _e46.y)));
}

fn warpUv(uv_4: vec2<f32>, t_2: f32) -> vec2<f32> {
    var uv_5: vec2<f32>;
    var t_3: f32;
    var e: f32;
    var p_2: vec2<f32>;
    var r: f32;
    var edgeSpin: f32;
    var coreSpin: f32;
    var visibleAngle: f32;
    var sc: f32;
    var rr: f32;

    uv_5 = uv_4;
    t_3 = t_2;
    let _e26 = t_3;
    let _e27 = revolveEnvelope(_e26);
    e = _e27;
    let _e29 = uv_5;
    let _e30 = center;
    p_2 = (_e29 - _e30);
    let _e34 = p_2;
    let _e36 = ratio;
    p_2.x = (_e34.x * _e36);
    let _e38 = p_2;
    r = length(_e38);
    let _e41 = maxRotation;
    let _e42 = e;
    edgeSpin = (_e41 * _e42);
    let _e45 = swirl;
    let _e46 = e;
    let _e49 = r;
    let _e52 = sat((_e49 / 0.96f));
    coreSpin = ((_e45 * _e46) * pow((1f - _e52), 1.55f));
    let _e58 = direction;
    let _e59 = edgeSpin;
    let _e60 = coreSpin;
    visibleAngle = (_e58 * (_e59 + _e60));
    let _e64 = p_2;
    let _e65 = visibleAngle;
    let _e67 = rotate2_(_e64, -(_e65));
    p_2 = _e67;
    let _e69 = peakZoom;
    let _e72 = e;
    sc = (1f + ((_e69 - 1f) * pow(_e72, 0.85f)));
    let _e78 = p_2;
    let _e79 = sc;
    p_2 = (_e78 / vec2(_e79));
    let _e82 = p_2;
    rr = length(_e82);
    let _e85 = p_2;
    let _e87 = barrel;
    let _e88 = e;
    let _e90 = rr;
    let _e92 = rr;
    p_2 = (_e85 * (1f + ((((_e87 * _e88) * _e90) * _e92) * 2.8f)));
    let _e99 = p_2;
    let _e101 = ratio;
    p_2.x = (_e99.x / _e101);
    let _e103 = p_2;
    let _e104 = center;
    return clamp((_e103 + _e104), vec2(0.001f), vec2(0.999f));
}

fn sampleRevolve(uv_6: vec2<f32>, t_4: f32) -> vec4<f32> {
    var uv_7: vec2<f32>;
    var t_5: f32;
    var p_3: vec2<f32>;
    var reveal: f32;

    uv_7 = uv_6;
    t_5 = t_4;
    let _e26 = uv_7;
    let _e27 = t_5;
    let _e28 = warpUv(_e26, _e27);
    p_3 = _e28;
    let _e30 = switchStart;
    let _e31 = switchEnd;
    let _e32 = t_5;
    reveal = smoothstep(_e30, _e31, _e32);
    let _e35 = p_3;
    let _e36 = getFromColor(_e35);
    let _e37 = p_3;
    let _e38 = getToColor(_e37);
    let _e39 = reveal;
    return mix(_e36, _e38, vec4(_e39));
}

fn transition(uv_8: vec2<f32>) -> vec4<f32> {
    var uv_9: vec2<f32>;
    var e_1: f32;
    var span: f32;
    var color: vec4<f32> = vec4(0f);
    var total: f32 = 0f;
    var i: i32 = -8i;
    var x_4: f32;
    var w: f32;
    var t_6: f32;
    var q: vec2<f32>;
    var vignette: f32;

    uv_9 = uv_8;
    let _e24 = progress;
    if (_e24 <= 0f) {
        let _e27 = uv_9;
        let _e28 = getFromColor(_e27);
        return _e28;
    }
    let _e29 = progress;
    if (_e29 >= 1f) {
        let _e32 = uv_9;
        let _e33 = getToColor(_e32);
        return _e33;
    }
    let _e34 = progress;
    let _e35 = revolveEnvelope(_e34);
    e_1 = _e35;
    let _e38 = motionBlur;
    let _e40 = e_1;
    span = ((0.06f * _e38) * _e40);
    loop {
        let _e51 = i;
        if !((_e51 <= 8i)) {
            break;
        }
        {
            let _e58 = i;
            x_4 = (f32(_e58) / 8f);
            let _e64 = x_4;
            w = (1f - abs(_e64));
            let _e68 = w;
            let _e69 = w;
            w = ((_e68 * _e69) + 0.01f);
            let _e73 = progress;
            let _e74 = x_4;
            let _e75 = span;
            let _e78 = sat((_e73 + (_e74 * _e75)));
            t_6 = _e78;
            let _e80 = color;
            let _e81 = uv_9;
            let _e82 = t_6;
            let _e83 = sampleRevolve(_e81, _e82);
            let _e84 = w;
            color = (_e80 + (_e83 * _e84));
            let _e87 = total;
            let _e88 = w;
            total = (_e87 + _e88);
        }
        continuing {
            let _e55 = i;
            i = (_e55 + 1i);
        }
    }
    let _e90 = color;
    let _e91 = total;
    color = (_e90 / vec4(_e91));
    let _e94 = uv_9;
    q = (_e94 - vec2(0.5f));
    let _e100 = q;
    let _e102 = ratio;
    q.x = (_e100.x * _e102);
    let _e105 = shadow;
    let _e106 = e_1;
    let _e110 = q;
    vignette = (1f - ((_e105 * _e106) * smoothstep(0.35f, 0.95f, length(_e110))));
    let _e116 = color;
    let _e118 = color;
    let _e120 = vignette;
    let _e121 = (_e118.xyz * _e120);
    color.x = _e121.x;
    color.y = _e121.y;
    color.z = _e121.z;
    let _e128 = color;
    return _e128;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
    var a_2: f32;
    var local: vec4<f32>;

    let _e22 = U;
    progress = _e22.state.x;
    let _e26 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e26);
    let _e29 = dims;
    let _e32 = dims;
    ratio = (f32(_e29.x) / f32(max(_e32.y, 1i)));
    let _e41 = U.params[0];
    center = _e41.xy;
    let _e46 = U.params[1];
    direction = _e46.x;
    let _e51 = U.params[2];
    maxRotation = _e51.x;
    let _e56 = U.params[3];
    peakZoom = _e56.x;
    let _e61 = U.params[4];
    swirl = _e61.x;
    let _e66 = U.params[5];
    barrel = _e66.x;
    let _e71 = U.params[6];
    motionBlur = _e71.x;
    let _e76 = U.params[7];
    switchStart = _e76.x;
    let _e81 = U.params[8];
    switchEnd = _e81.x;
    let _e86 = U.params[9];
    shadow = _e86.x;
    chukcut_init_globals();
    let _e88 = v_uv_1;
    let _e91 = v_uv_1;
    let _e95 = transition(vec2<f32>(_e88.x, (1f - _e91.y)));
    c_3 = _e95;
    let _e97 = c_3;
    a_2 = clamp(_e97.w, 0f, 1f);
    let _e103 = a_2;
    if (_e103 > 0.00001f) {
        let _e106 = c_3;
        let _e108 = a_2;
        let _e110 = (_e106.xyz / vec3(_e108));
        let _e111 = a_2;
        local = vec4<f32>(_e110.x, _e110.y, _e110.z, _e111);
    } else {
        local = vec4(0f);
    }
    let _e119 = local;
    o_color = _e119;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e42 = o_color;
    return FragmentOutput(_e42);
}

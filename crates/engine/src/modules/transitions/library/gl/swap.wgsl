// swap, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: gre
// License: MIT
// General parameters
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
const boundMin: vec2<f32> = vec2<f32>(0f, 0f);
const boundMax: vec2<f32> = vec2<f32>(1f, 1f);

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
var<private> reflection: f32;
var<private> perspective: f32;
var<private> depth: f32;

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

fn inBounds(p: vec2<f32>) -> bool {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e20 = p_1;
    let _e23 = p_1;
    return (all((boundMin < _e20)) && all((_e23 < boundMax)));
}

fn project(p_2: vec2<f32>) -> vec2<f32> {
    var p_3: vec2<f32>;

    p_3 = p_2;
    let _e20 = p_3;
    return ((_e20 * vec2<f32>(1f, -1.2f)) + vec2<f32>(0f, -0.02f));
}

fn bgColor(p_4: vec2<f32>, pfr: vec2<f32>, pto: vec2<f32>) -> vec4<f32> {
    var p_5: vec2<f32>;
    var pfr_1: vec2<f32>;
    var pto_1: vec2<f32>;
    var c_2: vec4<f32> = black;

    p_5 = p_4;
    pfr_1 = pfr;
    pto_1 = pto;
    let _e25 = pfr_1;
    let _e26 = project(_e25);
    pfr_1 = _e26;
    let _e27 = pfr_1;
    let _e28 = inBounds(_e27);
    if _e28 {
        {
            let _e29 = c_2;
            let _e30 = pfr_1;
            let _e31 = getFromColor(_e30);
            let _e32 = reflection;
            let _e35 = pfr_1;
            c_2 = (_e29 + mix(black, _e31, vec4((_e32 * mix(1f, 0f, _e35.y)))));
        }
    }
    let _e42 = pto_1;
    let _e43 = project(_e42);
    pto_1 = _e43;
    let _e44 = pto_1;
    let _e45 = inBounds(_e44);
    if _e45 {
        {
            let _e46 = c_2;
            let _e47 = pto_1;
            let _e48 = getToColor(_e47);
            let _e49 = reflection;
            let _e52 = pto_1;
            c_2 = (_e46 + mix(black, _e48, vec4((_e49 * mix(1f, 0f, _e52.y)))));
        }
    }
    let _e59 = c_2;
    return _e59;
}

fn transition(p_6: vec2<f32>) -> vec4<f32> {
    var p_7: vec2<f32>;
    var pfr_2: vec2<f32>;
    var pto_2: vec2<f32> = vec2(-1f);
    var size: f32;
    var persp: f32;

    p_7 = p_6;
    let _e26 = depth;
    let _e27 = progress;
    size = mix(1f, _e26, _e27);
    let _e30 = perspective;
    let _e31 = progress;
    persp = (_e30 * _e31);
    let _e34 = p_7;
    let _e41 = size;
    let _e43 = perspective;
    let _e44 = progress;
    let _e48 = size;
    let _e50 = size;
    let _e51 = persp;
    let _e53 = p_7;
    pfr_2 = (((_e34 + vec2<f32>(-0f, -0.5f)) * vec2<f32>((_e41 / (1f - (_e43 * _e44))), (_e48 / (1f - ((_e50 * _e51) * _e53.x))))) + vec2<f32>(0f, 0.5f));
    let _e65 = depth;
    let _e67 = progress;
    size = mix(1f, _e65, (1f - _e67));
    let _e70 = perspective;
    let _e72 = progress;
    persp = (_e70 * (1f - _e72));
    let _e75 = p_7;
    let _e82 = size;
    let _e84 = perspective;
    let _e86 = progress;
    let _e91 = size;
    let _e93 = size;
    let _e94 = persp;
    let _e97 = p_7;
    pto_2 = (((_e75 + vec2<f32>(-1f, -0.5f)) * vec2<f32>((_e82 / (1f - (_e84 * (1f - _e86)))), (_e91 / (1f - ((_e93 * _e94) * (0.5f - _e97.x)))))) + vec2<f32>(1f, 0.5f));
    let _e109 = progress;
    if (_e109 < 0.5f) {
        {
            let _e112 = pfr_2;
            let _e113 = inBounds(_e112);
            if _e113 {
                {
                    let _e114 = pfr_2;
                    let _e115 = getFromColor(_e114);
                    return _e115;
                }
            }
            let _e116 = pto_2;
            let _e117 = inBounds(_e116);
            if _e117 {
                {
                    let _e118 = pto_2;
                    let _e119 = getToColor(_e118);
                    return _e119;
                }
            }
        }
    }
    let _e120 = pto_2;
    let _e121 = inBounds(_e120);
    if _e121 {
        {
            let _e122 = pto_2;
            let _e123 = getToColor(_e122);
            return _e123;
        }
    }
    let _e124 = pfr_2;
    let _e125 = inBounds(_e124);
    if _e125 {
        {
            let _e126 = pfr_2;
            let _e127 = getFromColor(_e126);
            return _e127;
        }
    }
    let _e128 = p_7;
    let _e129 = pfr_2;
    let _e130 = pto_2;
    let _e131 = bgColor(_e128, _e129, _e130);
    return _e131;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
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
    reflection = _e37.x;
    let _e42 = U.params[1];
    perspective = _e42.x;
    let _e47 = U.params[2];
    depth = _e47.x;
    chukcut_init_globals();
    let _e49 = v_uv_1;
    let _e52 = v_uv_1;
    let _e56 = transition(vec2<f32>(_e49.x, (1f - _e52.y)));
    c_3 = _e56;
    let _e58 = c_3;
    a = clamp(_e58.w, 0f, 1f);
    let _e64 = a;
    if (_e64 > 0.00001f) {
        let _e67 = c_3;
        let _e69 = a;
        let _e71 = (_e67.xyz / vec3(_e69));
        let _e72 = a;
        local = vec4<f32>(_e71.x, _e71.y, _e71.z, _e72);
    } else {
        local = vec4(0f);
    }
    let _e80 = local;
    o_color = _e80;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e34 = o_color;
    return FragmentOutput(_e34);
}

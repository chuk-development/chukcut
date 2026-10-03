// doorway, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: gre
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

fn bgColor(p_4: vec2<f32>, pto: vec2<f32>) -> vec4<f32> {
    var p_5: vec2<f32>;
    var pto_1: vec2<f32>;
    var c_2: vec4<f32> = black;

    p_5 = p_4;
    pto_1 = pto;
    let _e23 = pto_1;
    let _e24 = project(_e23);
    pto_1 = _e24;
    let _e25 = pto_1;
    let _e26 = inBounds(_e25);
    if _e26 {
        {
            let _e27 = c_2;
            let _e28 = pto_1;
            let _e29 = getToColor(_e28);
            let _e30 = reflection;
            let _e33 = pto_1;
            c_2 = (_e27 + mix(black, _e29, vec4((_e30 * mix(1f, 0f, _e33.y)))));
        }
    }
    let _e40 = c_2;
    return _e40;
}

fn transition(p_6: vec2<f32>) -> vec4<f32> {
    var p_7: vec2<f32>;
    var pfr: vec2<f32> = vec2(-1f);
    var pto_2: vec2<f32> = vec2(-1f);
    var middleSlit: f32;
    var local: f32;
    var d: f32;
    var size: f32;

    p_7 = p_6;
    let _e29 = p_7;
    let _e35 = progress;
    middleSlit = ((2f * abs((_e29.x - 0.5f))) - _e35);
    let _e38 = middleSlit;
    if (_e38 > 0f) {
        {
            let _e41 = p_7;
            let _e42 = p_7;
            if (_e42.x > 0.5f) {
                local = -1f;
            } else {
                local = 1f;
            }
            let _e50 = local;
            let _e52 = progress;
            pfr = (_e41 + (_e50 * vec2<f32>((0.5f * _e52), 0f)));
            let _e60 = perspective;
            let _e61 = progress;
            let _e64 = middleSlit;
            d = (1f / (1f + ((_e60 * _e61) * (1f - _e64))));
            let _e71 = pfr;
            let _e73 = d;
            pfr.y = (_e71.y - (_e73 / 2f));
            let _e78 = pfr;
            let _e80 = d;
            pfr.y = (_e78.y * _e80);
            let _e83 = pfr;
            let _e85 = d;
            pfr.y = (_e83.y + (_e85 / 2f));
        }
    }
    let _e90 = depth;
    let _e92 = progress;
    size = mix(1f, _e90, (1f - _e92));
    let _e96 = p_7;
    let _e103 = size;
    let _e104 = size;
    pto_2 = (((_e96 + vec2<f32>(-0.5f, -0.5f)) * vec2<f32>(_e103, _e104)) + vec2<f32>(0.5f, 0.5f));
    let _e111 = pfr;
    let _e112 = inBounds(_e111);
    if _e112 {
        {
            let _e113 = pfr;
            let _e114 = getFromColor(_e113);
            return _e114;
        }
    } else {
        let _e115 = pto_2;
        let _e116 = inBounds(_e115);
        if _e116 {
            {
                let _e117 = pto_2;
                let _e118 = getToColor(_e117);
                return _e118;
            }
        } else {
            {
                let _e119 = p_7;
                let _e120 = pto_2;
                let _e121 = bgColor(_e119, _e120);
                return _e121;
            }
        }
    }
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
    var a: f32;
    var local_1: vec4<f32>;

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
        local_1 = vec4<f32>(_e71.x, _e71.y, _e71.z, _e72);
    } else {
        local_1 = vec4(0f);
    }
    let _e80 = local_1;
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

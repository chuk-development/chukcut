// cube, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
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
var<private> persp_2: f32;
var<private> unzoom: f32;
var<private> reflection: f32;
var<private> floating: f32;

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

fn project(p: vec2<f32>) -> vec2<f32> {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e18 = p_1;
    let _e25 = floating;
    return ((_e18 * vec2<f32>(1f, -1.2f)) + vec2<f32>(0f, (-(_e25) / 100f)));
}

fn inBounds(p_2: vec2<f32>) -> bool {
    var p_3: vec2<f32>;

    p_3 = p_2;
    let _e20 = p_3;
    let _e23 = p_3;
    return (all((vec2(0f) < _e20)) && all((_e23 < vec2(1f))));
}

fn bgColor(p_4: vec2<f32>, pfr: vec2<f32>, pto: vec2<f32>) -> vec4<f32> {
    var p_5: vec2<f32>;
    var pfr_1: vec2<f32>;
    var pto_1: vec2<f32>;
    var c_2: vec4<f32> = vec4<f32>(0f, 0f, 0f, 1f);

    p_5 = p_4;
    pfr_1 = pfr;
    pto_1 = pto;
    let _e28 = pfr_1;
    let _e29 = project(_e28);
    pfr_1 = _e29;
    let _e30 = pfr_1;
    let _e31 = inBounds(_e30);
    if _e31 {
        {
            let _e32 = c_2;
            let _e35 = pfr_1;
            let _e36 = getFromColor(_e35);
            let _e37 = reflection;
            let _e40 = pfr_1;
            c_2 = (_e32 + mix(vec4(0f), _e36, vec4((_e37 * mix(1f, 0f, _e40.y)))));
        }
    }
    let _e47 = pto_1;
    let _e48 = project(_e47);
    pto_1 = _e48;
    let _e49 = pto_1;
    let _e50 = inBounds(_e49);
    if _e50 {
        {
            let _e51 = c_2;
            let _e54 = pto_1;
            let _e55 = getToColor(_e54);
            let _e56 = reflection;
            let _e59 = pto_1;
            c_2 = (_e51 + mix(vec4(0f), _e55, vec4((_e56 * mix(1f, 0f, _e59.y)))));
        }
    }
    let _e66 = c_2;
    return _e66;
}

fn xskew(p_6: vec2<f32>, persp: f32, center: f32) -> vec2<f32> {
    var p_7: vec2<f32>;
    var persp_1: f32;
    var center_1: f32;
    var x: f32;
    var local: f32;
    var local_1: f32;

    p_7 = p_6;
    persp_1 = persp;
    center_1 = center;
    let _e22 = p_7;
    let _e25 = p_7;
    let _e28 = center_1;
    x = mix(_e22.x, (1f - _e25.x), _e28);
    let _e31 = x;
    let _e32 = p_7;
    let _e36 = persp_1;
    let _e39 = x;
    let _e43 = persp_1;
    let _e46 = x;
    let _e52 = center_1;
    let _e60 = center_1;
    let _e64 = center_1;
    if (_e64 < 0.5f) {
        local = 1f;
    } else {
        local = -1f;
    }
    let _e71 = local;
    let _e76 = center_1;
    if (_e76 < 0.5f) {
        local_1 = 0f;
    } else {
        local_1 = 1f;
    }
    let _e82 = local_1;
    return (((vec2<f32>(_e31, ((_e32.y - ((0.5f * (1f - _e36)) * _e39)) / (1f + ((_e43 - 1f) * _e46)))) - vec2<f32>((0.5f - distance(_e52, 0.5f)), 0f)) * vec2<f32>(((0.5f / distance(_e60, 0.5f)) * _e71), 1f)) + vec2<f32>(_e82, 0f));
}

fn transition(op: vec2<f32>) -> vec4<f32> {
    var op_1: vec2<f32>;
    var uz: f32;
    var p_8: vec2<f32>;
    var fromP: vec2<f32>;
    var toP: vec2<f32>;

    op_1 = op;
    let _e18 = unzoom;
    let _e23 = progress;
    uz = ((_e18 * 2f) * (0.5f - distance(0.5f, _e23)));
    let _e28 = uz;
    let _e33 = uz;
    let _e35 = op_1;
    p_8 = (vec2((-(_e28) * 0.5f)) + ((1f + _e33) * _e35));
    let _e40 = p_8;
    let _e41 = progress;
    let _e46 = progress;
    let _e52 = progress;
    let _e54 = persp_2;
    let _e58 = xskew(((_e40 - vec2<f32>(_e41, 0f)) / vec2<f32>((1f - _e46), 1f)), (1f - mix(_e52, 0f, _e54)), 0f);
    fromP = _e58;
    let _e60 = p_8;
    let _e61 = progress;
    let _e65 = progress;
    let _e69 = persp_2;
    let _e72 = xskew((_e60 / vec2<f32>(_e61, 1f)), mix(pow(_e65, 2f), 1f, _e69), 1f);
    toP = _e72;
    let _e74 = fromP;
    let _e75 = inBounds(_e74);
    if _e75 {
        {
            let _e76 = fromP;
            let _e77 = getFromColor(_e76);
            return _e77;
        }
    } else {
        let _e78 = toP;
        let _e79 = inBounds(_e78);
        if _e79 {
            {
                let _e80 = toP;
                let _e81 = getToColor(_e80);
                return _e81;
            }
        }
    }
    let _e82 = op_1;
    let _e83 = fromP;
    let _e84 = toP;
    let _e85 = bgColor(_e82, _e83, _e84);
    return _e85;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
    var a: f32;
    var local_2: vec4<f32>;

    let _e16 = U;
    progress = _e16.state.x;
    let _e20 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e20);
    let _e23 = dims;
    let _e26 = dims;
    ratio = (f32(_e23.x) / f32(max(_e26.y, 1i)));
    let _e35 = U.params[0];
    persp_2 = _e35.x;
    let _e40 = U.params[1];
    unzoom = _e40.x;
    let _e45 = U.params[2];
    reflection = _e45.x;
    let _e50 = U.params[3];
    floating = _e50.x;
    chukcut_init_globals();
    let _e52 = v_uv_1;
    let _e55 = v_uv_1;
    let _e59 = transition(vec2<f32>(_e52.x, (1f - _e55.y)));
    c_3 = _e59;
    let _e61 = c_3;
    a = clamp(_e61.w, 0f, 1f);
    let _e67 = a;
    if (_e67 > 0.00001f) {
        let _e70 = c_3;
        let _e72 = a;
        let _e74 = (_e70.xyz / vec3(_e72));
        let _e75 = a;
        local_2 = vec4<f32>(_e74.x, _e74.y, _e74.z, _e75);
    } else {
        local_2 = vec4(0f);
    }
    let _e83 = local_2;
    o_color = _e83;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e30 = o_color;
    return FragmentOutput(_e30);
}

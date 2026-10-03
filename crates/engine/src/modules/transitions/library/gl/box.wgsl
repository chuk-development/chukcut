// Box, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: lql
// License: MIT
// center:0, left_top:1, left_bottom:2, right_top:3, right_bottom:4
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
var<private> rectIn: i32;
var<private> location: i32;

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
    var local: f32;
    var p: f32;
    var x1_: f32;
    var y1_: f32;
    var x2_: f32;
    var y2_: f32;
    var local_1: f32;
    var local_2: f32;
    var local_3: f32;
    var local_4: f32;
    var in_rect: f32;
    var local_5: f32;

    uv_5 = uv_4;
    let _e16 = rectIn;
    if (_e16 == 1i) {
        let _e20 = progress;
        local = (1f - _e20);
    } else {
        let _e22 = progress;
        local = _e22;
    }
    let _e24 = local;
    p = _e24;
    let _e30 = location;
    if (_e30 == 0i) {
        {
            let _e35 = p;
            let _e37 = (0.5f * (1f - _e35));
            y1_ = _e37;
            x1_ = _e37;
            let _e39 = x1_;
            let _e40 = (1f - _e39);
            y2_ = _e40;
            x2_ = _e40;
        }
    } else {
        {
            let _e41 = location;
            let _e44 = location;
            if ((_e41 == 1i) || (_e44 == 2i)) {
                local_1 = 0f;
            } else {
                let _e50 = p;
                local_1 = (1f - _e50);
            }
            let _e53 = local_1;
            x1_ = _e53;
            let _e54 = location;
            let _e57 = location;
            if ((_e54 == 1i) || (_e57 == 3i)) {
                let _e62 = p;
                local_2 = (1f - _e62);
            } else {
                local_2 = 0f;
            }
            let _e66 = local_2;
            y1_ = _e66;
            let _e67 = location;
            let _e70 = location;
            if ((_e67 == 1i) || (_e70 == 2i)) {
                let _e74 = p;
                local_3 = _e74;
            } else {
                local_3 = 1f;
            }
            let _e77 = local_3;
            x2_ = _e77;
            let _e78 = location;
            let _e81 = location;
            if ((_e78 == 1i) || (_e81 == 3i)) {
                local_4 = 1f;
            } else {
                let _e86 = p;
                local_4 = _e86;
            }
            let _e88 = local_4;
            y2_ = _e88;
        }
    }
    let _e89 = x1_;
    let _e90 = uv_5;
    let _e93 = uv_5;
    let _e95 = x2_;
    let _e98 = y1_;
    let _e99 = uv_5;
    let _e103 = uv_5;
    let _e105 = y2_;
    in_rect = (((step(_e89, _e90.x) * step(_e93.x, _e95)) * step(_e98, _e99.y)) * step(_e103.y, _e105));
    let _e109 = rectIn;
    if (_e109 == 1i) {
        let _e113 = in_rect;
        local_5 = (1f - _e113);
    } else {
        let _e115 = in_rect;
        local_5 = _e115;
    }
    let _e117 = local_5;
    in_rect = _e117;
    let _e118 = uv_5;
    let _e119 = getFromColor(_e118);
    let _e120 = uv_5;
    let _e121 = getToColor(_e120);
    let _e122 = in_rect;
    return mix(_e119, _e121, vec4(_e122));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_6: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    rectIn = i32(_e33.x);
    let _e39 = U.params[1];
    location = i32(_e39.x);
    chukcut_init_globals();
    let _e42 = v_uv_1;
    let _e45 = v_uv_1;
    let _e49 = transition(vec2<f32>(_e42.x, (1f - _e45.y)));
    c_2 = _e49;
    let _e51 = c_2;
    a = clamp(_e51.w, 0f, 1f);
    let _e57 = a;
    if (_e57 > 0.00001f) {
        let _e60 = c_2;
        let _e62 = a;
        let _e64 = (_e60.xyz / vec3(_e62));
        let _e65 = a;
        local_6 = vec4<f32>(_e64.x, _e64.y, _e64.z, _e65);
    } else {
        local_6 = vec4(0f);
    }
    let _e73 = local_6;
    o_color = _e73;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}

// EdgeTransition, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Woohyun Kim
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
var<private> edge_thickness: f32;
var<private> edge_brightness: f32;

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

fn detectEdgeColor(c_2: array<vec3<f32>, 9>) -> vec4<f32> {
    var c_3: array<vec3<f32>, 9>;
    var dx: vec3<f32>;
    var dy: vec3<f32>;
    var delta: f32;

    c_3 = c_2;
    let _e19 = c_3[7];
    let _e22 = c_3[1];
    let _e28 = c_3[2];
    let _e31 = c_3[6];
    let _e37 = c_3[8];
    let _e40 = c_3[0];
    dx = (((2f * abs((_e19 - _e22))) + abs((_e28 - _e31))) + abs((_e37 - _e40)));
    let _e48 = c_3[3];
    let _e51 = c_3[5];
    let _e57 = c_3[6];
    let _e60 = c_3[8];
    let _e66 = c_3[0];
    let _e69 = c_3[2];
    dy = (((2f * abs((_e48 - _e51))) + abs((_e57 - _e60))) + abs((_e66 - _e69)));
    let _e75 = dx;
    let _e76 = dy;
    delta = length(((0.25f * (_e75 + _e76)) * 0.5f));
    let _e83 = edge_brightness;
    let _e84 = delta;
    let _e91 = c_3[4];
    let _e92 = (clamp((_e83 * _e84), 0f, 1f) * _e91);
    return vec4<f32>(_e92.x, _e92.y, _e92.z, 1f);
}

fn getFromEdgeColor(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var c_4: array<vec3<f32>, 9>;
    var i: i32 = 0i;
    var j: i32;
    var color: vec4<f32>;

    uv_5 = uv_4;
    loop {
        let _e19 = i;
        if !((_e19 < 3i)) {
            break;
        }
        j = 0i;
        loop {
            let _e28 = j;
            if !((_e28 < 3i)) {
                break;
            }
            {
                let _e35 = uv_5;
                let _e36 = edge_thickness;
                let _e37 = i;
                let _e40 = j;
                let _e48 = getFromColor((_e35 + (_e36 * vec2<f32>(f32((_e37 - 1i)), f32((_e40 - 1i))))));
                color = _e48;
                let _e51 = i;
                let _e53 = j;
                let _e56 = color;
                c_4[((3i * _e51) + _e53)] = _e56.xyz;
            }
            continuing {
                let _e32 = j;
                j = (_e32 + 1i);
            }
        }
        continuing {
            let _e23 = i;
            i = (_e23 + 1i);
        }
    }
    let _e58 = c_4;
    let _e59 = detectEdgeColor(_e58);
    return _e59;
}

fn getToEdgeColor(uv_6: vec2<f32>) -> vec4<f32> {
    var uv_7: vec2<f32>;
    var c_5: array<vec3<f32>, 9>;
    var i_1: i32 = 0i;
    var j_1: i32;
    var color_1: vec4<f32>;

    uv_7 = uv_6;
    loop {
        let _e19 = i_1;
        if !((_e19 < 3i)) {
            break;
        }
        j_1 = 0i;
        loop {
            let _e28 = j_1;
            if !((_e28 < 3i)) {
                break;
            }
            {
                let _e35 = uv_7;
                let _e36 = edge_thickness;
                let _e37 = i_1;
                let _e40 = j_1;
                let _e48 = getToColor((_e35 + (_e36 * vec2<f32>(f32((_e37 - 1i)), f32((_e40 - 1i))))));
                color_1 = _e48;
                let _e51 = i_1;
                let _e53 = j_1;
                let _e56 = color_1;
                c_5[((3i * _e51) + _e53)] = _e56.xyz;
            }
            continuing {
                let _e32 = j_1;
                j_1 = (_e32 + 1i);
            }
        }
        continuing {
            let _e23 = i_1;
            i_1 = (_e23 + 1i);
        }
    }
    let _e58 = c_5;
    let _e59 = detectEdgeColor(_e58);
    return _e59;
}

fn transition(uv_8: vec2<f32>) -> vec4<f32> {
    var uv_9: vec2<f32>;
    var start: vec4<f32>;
    var end: vec4<f32>;

    uv_9 = uv_8;
    let _e16 = uv_9;
    let _e17 = getFromColor(_e16);
    let _e18 = uv_9;
    let _e19 = getFromEdgeColor(_e18);
    let _e21 = progress;
    start = mix(_e17, _e19, vec4(clamp((2f * _e21), 0f, 1f)));
    let _e29 = uv_9;
    let _e30 = getToEdgeColor(_e29);
    let _e31 = uv_9;
    let _e32 = getToColor(_e31);
    let _e34 = progress;
    end = mix(_e30, _e32, vec4(clamp((2f * (_e34 - 0.5f)), 0f, 1f)));
    let _e44 = start;
    let _e45 = end;
    let _e46 = progress;
    return mix(_e44, _e45, vec4(_e46));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_6: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    edge_thickness = _e33.x;
    let _e38 = U.params[1];
    edge_brightness = _e38.x;
    chukcut_init_globals();
    let _e40 = v_uv_1;
    let _e43 = v_uv_1;
    let _e47 = transition(vec2<f32>(_e40.x, (1f - _e43.y)));
    c_6 = _e47;
    let _e49 = c_6;
    a = clamp(_e49.w, 0f, 1f);
    let _e55 = a;
    if (_e55 > 0.00001f) {
        let _e58 = c_6;
        let _e60 = a;
        let _e62 = (_e58.xyz / vec3(_e60));
        let _e63 = a;
        local = vec4<f32>(_e62.x, _e62.y, _e62.z, _e63);
    } else {
        local = vec4(0f);
    }
    let _e71 = local;
    o_color = _e71;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}

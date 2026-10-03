// StarWipe, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Ben Lucas
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
var<private> border_thickness: f32;
var<private> star_rotation: f32;
var<private> border_color: vec4<f32>;
var<private> star_center: vec2<f32>;

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

fn rotate(v: vec2<f32>, theta: f32) -> vec2<f32> {
    var v_1: vec2<f32>;
    var theta_1: f32;
    var cosTheta: f32;
    var sinTheta: f32;

    v_1 = v;
    theta_1 = theta;
    let _e20 = theta_1;
    cosTheta = cos(_e20);
    let _e23 = theta_1;
    sinTheta = sin(_e23);
    let _e26 = cosTheta;
    let _e27 = v_1;
    let _e30 = sinTheta;
    let _e31 = v_1;
    let _e35 = sinTheta;
    let _e36 = v_1;
    let _e39 = cosTheta;
    let _e40 = v_1;
    return vec2<f32>(((_e26 * _e27.x) - (_e30 * _e31.y)), ((_e35 * _e36.x) + (_e39 * _e40.y)));
}

fn inStar(uv_4: vec2<f32>, center: vec2<f32>, radius: f32) -> bool {
    var uv_5: vec2<f32>;
    var center_1: vec2<f32>;
    var radius_1: f32;
    var uv_centered: vec2<f32>;
    var theta_2: f32;
    var uv_rotated: vec2<f32>;
    var slope: f32 = 0.3f;

    uv_5 = uv_4;
    center_1 = center;
    radius_1 = radius;
    let _e22 = uv_5;
    let _e23 = center_1;
    uv_centered = (_e22 - _e23);
    let _e26 = uv_centered;
    let _e27 = star_rotation;
    let _e30 = rotate(_e26, (_e27 * 1.2566371f));
    uv_centered = _e30;
    let _e31 = uv_centered;
    let _e33 = uv_centered;
    theta_2 = (atan2(_e31.y, _e33.x) + 3.1415927f);
    let _e39 = uv_centered;
    let _e42 = theta_2;
    let _e49 = rotate(_e39, (-1.2566371f * (floor((_e42 / 1.2566371f)) + 0.5f)));
    uv_rotated = _e49;
    let _e53 = uv_rotated;
    if (_e53.y > 0f) {
        {
            let _e57 = radius_1;
            let _e58 = uv_rotated;
            let _e60 = slope;
            let _e63 = uv_rotated;
            return ((_e57 + (_e58.x * _e60)) > _e63.y);
        }
    } else {
        {
            let _e66 = radius_1;
            let _e68 = uv_rotated;
            let _e70 = slope;
            let _e73 = uv_rotated;
            return ((-(_e66) - (_e68.x * _e70)) < _e73.y);
        }
    }
}

fn transition(uv_6: vec2<f32>) -> vec4<f32> {
    var uv_7: vec2<f32>;
    var progressScaled: f32;

    uv_7 = uv_6;
    let _e19 = border_thickness;
    let _e23 = progress;
    let _e25 = border_thickness;
    progressScaled = ((((2f * _e19) + 1f) * _e23) - _e25);
    let _e28 = uv_7;
    let _e29 = star_center;
    let _e30 = progressScaled;
    let _e31 = inStar(_e28, _e29, _e30);
    if _e31 {
        {
            let _e32 = uv_7;
            let _e33 = getToColor(_e32);
            return _e33;
        }
    } else {
        let _e34 = uv_7;
        let _e35 = star_center;
        let _e36 = progressScaled;
        let _e37 = border_thickness;
        let _e39 = inStar(_e34, _e35, (_e36 + _e37));
        if _e39 {
            {
                let _e40 = border_color;
                return _e40;
            }
        } else {
            {
                let _e41 = uv_7;
                let _e42 = getFromColor(_e41);
                return _e42;
            }
        }
    }
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e16 = U;
    progress = _e16.state.x;
    let _e20 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e20);
    let _e23 = dims;
    let _e26 = dims;
    ratio = (f32(_e23.x) / f32(max(_e26.y, 1i)));
    let _e35 = U.params[0];
    border_thickness = _e35.x;
    let _e40 = U.params[1];
    star_rotation = _e40.x;
    let _e45 = U.params[2];
    border_color = _e45;
    let _e49 = U.params[3];
    star_center = _e49.xy;
    chukcut_init_globals();
    let _e51 = v_uv_1;
    let _e54 = v_uv_1;
    let _e58 = transition(vec2<f32>(_e51.x, (1f - _e54.y)));
    c_2 = _e58;
    let _e60 = c_2;
    a = clamp(_e60.w, 0f, 1f);
    let _e66 = a;
    if (_e66 > 0.00001f) {
        let _e69 = c_2;
        let _e71 = a;
        let _e73 = (_e69.xyz / vec3(_e71));
        let _e74 = a;
        local = vec4<f32>(_e73.x, _e73.y, _e73.z, _e74);
    } else {
        local = vec4(0f);
    }
    let _e82 = local;
    o_color = _e82;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e30 = o_color;
    return FragmentOutput(_e30);
}

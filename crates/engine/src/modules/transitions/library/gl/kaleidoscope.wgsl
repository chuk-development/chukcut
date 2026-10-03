// kaleidoscope, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: nwoeanhinnogaehr
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
var<private> speed: f32;
var<private> angle: f32;
var<private> power: f32;

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
    var p: vec2<f32>;
    var q: vec2<f32>;
    var t: f32;
    var i: i32 = 0i;

    uv_5 = uv_4;
    let _e17 = uv_5;
    p = (_e17.xy / vec2(1f));
    let _e24 = p;
    q = _e24;
    let _e26 = progress;
    let _e27 = power;
    let _e29 = speed;
    t = (pow(_e26, _e27) * _e29);
    let _e32 = p;
    p = (_e32 - vec2(0.5f));
    loop {
        let _e38 = i;
        if !((_e38 < 7i)) {
            break;
        }
        {
            let _e45 = t;
            let _e47 = p;
            let _e50 = t;
            let _e52 = p;
            let _e56 = t;
            let _e58 = p;
            let _e61 = t;
            let _e63 = p;
            p = vec2<f32>(((sin(_e45) * _e47.x) + (cos(_e50) * _e52.y)), ((sin(_e56) * _e58.y) - (cos(_e61) * _e63.x)));
            let _e68 = t;
            let _e69 = angle;
            t = (_e68 + _e69);
            let _e71 = p;
            let _e73 = vec2(2f);
            p = abs(((_e71 - (floor((_e71 / _e73)) * _e73)) - vec2(1f)));
        }
        continuing {
            let _e42 = i;
            i = (_e42 + 1i);
        }
    }
    let _e82 = p;
    let _e84 = vec2(1f);
    let _e90 = q;
    let _e91 = getFromColor(_e90);
    let _e92 = q;
    let _e93 = getToColor(_e92);
    let _e94 = progress;
    let _e97 = p;
    let _e98 = getFromColor(_e97);
    let _e99 = p;
    let _e100 = getToColor(_e99);
    let _e101 = progress;
    let _e106 = progress;
    return mix(mix(_e91, _e93, vec4(_e94)), mix(_e98, _e100, vec4(_e101)), vec4((1f - (2f * abs((_e106 - 0.5f))))));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    let _e34 = U.params[0];
    speed = _e34.x;
    let _e39 = U.params[1];
    angle = _e39.x;
    let _e44 = U.params[2];
    power = _e44.x;
    chukcut_init_globals();
    let _e46 = v_uv_1;
    let _e49 = v_uv_1;
    let _e53 = transition(vec2<f32>(_e46.x, (1f - _e49.y)));
    c_2 = _e53;
    let _e55 = c_2;
    a = clamp(_e55.w, 0f, 1f);
    let _e61 = a;
    if (_e61 > 0.00001f) {
        let _e64 = c_2;
        let _e66 = a;
        let _e68 = (_e64.xyz / vec3(_e66));
        let _e69 = a;
        local = vec4<f32>(_e68.x, _e68.y, _e68.z, _e69);
    } else {
        local = vec4(0f);
    }
    let _e77 = local;
    o_color = _e77;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}

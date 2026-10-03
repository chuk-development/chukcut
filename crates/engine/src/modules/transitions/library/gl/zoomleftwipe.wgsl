// ZoomLeftWipe, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Handk
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
var<private> zoom_quickness: f32;
var<private> nQuick: f32;

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

fn zoom(uv_4: vec2<f32>, amount: f32) -> vec2<f32> {
    var uv_5: vec2<f32>;
    var amount_1: f32;

    uv_5 = uv_4;
    amount_1 = amount;
    let _e18 = amount_1;
    if (_e18 < 0.5f) {
        let _e22 = uv_5;
        let _e27 = amount_1;
        return (vec2(0.5f) + ((_e22 - vec2(0.5f)) * (1f - _e27)));
    } else {
        let _e33 = uv_5;
        let _e37 = amount_1;
        return (vec2(0.5f) + ((_e33 - vec2(0.5f)) * _e37));
    }
}

fn transition(uv_6: vec2<f32>) -> vec4<f32> {
    var uv_7: vec2<f32>;
    var c_2: vec4<f32>;
    var p: vec2<f32>;
    var d: vec4<f32>;
    var e: vec4<f32>;
    var f: vec4<f32>;

    uv_7 = uv_6;
    let _e16 = progress;
    if (_e16 < 0.5f) {
        {
            let _e19 = uv_7;
            let _e21 = nQuick;
            let _e22 = progress;
            let _e24 = zoom(_e19, smoothstep(0f, _e21, _e22));
            let _e25 = getFromColor(_e24);
            let _e26 = uv_7;
            let _e27 = getToColor(_e26);
            let _e29 = progress;
            c_2 = mix(_e25, _e27, vec4(step(0.5f, _e29)));
            let _e34 = c_2;
            return _e34;
        }
    } else {
        {
            let _e35 = uv_7;
            p = (_e35.xy / vec2(1f));
            let _e42 = p;
            let _e43 = getFromColor(_e42);
            d = _e43;
            let _e45 = p;
            let _e46 = getToColor(_e45);
            e = _e46;
            let _e48 = d;
            let _e49 = e;
            let _e51 = p;
            let _e54 = progress;
            f = mix(_e48, _e49, vec4(step((1f - _e51.x), ((_e54 - 0.5f) * 2f))));
            let _e63 = f;
            return _e63;
        }
    }
}

fn chukcut_init_globals() {
    let _e14 = zoom_quickness;
    nQuick = clamp(_e14, 0f, 0.5f);
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
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
    zoom_quickness = _e33.x;
    chukcut_init_globals();
    let _e35 = v_uv_1;
    let _e38 = v_uv_1;
    let _e42 = transition(vec2<f32>(_e35.x, (1f - _e38.y)));
    c_3 = _e42;
    let _e44 = c_3;
    a = clamp(_e44.w, 0f, 1f);
    let _e50 = a;
    if (_e50 > 0.00001f) {
        let _e53 = c_3;
        let _e55 = a;
        let _e57 = (_e53.xyz / vec3(_e55));
        let _e58 = a;
        local = vec4<f32>(_e57.x, _e57.y, _e57.z, _e58);
    } else {
        local = vec4(0f);
    }
    let _e66 = local;
    o_color = _e66;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}

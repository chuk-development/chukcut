// undulatingBurnOut, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: pthrasher
// License: MIT
// adapted by gre from https://gist.github.com/pthrasher/8e6226b215548ba12734
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

const M_PI: f32 = 3.1415927f;

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
var<private> ratio_1: f32;
var<private> smoothness: f32;
var<private> center: vec2<f32>;
var<private> color: vec3<f32>;

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

fn quadraticInOut(t: f32) -> f32 {
    var t_1: f32;
    var p: f32;
    var local: f32;

    t_1 = t;
    let _e19 = t_1;
    let _e21 = t_1;
    p = ((2f * _e19) * _e21);
    let _e24 = t_1;
    if (_e24 < 0.5f) {
        let _e27 = p;
        local = _e27;
    } else {
        let _e28 = p;
        let _e31 = t_1;
        local = ((-(_e28) + (4f * _e31)) - 1f);
    }
    let _e37 = local;
    return _e37;
}

fn getGradient(r: f32, dist: f32) -> f32 {
    var r_1: f32;
    var dist_1: f32;
    var d: f32;

    r_1 = r;
    dist_1 = dist;
    let _e20 = r_1;
    let _e21 = dist_1;
    d = (_e20 - _e21);
    let _e24 = smoothness;
    let _e27 = r_1;
    let _e28 = dist_1;
    let _e30 = smoothness;
    let _e38 = d;
    let _e43 = d;
    let _e45 = d;
    return mix(smoothstep(-(_e24), 0f, (_e27 - (_e28 * (1f + _e30)))), (-1f - step(0.005f, _e38)), (step(-0.005f, _e43) * step(_e45, 0.01f)));
}

fn getWave(p_1: vec2<f32>) -> f32 {
    var p_2: vec2<f32>;
    var _p: vec2<f32>;
    var rads: f32;
    var degs: f32;
    var range: vec2<f32> = vec2<f32>(0f, 94.24778f);
    var domain: vec2<f32> = vec2<f32>(0f, 360f);
    var ratio: f32 = 0.2617994f;
    var x: f32;
    var magnitude: f32;
    var offset: f32;
    var ease_degs: f32;
    var deg_wave_pos: f32;

    p_2 = p_1;
    let _e18 = p_2;
    let _e19 = center;
    _p = (_e18 - _e19);
    let _e22 = _p;
    let _e24 = _p;
    rads = atan2(_e22.y, _e24.x);
    let _e28 = rads;
    degs = (degrees(_e28) + 180f);
    let _e49 = degs;
    let _e50 = ratio;
    degs = (_e49 * _e50);
    let _e52 = progress;
    x = _e52;
    let _e58 = x;
    magnitude = mix(0.02f, 0.09f, smoothstep(0f, 1f, _e58));
    let _e66 = x;
    offset = mix(40f, 30f, smoothstep(0f, 1f, _e66));
    let _e70 = degs;
    let _e72 = quadraticInOut(sin(_e70));
    ease_degs = _e72;
    let _e74 = ease_degs;
    let _e75 = magnitude;
    let _e77 = x;
    let _e78 = offset;
    deg_wave_pos = ((_e74 * _e75) * sin((_e77 * _e78)));
    let _e83 = x;
    let _e84 = deg_wave_pos;
    return (_e83 + _e84);
}

fn transition(p_3: vec2<f32>) -> vec4<f32> {
    var p_4: vec2<f32>;
    var dist_2: f32;
    var m: f32;
    var cfrom: vec4<f32>;
    var cto: vec4<f32>;

    p_4 = p_3;
    let _e18 = center;
    let _e19 = p_4;
    dist_2 = distance(_e18, _e19);
    let _e22 = p_4;
    let _e23 = getWave(_e22);
    let _e24 = dist_2;
    let _e25 = getGradient(_e23, _e24);
    m = _e25;
    let _e27 = p_4;
    let _e28 = getFromColor(_e27);
    cfrom = _e28;
    let _e30 = p_4;
    let _e31 = getToColor(_e30);
    cto = _e31;
    let _e33 = cfrom;
    let _e34 = cto;
    let _e35 = m;
    let _e38 = cfrom;
    let _e39 = color;
    let _e48 = m;
    return mix(mix(_e33, _e34, vec4(_e35)), mix(_e38, vec4<f32>(_e39.x, _e39.y, _e39.z, 1f), vec4(0.75f)), vec4(step(_e48, -2f)));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_1: vec4<f32>;

    let _e16 = U;
    progress = _e16.state.x;
    let _e20 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e20);
    let _e23 = dims;
    let _e26 = dims;
    ratio_1 = (f32(_e23.x) / f32(max(_e26.y, 1i)));
    let _e35 = U.params[0];
    smoothness = _e35.x;
    let _e40 = U.params[1];
    center = _e40.xy;
    let _e45 = U.params[2];
    color = _e45.xyz;
    chukcut_init_globals();
    let _e47 = v_uv_1;
    let _e50 = v_uv_1;
    let _e54 = transition(vec2<f32>(_e47.x, (1f - _e50.y)));
    c_2 = _e54;
    let _e56 = c_2;
    a = clamp(_e56.w, 0f, 1f);
    let _e62 = a;
    if (_e62 > 0.00001f) {
        let _e65 = c_2;
        let _e67 = a;
        let _e69 = (_e65.xyz / vec3(_e67));
        let _e70 = a;
        local_1 = vec4<f32>(_e69.x, _e69.y, _e69.z, _e70);
    } else {
        local_1 = vec4(0f);
    }
    let _e78 = local_1;
    o_color = _e78;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e30 = o_color;
    return FragmentOutput(_e30);
}

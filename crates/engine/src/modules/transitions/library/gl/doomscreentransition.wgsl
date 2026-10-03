// DoomScreenTransition, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Zeh Fernando
// License: MIT
// Transition parameters --------
// Number of total bars/columns
// Multiplier for speed ratio. 0 = no variation when going down, higher = some elements go much faster
// Further variations in speed. 0 = no noise, 1 = super noisy (ignore frequency)
// Speed variation horizontally. the bigger the value, the shorter the waves
// How much the bars seem to "run" from the middle of the screen first (sticking to the sides). 0 = no drip, 1 = curved drip
// The code proper --------
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
var<private> bars: i32;
var<private> amplitude: f32;
var<private> noise: f32;
var<private> frequency: f32;
var<private> dripScale: f32;

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

fn rand(num: i32) -> f32 {
    var num_1: i32;

    num_1 = num;
    let _e19 = num_1;
    let _e22 = (f32(_e19) * 67123.31f);
    let _e28 = num_1;
    let _e34 = num_1;
    return fract((((_e22 - (floor((_e22 / 12f)) * 12f)) * sin((f32(_e28) * 10.3f))) * cos(f32(_e34))));
}

fn wave(num_2: i32) -> f32 {
    var num_3: i32;
    var fn_: f32;

    num_3 = num_2;
    let _e19 = num_3;
    let _e21 = frequency;
    let _e25 = bars;
    fn_ = (((f32(_e19) * _e21) * 0.1f) * f32(_e25));
    let _e29 = fn_;
    let _e33 = fn_;
    let _e38 = fn_;
    return ((((cos((_e29 * 0.5f)) * cos((_e33 * 0.13f))) * sin(((_e38 + 10f) * 0.3f))) / 2f) + 0.5f);
}

fn drip(num_4: i32) -> f32 {
    var num_5: i32;

    num_5 = num_4;
    let _e19 = num_5;
    let _e21 = bars;
    let _e29 = dripScale;
    return (sin(((f32(_e19) / f32((_e21 - 1i))) * 3.141592f)) * _e29);
}

fn pos(num_6: i32) -> f32 {
    var num_7: i32;
    var local: f32;
    var local_1: f32;

    num_7 = num_6;
    let _e19 = noise;
    if (_e19 == 0f) {
        let _e22 = num_7;
        let _e23 = wave(_e22);
        local = _e23;
    } else {
        let _e24 = num_7;
        let _e25 = wave(_e24);
        let _e26 = num_7;
        let _e27 = rand(_e26);
        let _e28 = noise;
        local = mix(_e25, _e27, _e28);
    }
    let _e31 = local;
    let _e32 = dripScale;
    if (_e32 == 0f) {
        local_1 = 0f;
    } else {
        let _e36 = num_7;
        let _e37 = drip(_e36);
        local_1 = _e37;
    }
    let _e39 = local_1;
    return (_e31 + _e39);
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var bar: i32;
    var scale: f32;
    var phase: f32;
    var posY: f32;
    var p: vec2<f32>;
    var c_2: vec4<f32>;

    uv_5 = uv_4;
    let _e19 = uv_5;
    let _e21 = bars;
    bar = i32((_e19.x * f32(_e21)));
    let _e27 = bar;
    let _e28 = pos(_e27);
    let _e29 = amplitude;
    scale = (1f + (_e28 * _e29));
    let _e33 = progress;
    let _e34 = scale;
    phase = (_e33 * _e34);
    let _e37 = uv_5;
    posY = (_e37.y / 1f);
    let _e45 = phase;
    let _e46 = posY;
    if ((_e45 + _e46) < 1f) {
        {
            let _e50 = uv_5;
            let _e52 = uv_5;
            let _e57 = phase;
            p = (vec2<f32>(_e50.x, (_e52.y + mix(0f, 1f, _e57))) / vec2(1f));
            let _e65 = p;
            let _e66 = getFromColor(_e65);
            c_2 = _e66;
        }
    } else {
        {
            let _e67 = uv_5;
            p = (_e67.xy / vec2(1f));
            let _e73 = p;
            let _e74 = getToColor(_e73);
            c_2 = _e74;
        }
    }
    let _e75 = c_2;
    return _e75;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
    var a: f32;
    var local_2: vec4<f32>;

    let _e17 = U;
    progress = _e17.state.x;
    let _e21 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e21);
    let _e24 = dims;
    let _e27 = dims;
    ratio = (f32(_e24.x) / f32(max(_e27.y, 1i)));
    let _e36 = U.params[0];
    bars = i32(_e36.x);
    let _e42 = U.params[1];
    amplitude = _e42.x;
    let _e47 = U.params[2];
    noise = _e47.x;
    let _e52 = U.params[3];
    frequency = _e52.x;
    let _e57 = U.params[4];
    dripScale = _e57.x;
    chukcut_init_globals();
    let _e59 = v_uv_1;
    let _e62 = v_uv_1;
    let _e66 = transition(vec2<f32>(_e59.x, (1f - _e62.y)));
    c_3 = _e66;
    let _e68 = c_3;
    a = clamp(_e68.w, 0f, 1f);
    let _e74 = a;
    if (_e74 > 0.00001f) {
        let _e77 = c_3;
        let _e79 = a;
        let _e81 = (_e77.xyz / vec3(_e79));
        let _e82 = a;
        local_2 = vec4<f32>(_e81.x, _e81.y, _e81.z, _e82);
    } else {
        local_2 = vec4(0f);
    }
    let _e90 = local_2;
    o_color = _e90;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e32 = o_color;
    return FragmentOutput(_e32);
}

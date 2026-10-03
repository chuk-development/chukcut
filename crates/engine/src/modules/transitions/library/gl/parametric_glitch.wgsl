// parametric_glitch, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Yoni Maltsman @friendlyspinach
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
var<private> ampx: f32;
var<private> ampy: f32;

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
    var from_: vec4<f32>;
    var to: vec4<f32>;
    var r: f32;
    var g: f32;
    var b: f32;
    var sphere: f32;
    var spiralX: f32;
    var spiralY: f32;
    var st: vec2<f32>;
    var diff: vec2<f32>;

    uv_5 = uv_4;
    let _e16 = uv_5;
    let _e17 = getFromColor(_e16);
    from_ = _e17;
    let _e19 = uv_5;
    let _e20 = getToColor(_e19);
    to = _e20;
    let _e22 = from_;
    r = _e22.x;
    let _e25 = from_;
    g = _e25.y;
    let _e28 = from_;
    b = _e28.z;
    let _e31 = r;
    let _e32 = r;
    let _e34 = g;
    let _e35 = g;
    let _e38 = b;
    let _e39 = b;
    sphere = ((((_e31 * _e32) + (_e34 * _e35)) + (_e38 * _e39)) - 1f);
    let _e45 = sphere;
    let _e46 = uv_5;
    let _e48 = progress;
    spiralX = cos((_e45 - (_e46.x / (_e48 + 0.01f))));
    let _e55 = sphere;
    let _e56 = uv_5;
    let _e58 = progress;
    spiralY = sin((_e55 - (_e56.y / (_e58 + 0.01f))));
    let _e65 = uv_5;
    st = _e65;
    let _e68 = ampx;
    let _e69 = st;
    let _e72 = spiralX;
    st.x = fract(((_e68 * _e69.x) * _e72));
    let _e76 = ampy;
    let _e77 = st;
    let _e80 = spiralY;
    st.y = fract(((_e76 * _e77.y) * _e80));
    let _e83 = uv_5;
    let _e84 = st;
    diff = (_e83 - _e84);
    let _e87 = uv_5;
    let _e88 = progress;
    let _e89 = diff;
    let _e92 = getFromColor((_e87 + (_e88 * _e89)));
    from_ = _e92;
    let _e93 = from_;
    let _e94 = to;
    let _e95 = progress;
    return mix(_e93, _e94, vec4(_e95));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
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
    ampx = _e33.x;
    let _e38 = U.params[1];
    ampy = _e38.x;
    chukcut_init_globals();
    let _e40 = v_uv_1;
    let _e43 = v_uv_1;
    let _e47 = transition(vec2<f32>(_e40.x, (1f - _e43.y)));
    c_2 = _e47;
    let _e49 = c_2;
    a = clamp(_e49.w, 0f, 1f);
    let _e55 = a;
    if (_e55 > 0.00001f) {
        let _e58 = c_2;
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

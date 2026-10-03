// LinearBlur, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
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

const passes: i32 = 6i;

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
var<private> intensity: f32;

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
    var c1_: vec4<f32> = vec4(0f);
    var c2_: vec4<f32> = vec4(0f);
    var disp: f32;
    var xi: i32 = 0i;
    var x: f32;
    var yi: i32;
    var y: f32;
    var v: vec2<f32>;
    var d: f32;

    uv_5 = uv_4;
    let _e22 = intensity;
    let _e25 = progress;
    disp = (_e22 * (0.5f - distance(0.5f, _e25)));
    loop {
        let _e32 = xi;
        if !((_e32 < passes)) {
            break;
        }
        {
            let _e38 = xi;
            x = ((f32(_e38) / 6f) - 0.5f);
            yi = 0i;
            loop {
                let _e48 = yi;
                if !((_e48 < passes)) {
                    break;
                }
                {
                    let _e54 = yi;
                    y = ((f32(_e54) / 6f) - 0.5f);
                    let _e62 = x;
                    let _e63 = y;
                    v = vec2<f32>(_e62, _e63);
                    let _e66 = disp;
                    d = _e66;
                    let _e68 = c1_;
                    let _e69 = uv_5;
                    let _e70 = d;
                    let _e71 = v;
                    let _e74 = getFromColor((_e69 + (_e70 * _e71)));
                    c1_ = (_e68 + _e74);
                    let _e76 = c2_;
                    let _e77 = uv_5;
                    let _e78 = d;
                    let _e79 = v;
                    let _e82 = getToColor((_e77 + (_e78 * _e79)));
                    c2_ = (_e76 + _e82);
                }
                continuing {
                    let _e51 = yi;
                    yi = (_e51 + 1i);
                }
            }
        }
        continuing {
            let _e35 = xi;
            xi = (_e35 + 1i);
        }
    }
    let _e84 = c1_;
    c1_ = (_e84 / vec4(36f));
    let _e91 = c2_;
    c2_ = (_e91 / vec4(36f));
    let _e98 = c1_;
    let _e99 = c2_;
    let _e100 = progress;
    return mix(_e98, _e99, vec4(_e100));
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
    intensity = _e33.x;
    chukcut_init_globals();
    let _e35 = v_uv_1;
    let _e38 = v_uv_1;
    let _e42 = transition(vec2<f32>(_e35.x, (1f - _e38.y)));
    c_2 = _e42;
    let _e44 = c_2;
    a = clamp(_e44.w, 0f, 1f);
    let _e50 = a;
    if (_e50 > 0.00001f) {
        let _e53 = c_2;
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

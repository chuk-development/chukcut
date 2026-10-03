// FilmBurn, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Anastasia Dunbar
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
var<private> Seed: f32;

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

fn sigmoid(x: f32, a: f32) -> f32 {
    var x_1: f32;
    var a_1: f32;
    var b: f32;

    x_1 = x;
    a_1 = a;
    let _e17 = x_1;
    let _e20 = a_1;
    b = (pow((_e17 * 2f), _e20) / 2f);
    let _e25 = x_1;
    if (_e25 > 0.5f) {
        {
            let _e30 = x_1;
            let _e34 = a_1;
            b = (1f - (pow((2f - (_e30 * 2f)), _e34) / 2f));
        }
    }
    let _e39 = b;
    return _e39;
}

fn rand(co: f32) -> f32 {
    var co_1: f32;

    co_1 = co;
    let _e15 = co_1;
    let _e18 = Seed;
    return fract((sin(((_e15 * 24.9898f) + _e18)) * 43758.547f));
}

fn rand_1(co_2: vec2<f32>) -> f32 {
    var co_3: vec2<f32>;

    co_3 = co_2;
    let _e15 = co_3;
    return fract((sin(dot(_e15.xy, vec2<f32>(12.9898f, 78.233f))) * 43758.547f));
}

fn apow(a_2: f32, b_1: f32) -> f32 {
    var a_3: f32;
    var b_2: f32;

    a_3 = a_2;
    b_2 = b_1;
    let _e17 = a_3;
    let _e19 = b_2;
    let _e21 = b_2;
    return (pow(abs(_e17), _e19) * sign(_e21));
}

fn pow3_(a_4: vec3<f32>, b_3: vec3<f32>) -> vec3<f32> {
    var a_5: vec3<f32>;
    var b_4: vec3<f32>;

    a_5 = a_4;
    b_4 = b_3;
    let _e17 = a_5;
    let _e19 = b_4;
    let _e21 = apow(_e17.x, _e19.x);
    let _e22 = a_5;
    let _e24 = b_4;
    let _e26 = apow(_e22.y, _e24.y);
    let _e27 = a_5;
    let _e29 = b_4;
    let _e31 = apow(_e27.z, _e29.z);
    return vec3<f32>(_e21, _e26, _e31);
}

fn smooth_mix(a_6: f32, b_5: f32, c_2: f32) -> f32 {
    var a_7: f32;
    var b_6: f32;
    var c_3: f32;

    a_7 = a_6;
    b_6 = b_5;
    c_3 = c_2;
    let _e19 = a_7;
    let _e20 = b_6;
    let _e21 = c_3;
    let _e23 = sigmoid(_e21, 2f);
    return mix(_e19, _e20, _e23);
}

fn random(co_4: vec2<f32>, shft: f32) -> f32 {
    var co_5: vec2<f32>;
    var shft_1: f32;

    co_5 = co_4;
    shft_1 = shft;
    let _e17 = co_5;
    co_5 = (_e17 + vec2(10f));
    let _e21 = co_5;
    let _e24 = shft_1;
    let _e30 = Seed;
    let _e38 = co_5;
    let _e41 = shft_1;
    let _e49 = Seed;
    let _e57 = shft_1;
    let _e59 = smooth_mix(fract((sin(dot(_e21.xy, vec2<f32>((12.9898f + (floor(_e24) * 0.5f)), (78.233f + _e30)))) * 43758.547f)), fract((sin(dot(_e38.xy, vec2<f32>((12.9898f + (floor((_e41 + 1f)) * 0.5f)), (78.233f + _e49)))) * 43758.547f)), fract(_e57));
    return _e59;
}

fn smooth_random(co_6: vec2<f32>, shft_2: f32) -> f32 {
    var co_7: vec2<f32>;
    var shft_3: f32;

    co_7 = co_6;
    shft_3 = shft_2;
    let _e17 = co_7;
    let _e19 = shft_3;
    let _e20 = random(floor(_e17), _e19);
    let _e21 = co_7;
    let _e27 = shft_3;
    let _e28 = random(floor((_e21 + vec2<f32>(1f, 0f))), _e27);
    let _e29 = co_7;
    let _e32 = smooth_mix(_e20, _e28, fract(_e29.x));
    let _e33 = co_7;
    let _e39 = shft_3;
    let _e40 = random(floor((_e33 + vec2<f32>(0f, 1f))), _e39);
    let _e41 = co_7;
    let _e47 = shft_3;
    let _e48 = random(floor((_e41 + vec2<f32>(1f, 1f))), _e47);
    let _e49 = co_7;
    let _e52 = smooth_mix(_e40, _e48, fract(_e49.x));
    let _e53 = co_7;
    let _e56 = smooth_mix(_e32, _e52, fract(_e53.y));
    return _e56;
}

fn chukcut_texture(p: vec2<f32>) -> vec4<f32> {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e15 = p_1;
    let _e16 = getFromColor(_e15);
    let _e17 = p_1;
    let _e18 = getToColor(_e17);
    let _e19 = progress;
    let _e21 = sigmoid(_e19, 10f);
    return mix(_e16, _e18, vec4(_e21));
}

fn transition(p_2: vec2<f32>) -> vec4<f32> {
    var p_3: vec2<f32>;
    var f: vec3<f32> = vec3(0f);
    var i: f32 = 0f;
    var blurred_image: vec4<f32> = vec4(0f);
    var bluramount: f32;
    var i_1: f32 = 0f;
    var q: vec2<f32>;
    var uv2_: vec2<f32>;

    p_3 = p_2;
    loop {
        let _e20 = i;
        if !((_e20 < 13f)) {
            break;
        }
        {
            let _e27 = f;
            let _e28 = p_3;
            let _e30 = i;
            let _e31 = rand(_e30);
            let _e35 = progress;
            let _e39 = i;
            let _e42 = rand((_e39 + 1.43f));
            let _e45 = p_3;
            let _e47 = i;
            let _e50 = rand((_e47 + 4.4f));
            let _e54 = progress;
            let _e58 = i;
            let _e61 = rand((_e58 + 2.4f));
            f = (_e27 + vec3((sin(((((_e28.x * _e31) * 6f) + (_e35 * 8f)) + _e42)) * sin(((((_e45.y * _e50) * 6f) + (_e54 * 6f)) + _e61)))));
            let _e67 = f;
            let _e69 = p_3;
            let _e70 = progress;
            let _e74 = i;
            let _e77 = smooth_random(vec2((_e70 * 1.3f)), (_e74 + 1f));
            let _e78 = progress;
            let _e82 = i;
            let _e85 = smooth_random(vec2((_e78 * 0.5f)), (_e82 + 6.25f));
            let _e91 = i;
            let _e92 = rand(_e91);
            f = (_e67 + vec3((1f - clamp((length((_e69 - vec2<f32>(_e77, _e85))) * mix(20f, 70f, _e92)), 0f, 1f))));
        }
        continuing {
            let _e24 = i;
            i = (_e24 + 1f);
        }
    }
    let _e101 = f;
    f = (_e101 + vec3(4f));
    let _e105 = f;
    f = (_e105 / vec3(11f));
    let _e109 = f;
    let _e117 = progress;
    let _e124 = pow3_((_e109 * vec3<f32>(1f, 0.7f, 0.6f)), vec3<f32>(1f, (2f - sin((_e117 * 3.1415927f))), 1.3f));
    f = _e124;
    let _e125 = f;
    let _e126 = progress;
    f = (_e125 * sin((_e126 * 3.1415927f)));
    let _e131 = p_3;
    p_3 = (_e131 - vec2(0.5f));
    let _e135 = p_3;
    let _e137 = progress;
    let _e142 = smooth_random(vec2((_e137 * 5f)), 6.3f);
    let _e143 = progress;
    p_3 = (_e135 * (1f + ((_e142 * sin((_e143 * 3.1415927f))) * 0.05f)));
    let _e152 = p_3;
    p_3 = (_e152 + vec2(0.5f));
    let _e159 = progress;
    bluramount = (sin((_e159 * 3.1415927f)) * 0.03f);
    loop {
        let _e168 = i_1;
        if !((_e168 < 50f)) {
            break;
        }
        {
            let _e175 = i_1;
            let _e182 = i_1;
            let _e190 = i_1;
            let _e191 = p_3;
            let _e193 = p_3;
            let _e197 = rand_1(vec2<f32>(_e190, (_e191.x + _e193.y)));
            let _e198 = bluramount;
            q = (vec2<f32>(cos(degrees(((_e175 / 50f) * 360f))), sin(degrees(((_e182 / 50f) * 360f)))) * (_e197 + _e198));
            let _e202 = p_3;
            let _e203 = q;
            let _e204 = bluramount;
            uv2_ = (_e202 + (_e203 * _e204));
            let _e208 = blurred_image;
            let _e209 = uv2_;
            let _e210 = chukcut_texture(_e209);
            blurred_image = (_e208 + _e210);
        }
        continuing {
            let _e172 = i_1;
            i_1 = (_e172 + 1f);
        }
    }
    let _e212 = blurred_image;
    blurred_image = (_e212 / vec4(50f));
    let _e216 = blurred_image;
    let _e217 = f;
    return (_e216 + vec4<f32>(_e217.x, _e217.y, _e217.z, 0f));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_4: vec4<f32>;
    var a_8: f32;
    var local: vec4<f32>;

    let _e13 = U;
    progress = _e13.state.x;
    let _e17 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e17);
    let _e20 = dims;
    let _e23 = dims;
    ratio = (f32(_e20.x) / f32(max(_e23.y, 1i)));
    let _e32 = U.params[0];
    Seed = _e32.x;
    chukcut_init_globals();
    let _e34 = v_uv_1;
    let _e37 = v_uv_1;
    let _e41 = transition(vec2<f32>(_e34.x, (1f - _e37.y)));
    c_4 = _e41;
    let _e43 = c_4;
    a_8 = clamp(_e43.w, 0f, 1f);
    let _e49 = a_8;
    if (_e49 > 0.00001f) {
        let _e52 = c_4;
        let _e54 = a_8;
        let _e56 = (_e52.xyz / vec3(_e54));
        let _e57 = a_8;
        local = vec4<f32>(_e56.x, _e56.y, _e56.z, _e57);
    } else {
        local = vec4(0f);
    }
    let _e65 = local;
    o_color = _e65;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e24 = o_color;
    return FragmentOutput(_e24);
}

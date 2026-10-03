// fragment, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: lbl
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
var<uniform> U_1: GlBlock;
@group(1) @binding(0) 
var u_from: texture_2d<f32>;
@group(1) @binding(1) 
var u_to: texture_2d<f32>;
@group(1) @binding(2) 
var u_sampler: sampler;
var<private> progress: f32;
var<private> ratio: f32;

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

fn random(par: vec2<f32>) -> f32 {
    var par_1: vec2<f32>;

    par_1 = par;
    let _e14 = par_1;
    return fract((sin(dot(_e14.xy, vec2<f32>(12.9898f, 78.233f))) * 43758.547f));
}

fn random2_(par_2: vec2<f32>) -> vec2<f32> {
    var par_3: vec2<f32>;
    var rand: f32;

    par_3 = par_2;
    let _e14 = par_3;
    let _e15 = random(_e14);
    rand = _e15;
    let _e17 = rand;
    let _e18 = par_3;
    let _e19 = rand;
    let _e22 = random((_e18 + vec2(_e19)));
    return vec2<f32>(_e17, _e22);
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var duration: f32 = 8f;
    var time: f32;
    var point: array<vec2<f32>, 10>;
    var i: i32 = 0i;
    var col: vec4<f32>;
    var i_1: i32 = 0i;
    var dir: vec2<f32>;
    var v: f32;
    var ofst: vec2<f32>;
    var U: vec2<f32>;
    var dist_i: f32;
    var closest: bool;
    var j: i32;

    uv_5 = uv_4;
    let _e14 = progress;
    if (_e14 <= 0f) {
        let _e17 = uv_5;
        let _e18 = getFromColor(_e17);
        return _e18;
    }
    let _e19 = progress;
    if (_e19 >= 1f) {
        let _e22 = uv_5;
        let _e23 = getToColor(_e22);
        return _e23;
    }
    let _e26 = progress;
    let _e27 = duration;
    time = (_e26 * _e27);
    loop {
        let _e33 = i;
        if !((_e33 < 10i)) {
            break;
        }
        {
            let _e40 = i;
            let _e42 = i;
            let _e45 = random2_(vec2(f32(_e42)));
            point[_e40] = _e45;
        }
        continuing {
            let _e37 = i;
            i = (_e37 + 1i);
        }
    }
    let _e46 = uv_5;
    let _e47 = getToColor(_e46);
    col = _e47;
    loop {
        let _e51 = i_1;
        if !((_e51 < 10i)) {
            break;
        }
        {
            let _e58 = i_1;
            let _e60 = i_1;
            let _e65 = random2_(vec2<f32>(f32(_e58), (f32(_e60) + 11f)));
            dir = normalize(_e65);
            let _e69 = dir;
            let _e70 = random(_e69);
            v = ((1f + (_e70 * 0.5f)) * 0.2f);
            let _e77 = dir;
            let _e78 = time;
            let _e82 = duration;
            let _e85 = v;
            ofst = ((_e77 * clamp((_e78 - 0.5f), 0f, _e82)) * _e85);
            let _e88 = uv_5;
            let _e89 = ofst;
            U = (_e88 - _e89);
            let _e92 = U;
            let _e96 = U;
            let _e101 = U;
            let _e106 = U;
            if ((((_e92.x < 0f) || (_e96.x > 1f)) || (_e101.y < 0f)) || (_e106.y > 1f)) {
                continue;
            }
            let _e111 = U;
            let _e112 = i_1;
            let _e114 = point[_e112];
            dist_i = distance(_e111, _e114);
            closest = true;
            j = 0i;
            loop {
                let _e121 = j;
                if !((_e121 < 10i)) {
                    break;
                }
                {
                    let _e128 = U;
                    let _e129 = j;
                    let _e131 = point[_e129];
                    let _e133 = dist_i;
                    if (distance(_e128, _e131) < _e133) {
                        {
                            closest = false;
                            break;
                        }
                    }
                }
                continuing {
                    let _e125 = j;
                    j = (_e125 + 1i);
                }
            }
            let _e136 = closest;
            if _e136 {
                {
                    let _e137 = U;
                    let _e138 = getFromColor(_e137);
                    col = _e138;
                    break;
                }
            }
        }
        continuing {
            let _e55 = i_1;
            i_1 = (_e55 + 1i);
        }
    }
    let _e139 = col;
    return _e139;
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e12 = U_1;
    progress = _e12.state.x;
    let _e16 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e16);
    let _e19 = dims;
    let _e22 = dims;
    ratio = (f32(_e19.x) / f32(max(_e22.y, 1i)));
    chukcut_init_globals();
    let _e28 = v_uv_1;
    let _e31 = v_uv_1;
    let _e35 = transition(vec2<f32>(_e28.x, (1f - _e31.y)));
    c_2 = _e35;
    let _e37 = c_2;
    a = clamp(_e37.w, 0f, 1f);
    let _e43 = a;
    if (_e43 > 0.00001f) {
        let _e46 = c_2;
        let _e48 = a;
        let _e50 = (_e46.xyz / vec3(_e48));
        let _e51 = a;
        local = vec4<f32>(_e50.x, _e50.y, _e50.z, _e51);
    } else {
        local = vec4(0f);
    }
    let _e59 = local;
    o_color = _e59;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e22 = o_color;
    return FragmentOutput(_e22);
}

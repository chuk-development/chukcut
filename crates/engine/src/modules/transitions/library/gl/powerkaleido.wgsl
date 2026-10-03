// powerKaleido, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Boundless
// License: MIT
// Name: Power Kaleido
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

const rad: f32 = 120f;
const deg: f32 = 2.0943952f;

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
var<private> scale: f32;
var<private> z: f32;
var<private> speed: f32;
var<private> dist: f32;

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

fn refl(p: vec2<f32>, o: vec2<f32>, n: vec2<f32>) -> vec2<f32> {
    var p_1: vec2<f32>;
    var o_1: vec2<f32>;
    var n_1: vec2<f32>;

    p_1 = p;
    o_1 = o;
    n_1 = n;
    let _e25 = o_1;
    let _e28 = n_1;
    let _e30 = p_1;
    let _e31 = o_1;
    let _e33 = n_1;
    let _e37 = p_1;
    return (((2f * _e25) + ((2f * _e28) * dot((_e30 - _e31), _e33))) - _e37);
}

fn rot(p_2: vec2<f32>, o_2: vec2<f32>, a: f32) -> vec2<f32> {
    var p_3: vec2<f32>;
    var o_3: vec2<f32>;
    var a_1: f32;
    var s: f32;
    var c_2: f32;

    p_3 = p_2;
    o_3 = o_2;
    a_1 = a;
    let _e24 = a_1;
    s = sin(_e24);
    let _e27 = a_1;
    c_2 = cos(_e27);
    let _e30 = o_3;
    let _e31 = c_2;
    let _e32 = s;
    let _e34 = s;
    let _e35 = c_2;
    let _e39 = p_3;
    let _e40 = o_3;
    return (_e30 + (mat2x2<f32>(vec2<f32>(_e31, -(_e32)), vec2<f32>(_e34, _e35)) * (_e39 - _e40)));
}

fn mainImage(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var uv0_: vec2<f32>;
    var theta: f32;
    var iter: i32 = 0i;
    var i: f32;
    var local: f32;
    var ts: f32;
    var uvMix: vec2<f32>;
    var color: vec4<f32>;

    uv_5 = uv_4;
    let _e20 = uv_5;
    uv0_ = _e20;
    let _e22 = uv_5;
    uv_5 = (_e22 - vec2(0.5f));
    let _e27 = uv_5;
    let _e29 = ratio;
    uv_5.x = (_e27.x * _e29);
    let _e31 = uv_5;
    let _e32 = z;
    uv_5 = (_e31 * _e32);
    let _e34 = uv_5;
    let _e37 = progress;
    let _e38 = speed;
    let _e40 = rot(_e34, vec2(0f), (_e37 * _e38));
    uv_5 = _e40;
    let _e41 = progress;
    theta = ((_e41 * 6f) + 6.2831855f);
    loop {
        let _e51 = iter;
        if !((_e51 < 10i)) {
            break;
        }
        {
            i = 0f;
            loop {
                let _e60 = i;
                if !((_e60 < 6.2831855f)) {
                    break;
                }
                {
                    let _e68 = i;
                    if (sign(asin(cos(_e68))) == 1f) {
                        local = 1f;
                    } else {
                        local = 0f;
                    }
                    let _e77 = local;
                    ts = _e77;
                    let _e79 = ts;
                    let _e82 = uv_5;
                    let _e84 = dist;
                    let _e85 = i;
                    let _e89 = i;
                    let _e91 = uv_5;
                    let _e93 = dist;
                    let _e94 = i;
                    let _e101 = ts;
                    let _e104 = uv_5;
                    let _e106 = dist;
                    let _e107 = i;
                    let _e111 = i;
                    let _e113 = uv_5;
                    let _e115 = dist;
                    let _e116 = i;
                    if (((_e79 == 1f) && ((_e82.y - (_e84 * cos(_e85))) > (tan(_e89) * (_e91.x + (_e93 * sin(_e94)))))) || ((_e101 == 0f) && ((_e104.y - (_e106 * cos(_e107))) < (tan(_e111) * (_e113.x + (_e115 * sin(_e116))))))) {
                        {
                            let _e124 = uv_5;
                            let _e126 = i;
                            let _e128 = dist;
                            let _e133 = uv_5;
                            let _e135 = i;
                            let _e137 = dist;
                            let _e146 = i;
                            let _e148 = i;
                            let _e151 = refl(vec2<f32>((_e124.x + ((sin(_e126) * _e128) * 2f)), (_e133.y - ((cos(_e135) * _e137) * 2f))), vec2<f32>(0f, 0f), vec2<f32>(cos(_e146), sin(_e148)));
                            uv_5 = _e151;
                        }
                    }
                }
                continuing {
                    let _e66 = i;
                    i = (_e66 + deg);
                }
            }
        }
        continuing {
            let _e55 = iter;
            iter = (_e55 + 1i);
        }
    }
    let _e152 = uv_5;
    uv_5 = (_e152 + vec2(0.5f));
    let _e156 = uv_5;
    let _e159 = progress;
    let _e160 = speed;
    let _e163 = rot(_e156, vec2(0.5f), (_e159 * -(_e160)));
    uv_5 = _e163;
    let _e164 = uv_5;
    uv_5 = (_e164 - vec2(0.5f));
    let _e169 = uv_5;
    let _e171 = ratio;
    uv_5.x = (_e169.x / _e171);
    let _e173 = uv_5;
    uv_5 = (_e173 + vec2(0.5f));
    let _e178 = uv_5;
    let _e182 = uv_5;
    uv_5 = (2f * abs(((_e178 / vec2(2f)) - floor(((_e182 / vec2(2f)) + vec2(0.5f))))));
    let _e193 = uv_5;
    let _e194 = uv0_;
    let _e195 = progress;
    uvMix = mix(_e193, _e194, vec2(((cos(((_e195 * 3.1415927f) * 2f)) / 2f) + 0.5f)));
    let _e208 = uvMix;
    let _e209 = getFromColor(_e208);
    let _e210 = uvMix;
    let _e211 = getToColor(_e210);
    let _e212 = progress;
    color = mix(_e209, _e211, vec4(((cos(((_e212 - 1f) * 3.1415927f)) / 2f) + 0.5f)));
    let _e225 = color;
    return _e225;
}

fn transition(uv_6: vec2<f32>) -> vec4<f32> {
    var uv_7: vec2<f32>;
    var color_1: vec4<f32>;

    uv_7 = uv_6;
    let _e20 = uv_7;
    let _e21 = mainImage(_e20);
    color_1 = _e21;
    let _e23 = color_1;
    return _e23;
}

fn chukcut_init_globals() {
    let _e18 = scale;
    dist = (_e18 / 10f);
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
    var a_2: f32;
    var local_1: vec4<f32>;

    let _e18 = U;
    progress = _e18.state.x;
    let _e22 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e22);
    let _e25 = dims;
    let _e28 = dims;
    ratio = (f32(_e25.x) / f32(max(_e28.y, 1i)));
    let _e37 = U.params[0];
    scale = _e37.x;
    let _e42 = U.params[1];
    z = _e42.x;
    let _e47 = U.params[2];
    speed = _e47.x;
    chukcut_init_globals();
    let _e49 = v_uv_1;
    let _e52 = v_uv_1;
    let _e56 = transition(vec2<f32>(_e49.x, (1f - _e52.y)));
    c_3 = _e56;
    let _e58 = c_3;
    a_2 = clamp(_e58.w, 0f, 1f);
    let _e64 = a_2;
    if (_e64 > 0.00001f) {
        let _e67 = c_3;
        let _e69 = a_2;
        let _e71 = (_e67.xyz / vec3(_e69));
        let _e72 = a_2;
        local_1 = vec4<f32>(_e71.x, _e71.y, _e71.z, _e72);
    } else {
        local_1 = vec4(0f);
    }
    let _e80 = local_1;
    o_color = _e80;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e34 = o_color;
    return FragmentOutput(_e34);
}

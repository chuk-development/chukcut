// GlitchDisplace, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Matt DesLauriers
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

fn random(co: vec2<f32>) -> f32 {
    var co_1: vec2<f32>;
    var a: f32 = 12.9898f;
    var b: f32 = 78.233f;
    var c_2: f32 = 43758.547f;
    var dt: f32;
    var sn: f32;

    co_1 = co;
    let _e20 = co_1;
    let _e22 = a;
    let _e23 = b;
    dt = dot(_e20.xy, vec2<f32>(_e22, _e23));
    let _e27 = dt;
    sn = (_e27 - (floor((_e27 / 3.14f)) * 3.14f));
    let _e34 = sn;
    let _e36 = c_2;
    return fract((sin(_e34) * _e36));
}

fn voronoi(x: vec2<f32>) -> f32 {
    var x_1: vec2<f32>;
    var p: vec2<f32>;
    var f: vec2<f32>;
    var res: f32 = 8f;
    var j: f32 = -1f;
    var i: f32;
    var b_1: vec2<f32>;
    var r: vec2<f32>;
    var d: f32;

    x_1 = x;
    let _e14 = x_1;
    p = floor(_e14);
    let _e17 = x_1;
    f = fract(_e17);
    loop {
        let _e25 = j;
        if !((_e25 <= 1f)) {
            break;
        }
        i = -1f;
        loop {
            let _e35 = i;
            if !((_e35 <= 1f)) {
                break;
            }
            {
                let _e42 = i;
                let _e43 = j;
                b_1 = vec2<f32>(_e42, _e43);
                let _e46 = b_1;
                let _e47 = f;
                let _e49 = p;
                let _e50 = b_1;
                let _e52 = random((_e49 + _e50));
                r = ((_e46 - _e47) + vec2(_e52));
                let _e56 = r;
                let _e57 = r;
                d = dot(_e56, _e57);
                let _e60 = res;
                let _e61 = d;
                res = min(_e60, _e61);
            }
            continuing {
                let _e39 = i;
                i = (_e39 + 1f);
            }
        }
        continuing {
            let _e29 = j;
            j = (_e29 + 1f);
        }
    }
    let _e63 = res;
    return sqrt(_e63);
}

fn displace(tex: vec4<f32>, texCoord: vec2<f32>, dotDepth: f32, textureDepth: f32, strength: f32) -> vec2<f32> {
    var tex_1: vec4<f32>;
    var texCoord_1: vec2<f32>;
    var dotDepth_1: f32;
    var textureDepth_1: f32;
    var strength_1: f32;
    var b_2: f32;
    var g: f32;
    var r_1: f32;
    var dt_1: vec4<f32>;
    var dis: vec4<f32>;
    var res_uv: vec2<f32>;

    tex_1 = tex;
    texCoord_1 = texCoord;
    dotDepth_1 = dotDepth;
    textureDepth_1 = textureDepth;
    strength_1 = strength;
    let _e23 = texCoord_1;
    let _e28 = voronoi(((0.003f * _e23) + vec2(2f)));
    b_2 = _e28;
    let _e31 = texCoord_1;
    let _e33 = voronoi((0.2f * _e31));
    g = _e33;
    let _e35 = texCoord_1;
    let _e39 = voronoi((_e35 - vec2(1f)));
    r_1 = _e39;
    let _e41 = tex_1;
    dt_1 = (_e41 * 1f);
    let _e45 = dt_1;
    let _e46 = dotDepth_1;
    let _e51 = tex_1;
    let _e52 = textureDepth_1;
    dis = (((_e45 * _e46) + vec4(1f)) - (_e51 * _e52));
    let _e57 = dis;
    let _e61 = textureDepth_1;
    let _e62 = dotDepth_1;
    dis.x = ((_e57.x - 1f) + (_e61 * _e62));
    let _e66 = dis;
    let _e70 = textureDepth_1;
    let _e71 = dotDepth_1;
    dis.y = ((_e66.y - 1f) + (_e70 * _e71));
    let _e75 = dis;
    let _e77 = strength_1;
    dis.x = (_e75.x * _e77);
    let _e80 = dis;
    let _e82 = strength_1;
    dis.y = (_e80.y * _e82);
    let _e84 = texCoord_1;
    res_uv = _e84;
    let _e87 = res_uv;
    let _e89 = dis;
    res_uv.x = ((_e87.x + _e89.x) - 0f);
    let _e95 = res_uv;
    let _e97 = dis;
    res_uv.y = (_e95.y + _e97.y);
    let _e100 = res_uv;
    return _e100;
}

fn ease1_(t: f32) -> f32 {
    var t_1: f32;
    var local: f32;
    var local_1: f32;

    t_1 = t;
    let _e14 = t_1;
    let _e17 = t_1;
    if ((_e14 == 0f) || (_e17 == 1f)) {
        let _e21 = t_1;
        local_1 = _e21;
    } else {
        let _e22 = t_1;
        if (_e22 < 0.5f) {
            let _e28 = t_1;
            local = (0.5f * pow(2f, ((20f * _e28) - 10f)));
        } else {
            let _e38 = t_1;
            local = ((-0.5f * pow(2f, (10f - (_e38 * 20f)))) + 1f);
        }
        let _e47 = local;
        local_1 = _e47;
    }
    let _e49 = local_1;
    return _e49;
}

fn ease2_(t_2: f32) -> f32 {
    var t_3: f32;
    var local_2: f32;

    t_3 = t_2;
    let _e14 = t_3;
    if (_e14 == 1f) {
        let _e17 = t_3;
        local_2 = _e17;
    } else {
        let _e22 = t_3;
        local_2 = (1f - pow(2f, (-10f * _e22)));
    }
    let _e27 = local_2;
    return _e27;
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var p_1: vec2<f32>;
    var color1_: vec4<f32>;
    var color2_: vec4<f32>;
    var disp: vec2<f32>;
    var disp2_: vec2<f32>;
    var dColor1_: vec4<f32>;
    var dColor2_: vec4<f32>;
    var val: f32;
    var gray: vec3<f32>;

    uv_5 = uv_4;
    let _e14 = uv_5;
    p_1 = (_e14.xy / vec2(1f));
    let _e21 = p_1;
    let _e22 = getFromColor(_e21);
    color1_ = _e22;
    let _e24 = p_1;
    let _e25 = getToColor(_e24);
    color2_ = _e25;
    let _e27 = color1_;
    let _e28 = p_1;
    let _e32 = progress;
    let _e33 = ease1_(_e32);
    let _e35 = displace(_e27, _e28, 0.33f, 0.7f, (1f - _e33));
    disp = _e35;
    let _e37 = color2_;
    let _e38 = p_1;
    let _e41 = progress;
    let _e42 = ease2_(_e41);
    let _e43 = displace(_e37, _e38, 0.33f, 0.5f, _e42);
    disp2_ = _e43;
    let _e45 = disp;
    let _e46 = getToColor(_e45);
    dColor1_ = _e46;
    let _e48 = disp2_;
    let _e49 = getFromColor(_e48);
    dColor2_ = _e49;
    let _e51 = progress;
    let _e52 = ease1_(_e51);
    val = _e52;
    let _e54 = dColor2_;
    let _e55 = dColor1_;
    gray = vec3(dot(min(_e54, _e55).xyz, vec3<f32>(0.299f, 0.587f, 0.114f)));
    let _e65 = gray;
    dColor2_ = vec4<f32>(_e65.x, _e65.y, _e65.z, 1f);
    let _e71 = dColor2_;
    dColor2_ = (_e71 * 2f);
    let _e74 = color1_;
    let _e75 = dColor2_;
    let _e78 = progress;
    color1_ = mix(_e74, _e75, vec4(smoothstep(0f, 0.5f, _e78)));
    let _e82 = color2_;
    let _e83 = dColor1_;
    let _e86 = progress;
    color2_ = mix(_e82, _e83, vec4(smoothstep(1f, 0.5f, _e86)));
    let _e90 = color1_;
    let _e91 = color2_;
    let _e92 = val;
    return mix(_e90, _e91, vec4(_e92));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
    var a_1: f32;
    var local_3: vec4<f32>;

    let _e12 = U;
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
    c_3 = _e35;
    let _e37 = c_3;
    a_1 = clamp(_e37.w, 0f, 1f);
    let _e43 = a_1;
    if (_e43 > 0.00001f) {
        let _e46 = c_3;
        let _e48 = a_1;
        let _e50 = (_e46.xyz / vec3(_e48));
        let _e51 = a_1;
        local_3 = vec4<f32>(_e50.x, _e50.y, _e50.z, _e51);
    } else {
        local_3 = vec4(0f);
    }
    let _e59 = local_3;
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

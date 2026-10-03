// BowTieWithParameter, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: KMojek
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

const height: f32 = 0.5f;

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
var<private> adjust: f32;
var<private> reverse: bool;

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

fn check(p1_: vec2<f32>, p2_: vec2<f32>, p3_: vec2<f32>) -> f32 {
    var p1_1: vec2<f32>;
    var p2_1: vec2<f32>;
    var p3_1: vec2<f32>;

    p1_1 = p1_;
    p2_1 = p2_;
    p3_1 = p3_;
    let _e20 = p1_1;
    let _e22 = p3_1;
    let _e25 = p2_1;
    let _e27 = p3_1;
    let _e31 = p2_1;
    let _e33 = p3_1;
    let _e36 = p1_1;
    let _e38 = p3_1;
    return (((_e20.x - _e22.x) * (_e25.y - _e27.y)) - ((_e31.x - _e33.x) * (_e36.y - _e38.y)));
}

fn pointInTriangle(pt: vec2<f32>, p1_2: vec2<f32>, p2_2: vec2<f32>, p3_2: vec2<f32>) -> bool {
    var pt_1: vec2<f32>;
    var p1_3: vec2<f32>;
    var p2_3: vec2<f32>;
    var p3_3: vec2<f32>;
    var b1_: bool;
    var b2_: bool;
    var b3_: bool;

    pt_1 = pt;
    p1_3 = p1_2;
    p2_3 = p2_2;
    p3_3 = p3_2;
    let _e22 = pt_1;
    let _e23 = p1_3;
    let _e24 = p2_3;
    let _e25 = check(_e22, _e23, _e24);
    b1_ = (_e25 < 0f);
    let _e29 = pt_1;
    let _e30 = p2_3;
    let _e31 = p3_3;
    let _e32 = check(_e29, _e30, _e31);
    b2_ = (_e32 < 0f);
    let _e36 = pt_1;
    let _e37 = p3_3;
    let _e38 = p1_3;
    let _e39 = check(_e36, _e37, _e38);
    b3_ = (_e39 < 0f);
    let _e43 = b1_;
    let _e44 = b2_;
    let _e46 = b2_;
    let _e47 = b3_;
    return ((_e43 == _e44) && (_e46 == _e47));
}

fn transition_firstHalf(uv_4: vec2<f32>, prog: f32) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var prog_1: f32;
    var botLeft: vec2<f32>;
    var botRight: vec2<f32>;
    var tip: vec2<f32>;
    var topLeft: vec2<f32>;
    var topRight: vec2<f32>;
    var tip_1: vec2<f32>;

    uv_5 = uv_4;
    prog_1 = prog;
    let _e19 = uv_5;
    if (_e19.y < 0.5f) {
        {
            let _e25 = prog_1;
            botLeft = vec2<f32>(-0f, (_e25 - height));
            let _e30 = prog_1;
            botRight = vec2<f32>(1f, (_e30 - height));
            let _e34 = adjust;
            let _e35 = prog_1;
            tip = vec2<f32>(_e34, _e35);
            let _e38 = uv_5;
            let _e39 = botLeft;
            let _e40 = botRight;
            let _e41 = tip;
            let _e42 = pointInTriangle(_e38, _e39, _e40, _e41);
            if _e42 {
                let _e43 = uv_5;
                let _e44 = getToColor(_e43);
                return _e44;
            }
        }
    } else {
        {
            let _e48 = prog_1;
            topLeft = vec2<f32>(-0f, ((1f - _e48) + height));
            let _e55 = prog_1;
            topRight = vec2<f32>(1f, ((1f - _e55) + height));
            let _e60 = adjust;
            let _e62 = prog_1;
            tip_1 = vec2<f32>(_e60, (1f - _e62));
            let _e66 = uv_5;
            let _e67 = topLeft;
            let _e68 = topRight;
            let _e69 = tip_1;
            let _e70 = pointInTriangle(_e66, _e67, _e68, _e69);
            if _e70 {
                let _e71 = uv_5;
                let _e72 = getToColor(_e71);
                return _e72;
            }
        }
    }
    let _e73 = uv_5;
    let _e74 = getFromColor(_e73);
    return _e74;
}

fn transition_secondHalf(uv_6: vec2<f32>, prog_2: f32) -> vec4<f32> {
    var uv_7: vec2<f32>;
    var prog_3: f32;
    var top: vec2<f32>;
    var bot: vec2<f32>;
    var tip_2: vec2<f32>;
    var top_1: vec2<f32>;
    var bot_1: vec2<f32>;
    var tip_3: vec2<f32>;

    uv_7 = uv_6;
    prog_3 = prog_2;
    let _e19 = uv_7;
    let _e21 = adjust;
    if (_e19.x > _e21) {
        {
            let _e23 = prog_3;
            top = vec2<f32>((_e23 + height), 1f);
            let _e28 = prog_3;
            bot = vec2<f32>((_e28 + height), -0f);
            let _e34 = adjust;
            let _e37 = prog_3;
            tip_2 = vec2<f32>(mix(_e34, 1f, (2f * (_e37 - 0.5f))), 0.5f);
            let _e45 = uv_7;
            let _e46 = top;
            let _e47 = bot;
            let _e48 = tip_2;
            let _e49 = pointInTriangle(_e45, _e46, _e47, _e48);
            if _e49 {
                let _e50 = uv_7;
                let _e51 = getFromColor(_e50);
                return _e51;
            }
        }
    } else {
        {
            let _e53 = prog_3;
            top_1 = vec2<f32>(((1f - _e53) - height), 1f);
            let _e60 = prog_3;
            bot_1 = vec2<f32>(((1f - _e60) - height), -0f);
            let _e67 = adjust;
            let _e70 = prog_3;
            tip_3 = vec2<f32>(mix(_e67, 0f, (2f * (_e70 - 0.5f))), 0.5f);
            let _e78 = uv_7;
            let _e79 = top_1;
            let _e80 = bot_1;
            let _e81 = tip_3;
            let _e82 = pointInTriangle(_e78, _e79, _e80, _e81);
            if _e82 {
                let _e83 = uv_7;
                let _e84 = getFromColor(_e83);
                return _e84;
            }
        }
    }
    let _e85 = uv_7;
    let _e86 = getToColor(_e85);
    return _e86;
}

fn transition(uv_8: vec2<f32>) -> vec4<f32> {
    var uv_9: vec2<f32>;
    var local: vec4<f32>;
    var local_1: vec4<f32>;

    uv_9 = uv_8;
    let _e17 = reverse;
    if _e17 {
        let _e18 = progress;
        if (_e18 < 0.5f) {
            let _e21 = uv_9;
            let _e23 = progress;
            let _e25 = transition_secondHalf(_e21, (1f - _e23));
            local = _e25;
        } else {
            let _e26 = uv_9;
            let _e28 = progress;
            let _e30 = transition_firstHalf(_e26, (1f - _e28));
            local = _e30;
        }
        let _e32 = local;
        return _e32;
    } else {
        let _e33 = progress;
        if (_e33 < 0.5f) {
            let _e36 = uv_9;
            let _e37 = progress;
            let _e38 = transition_firstHalf(_e36, _e37);
            local_1 = _e38;
        } else {
            let _e39 = uv_9;
            let _e40 = progress;
            let _e41 = transition_secondHalf(_e39, _e40);
            local_1 = _e41;
        }
        let _e43 = local_1;
        return _e43;
    }
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_2: vec4<f32>;

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    let _e34 = U.params[0];
    adjust = _e34.x;
    let _e39 = U.params[1];
    reverse = (_e39.x > 0.5f);
    chukcut_init_globals();
    let _e43 = v_uv_1;
    let _e46 = v_uv_1;
    let _e50 = transition(vec2<f32>(_e43.x, (1f - _e46.y)));
    c_2 = _e50;
    let _e52 = c_2;
    a = clamp(_e52.w, 0f, 1f);
    let _e58 = a;
    if (_e58 > 0.00001f) {
        let _e61 = c_2;
        let _e63 = a;
        let _e65 = (_e61.xyz / vec3(_e63));
        let _e66 = a;
        local_2 = vec4<f32>(_e65.x, _e65.y, _e65.z, _e66);
    } else {
        local_2 = vec4(0f);
    }
    let _e74 = local_2;
    o_color = _e74;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}

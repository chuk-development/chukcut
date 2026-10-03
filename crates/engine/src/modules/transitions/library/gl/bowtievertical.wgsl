// BowTieVertical, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: huynx
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

fn check(p1_: vec2<f32>, p2_: vec2<f32>, p3_: vec2<f32>) -> f32 {
    var p1_1: vec2<f32>;
    var p2_1: vec2<f32>;
    var p3_1: vec2<f32>;

    p1_1 = p1_;
    p2_1 = p2_;
    p3_1 = p3_;
    let _e18 = p1_1;
    let _e20 = p3_1;
    let _e23 = p2_1;
    let _e25 = p3_1;
    let _e29 = p2_1;
    let _e31 = p3_1;
    let _e34 = p1_1;
    let _e36 = p3_1;
    return (((_e18.x - _e20.x) * (_e23.y - _e25.y)) - ((_e29.x - _e31.x) * (_e34.y - _e36.y)));
}

fn PointInTriangle(pt: vec2<f32>, p1_2: vec2<f32>, p2_2: vec2<f32>, p3_2: vec2<f32>) -> bool {
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
    let _e23 = pt_1;
    let _e24 = p1_3;
    let _e25 = p2_3;
    let _e26 = check(_e23, _e24, _e25);
    b1_ = (_e26 < 0f);
    let _e29 = pt_1;
    let _e30 = p2_3;
    let _e31 = p3_3;
    let _e32 = check(_e29, _e30, _e31);
    b2_ = (_e32 < 0f);
    let _e35 = pt_1;
    let _e36 = p3_3;
    let _e37 = p1_3;
    let _e38 = check(_e35, _e36, _e37);
    b3_ = (_e38 < 0f);
    let _e41 = b1_;
    let _e42 = b2_;
    let _e44 = b2_;
    let _e45 = b3_;
    return ((_e41 == _e42) && (_e44 == _e45));
}

fn in_top_triangle(p: vec2<f32>) -> bool {
    var p_1: vec2<f32>;
    var vertex1_: vec2<f32>;
    var vertex2_: vec2<f32>;
    var vertex3_: vec2<f32>;

    p_1 = p;
    let _e18 = progress;
    vertex1_ = vec2<f32>(0.5f, _e18);
    let _e21 = progress;
    vertex2_ = vec2<f32>((0.5f - _e21), 0f);
    let _e26 = progress;
    vertex3_ = vec2<f32>((0.5f + _e26), 0f);
    let _e30 = p_1;
    let _e31 = vertex1_;
    let _e32 = vertex2_;
    let _e33 = vertex3_;
    let _e34 = PointInTriangle(_e30, _e31, _e32, _e33);
    if _e34 {
        {
            return true;
        }
    }
    return false;
}

fn in_bottom_triangle(p_2: vec2<f32>) -> bool {
    var p_3: vec2<f32>;
    var vertex1_1: vec2<f32>;
    var vertex2_1: vec2<f32>;
    var vertex3_1: vec2<f32>;

    p_3 = p_2;
    let _e19 = progress;
    vertex1_1 = vec2<f32>(0.5f, (1f - _e19));
    let _e23 = progress;
    vertex2_1 = vec2<f32>((0.5f - _e23), 1f);
    let _e28 = progress;
    vertex3_1 = vec2<f32>((0.5f + _e28), 1f);
    let _e32 = p_3;
    let _e33 = vertex1_1;
    let _e34 = vertex2_1;
    let _e35 = vertex3_1;
    let _e36 = PointInTriangle(_e32, _e33, _e34, _e35);
    if _e36 {
        {
            return true;
        }
    }
    return false;
}

fn blur_edge(bot1_: vec2<f32>, bot2_: vec2<f32>, top: vec2<f32>, testPt: vec2<f32>) -> f32 {
    var bot1_1: vec2<f32>;
    var bot2_1: vec2<f32>;
    var top_1: vec2<f32>;
    var testPt_1: vec2<f32>;
    var lineDir: vec2<f32>;
    var perpDir: vec2<f32>;
    var dirToPt1_: vec2<f32>;
    var dist1_: f32;
    var min_dist: f32;

    bot1_1 = bot1_;
    bot2_1 = bot2_;
    top_1 = top;
    testPt_1 = testPt;
    let _e20 = bot1_1;
    let _e21 = top_1;
    lineDir = (_e20 - _e21);
    let _e24 = lineDir;
    let _e26 = lineDir;
    perpDir = vec2<f32>(_e24.y, -(_e26.x));
    let _e31 = bot1_1;
    let _e32 = testPt_1;
    dirToPt1_ = (_e31 - _e32);
    let _e35 = perpDir;
    let _e37 = dirToPt1_;
    dist1_ = abs(dot(normalize(_e35), _e37));
    let _e41 = bot2_1;
    let _e42 = top_1;
    lineDir = (_e41 - _e42);
    let _e44 = lineDir;
    let _e46 = lineDir;
    perpDir = vec2<f32>(_e44.y, -(_e46.x));
    let _e50 = bot2_1;
    let _e51 = testPt_1;
    dirToPt1_ = (_e50 - _e51);
    let _e53 = perpDir;
    let _e55 = dirToPt1_;
    let _e58 = dist1_;
    min_dist = min(abs(dot(normalize(_e53), _e55)), _e58);
    let _e61 = min_dist;
    if (_e61 < 0.005f) {
        {
            let _e64 = min_dist;
            return (_e64 / 0.005f);
        }
    } else {
        {
            return 1f;
        }
    }
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var vertex1_2: vec2<f32>;
    var vertex2_2: vec2<f32>;
    var vertex3_2: vec2<f32>;
    var vertex1_3: vec2<f32>;
    var vertex2_3: vec2<f32>;
    var vertex3_3: vec2<f32>;

    uv_5 = uv_4;
    let _e14 = uv_5;
    let _e15 = in_top_triangle(_e14);
    if _e15 {
        {
            let _e16 = progress;
            if (_e16 < 0.1f) {
                {
                    let _e19 = uv_5;
                    let _e20 = getFromColor(_e19);
                    return _e20;
                }
            }
            let _e21 = uv_5;
            if (_e21.y < 0.5f) {
                {
                    let _e26 = progress;
                    vertex1_2 = vec2<f32>(0.5f, _e26);
                    let _e30 = progress;
                    vertex2_2 = vec2<f32>((0.5f - _e30), 0f);
                    let _e36 = progress;
                    vertex3_2 = vec2<f32>((0.5f + _e36), 0f);
                    let _e41 = uv_5;
                    let _e42 = getFromColor(_e41);
                    let _e43 = uv_5;
                    let _e44 = getToColor(_e43);
                    let _e45 = vertex2_2;
                    let _e46 = vertex3_2;
                    let _e47 = vertex1_2;
                    let _e48 = uv_5;
                    let _e49 = blur_edge(_e45, _e46, _e47, _e48);
                    return mix(_e42, _e44, vec4(_e49));
                }
            } else {
                {
                    let _e52 = progress;
                    if (_e52 > 0f) {
                        {
                            let _e55 = uv_5;
                            let _e56 = getToColor(_e55);
                            return _e56;
                        }
                    } else {
                        {
                            let _e57 = uv_5;
                            let _e58 = getFromColor(_e57);
                            return _e58;
                        }
                    }
                }
            }
        }
    } else {
        let _e59 = uv_5;
        let _e60 = in_bottom_triangle(_e59);
        if _e60 {
            {
                let _e61 = uv_5;
                if (_e61.y >= 0.5f) {
                    {
                        let _e67 = progress;
                        vertex1_3 = vec2<f32>(0.5f, (1f - _e67));
                        let _e72 = progress;
                        vertex2_3 = vec2<f32>((0.5f - _e72), 1f);
                        let _e78 = progress;
                        vertex3_3 = vec2<f32>((0.5f + _e78), 1f);
                        let _e83 = uv_5;
                        let _e84 = getFromColor(_e83);
                        let _e85 = uv_5;
                        let _e86 = getToColor(_e85);
                        let _e87 = vertex2_3;
                        let _e88 = vertex3_3;
                        let _e89 = vertex1_3;
                        let _e90 = uv_5;
                        let _e91 = blur_edge(_e87, _e88, _e89, _e90);
                        return mix(_e84, _e86, vec4(_e91));
                    }
                } else {
                    {
                        let _e94 = uv_5;
                        let _e95 = getFromColor(_e94);
                        return _e95;
                    }
                }
            }
        } else {
            {
                let _e96 = uv_5;
                let _e97 = getFromColor(_e96);
                return _e97;
            }
        }
    }
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

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

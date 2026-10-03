// BowTieHorizontal, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
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

const bottom_left: vec2<f32> = vec2<f32>(0f, 1f);
const bottom_right: vec2<f32> = vec2<f32>(1f, 1f);
const top_left: vec2<f32> = vec2<f32>(0f, 0f);
const top_right: vec2<f32> = vec2<f32>(1f, 0f);
const center: vec2<f32> = vec2<f32>(0.5f, 0.5f);

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
    let _e23 = p1_1;
    let _e25 = p3_1;
    let _e28 = p2_1;
    let _e30 = p3_1;
    let _e34 = p2_1;
    let _e36 = p3_1;
    let _e39 = p1_1;
    let _e41 = p3_1;
    return (((_e23.x - _e25.x) * (_e28.y - _e30.y)) - ((_e34.x - _e36.x) * (_e39.y - _e41.y)));
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
    let _e28 = pt_1;
    let _e29 = p1_3;
    let _e30 = p2_3;
    let _e31 = check(_e28, _e29, _e30);
    b1_ = (_e31 < 0f);
    let _e34 = pt_1;
    let _e35 = p2_3;
    let _e36 = p3_3;
    let _e37 = check(_e34, _e35, _e36);
    b2_ = (_e37 < 0f);
    let _e40 = pt_1;
    let _e41 = p3_3;
    let _e42 = p1_3;
    let _e43 = check(_e40, _e41, _e42);
    b3_ = (_e43 < 0f);
    let _e46 = b1_;
    let _e47 = b2_;
    let _e49 = b2_;
    let _e50 = b3_;
    return ((_e46 == _e47) && (_e49 == _e50));
}

fn in_left_triangle(p: vec2<f32>) -> bool {
    var p_1: vec2<f32>;
    var vertex1_: vec2<f32>;
    var vertex2_: vec2<f32>;
    var vertex3_: vec2<f32>;

    p_1 = p;
    let _e22 = progress;
    vertex1_ = vec2<f32>(_e22, 0.5f);
    let _e27 = progress;
    vertex2_ = vec2<f32>(0f, (0.5f - _e27));
    let _e32 = progress;
    vertex3_ = vec2<f32>(0f, (0.5f + _e32));
    let _e35 = p_1;
    let _e36 = vertex1_;
    let _e37 = vertex2_;
    let _e38 = vertex3_;
    let _e39 = PointInTriangle(_e35, _e36, _e37, _e38);
    if _e39 {
        {
            return true;
        }
    }
    return false;
}

fn in_right_triangle(p_2: vec2<f32>) -> bool {
    var p_3: vec2<f32>;
    var vertex1_1: vec2<f32>;
    var vertex2_1: vec2<f32>;
    var vertex3_1: vec2<f32>;

    p_3 = p_2;
    let _e23 = progress;
    vertex1_1 = vec2<f32>((1f - _e23), 0.5f);
    let _e29 = progress;
    vertex2_1 = vec2<f32>(1f, (0.5f - _e29));
    let _e34 = progress;
    vertex3_1 = vec2<f32>(1f, (0.5f + _e34));
    let _e37 = p_3;
    let _e38 = vertex1_1;
    let _e39 = vertex2_1;
    let _e40 = vertex3_1;
    let _e41 = PointInTriangle(_e37, _e38, _e39, _e40);
    if _e41 {
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
    let _e25 = bot1_1;
    let _e26 = top_1;
    lineDir = (_e25 - _e26);
    let _e29 = lineDir;
    let _e31 = lineDir;
    perpDir = vec2<f32>(_e29.y, -(_e31.x));
    let _e36 = bot1_1;
    let _e37 = testPt_1;
    dirToPt1_ = (_e36 - _e37);
    let _e40 = perpDir;
    let _e42 = dirToPt1_;
    dist1_ = abs(dot(normalize(_e40), _e42));
    let _e46 = bot2_1;
    let _e47 = top_1;
    lineDir = (_e46 - _e47);
    let _e49 = lineDir;
    let _e51 = lineDir;
    perpDir = vec2<f32>(_e49.y, -(_e51.x));
    let _e55 = bot2_1;
    let _e56 = testPt_1;
    dirToPt1_ = (_e55 - _e56);
    let _e58 = perpDir;
    let _e60 = dirToPt1_;
    let _e63 = dist1_;
    min_dist = min(abs(dot(normalize(_e58), _e60)), _e63);
    let _e66 = min_dist;
    if (_e66 < 0.005f) {
        {
            let _e69 = min_dist;
            return (_e69 / 0.005f);
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
    let _e19 = uv_5;
    let _e20 = in_left_triangle(_e19);
    if _e20 {
        {
            let _e21 = progress;
            if (_e21 < 0.1f) {
                {
                    let _e24 = uv_5;
                    let _e25 = getFromColor(_e24);
                    return _e25;
                }
            }
            let _e26 = uv_5;
            if (_e26.x < 0.5f) {
                {
                    let _e30 = progress;
                    vertex1_2 = vec2<f32>(_e30, 0.5f);
                    let _e36 = progress;
                    vertex2_2 = vec2<f32>(0f, (0.5f - _e36));
                    let _e42 = progress;
                    vertex3_2 = vec2<f32>(0f, (0.5f + _e42));
                    let _e46 = uv_5;
                    let _e47 = getFromColor(_e46);
                    let _e48 = uv_5;
                    let _e49 = getToColor(_e48);
                    let _e50 = vertex2_2;
                    let _e51 = vertex3_2;
                    let _e52 = vertex1_2;
                    let _e53 = uv_5;
                    let _e54 = blur_edge(_e50, _e51, _e52, _e53);
                    return mix(_e47, _e49, vec4(_e54));
                }
            } else {
                {
                    let _e57 = progress;
                    if (_e57 > 0f) {
                        {
                            let _e60 = uv_5;
                            let _e61 = getToColor(_e60);
                            return _e61;
                        }
                    } else {
                        {
                            let _e62 = uv_5;
                            let _e63 = getFromColor(_e62);
                            return _e63;
                        }
                    }
                }
            }
        }
    } else {
        let _e64 = uv_5;
        let _e65 = in_right_triangle(_e64);
        if _e65 {
            {
                let _e66 = uv_5;
                if (_e66.x >= 0.5f) {
                    {
                        let _e71 = progress;
                        vertex1_3 = vec2<f32>((1f - _e71), 0.5f);
                        let _e78 = progress;
                        vertex2_3 = vec2<f32>(1f, (0.5f - _e78));
                        let _e84 = progress;
                        vertex3_3 = vec2<f32>(1f, (0.5f + _e84));
                        let _e88 = uv_5;
                        let _e89 = getFromColor(_e88);
                        let _e90 = uv_5;
                        let _e91 = getToColor(_e90);
                        let _e92 = vertex2_3;
                        let _e93 = vertex3_3;
                        let _e94 = vertex1_3;
                        let _e95 = uv_5;
                        let _e96 = blur_edge(_e92, _e93, _e94, _e95);
                        return mix(_e89, _e91, vec4(_e96));
                    }
                } else {
                    {
                        let _e99 = uv_5;
                        let _e100 = getFromColor(_e99);
                        return _e100;
                    }
                }
            }
        } else {
            {
                let _e101 = uv_5;
                let _e102 = getFromColor(_e101);
                return _e102;
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

    let _e17 = U;
    progress = _e17.state.x;
    let _e21 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e21);
    let _e24 = dims;
    let _e27 = dims;
    ratio = (f32(_e24.x) / f32(max(_e27.y, 1i)));
    chukcut_init_globals();
    let _e33 = v_uv_1;
    let _e36 = v_uv_1;
    let _e40 = transition(vec2<f32>(_e33.x, (1f - _e36.y)));
    c_2 = _e40;
    let _e42 = c_2;
    a = clamp(_e42.w, 0f, 1f);
    let _e48 = a;
    if (_e48 > 0.00001f) {
        let _e51 = c_2;
        let _e53 = a;
        let _e55 = (_e51.xyz / vec3(_e53));
        let _e56 = a;
        local = vec4<f32>(_e55.x, _e55.y, _e55.z, _e56);
    } else {
        local = vec4(0f);
    }
    let _e64 = local;
    o_color = _e64;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e32 = o_color;
    return FragmentOutput(_e32);
}

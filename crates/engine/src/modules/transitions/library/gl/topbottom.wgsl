// TopBottom, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: zhmy
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

const black: vec4<f32> = vec4<f32>(0f, 0f, 0f, 1f);
const boundMin: vec2<f32> = vec2<f32>(0f, 0f);
const boundMax: vec2<f32> = vec2<f32>(1f, 1f);

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

fn inBounds(p: vec2<f32>) -> bool {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e17 = p_1;
    let _e20 = p_1;
    return (all((boundMin < _e17)) && all((_e20 < boundMax)));
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var spfr: vec2<f32>;
    var spto: vec2<f32> = vec2(-1f);
    var size: f32;

    uv_5 = uv_4;
    let _e24 = progress;
    size = mix(1f, 3f, (_e24 * 0.2f));
    let _e29 = uv_5;
    let _e36 = size;
    let _e37 = size;
    spto = (((_e29 + vec2<f32>(-0.5f, -0.5f)) * vec2<f32>(_e36, _e37)) + vec2<f32>(0.5f, 0.5f));
    let _e44 = uv_5;
    let _e47 = progress;
    spfr = (_e44 + vec2<f32>(0f, (1f - _e47)));
    let _e51 = spfr;
    let _e52 = inBounds(_e51);
    if _e52 {
        {
            let _e53 = spfr;
            let _e54 = getToColor(_e53);
            return _e54;
        }
    } else {
        let _e55 = spto;
        let _e56 = inBounds(_e55);
        if _e56 {
            {
                let _e57 = spto;
                let _e58 = getFromColor(_e57);
                let _e60 = progress;
                return (_e58 * (1f - _e60));
            }
        } else {
            {
                return black;
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

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    chukcut_init_globals();
    let _e31 = v_uv_1;
    let _e34 = v_uv_1;
    let _e38 = transition(vec2<f32>(_e31.x, (1f - _e34.y)));
    c_2 = _e38;
    let _e40 = c_2;
    a = clamp(_e40.w, 0f, 1f);
    let _e46 = a;
    if (_e46 > 0.00001f) {
        let _e49 = c_2;
        let _e51 = a;
        let _e53 = (_e49.xyz / vec3(_e51));
        let _e54 = a;
        local = vec4<f32>(_e53.x, _e53.y, _e53.z, _e54);
    } else {
        local = vec4(0f);
    }
    let _e62 = local;
    o_color = _e62;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}

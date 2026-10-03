// PuzzleRight, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: JustKirillS
// License: MIT
// Ported from https://gist.github.com/JustKirillS/714f095318834f4d2375de872c53af1e
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
var<private> size: vec2<i32>;
var<private> pause: f32;
var<private> dividerWidth: f32;

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

fn rand(co: vec2<f32>) -> f32 {
    var co_1: vec2<f32>;

    co_1 = co;
    let _e17 = co_1;
    return fract((sin(dot(_e17, vec2<f32>(12.9898f, 78.233f))) * 43758.547f));
}

fn getDelta(p: vec2<f32>) -> f32 {
    var p_1: vec2<f32>;
    var rectangleSize: vec2<f32>;
    var rectanglePos: vec2<f32>;
    var top: f32;
    var bottom: f32;
    var left: f32;
    var right: f32;
    var minX: f32;
    var minY: f32;

    p_1 = p;
    let _e18 = size;
    rectangleSize = (vec2(1f) / vec2<f32>(_e18));
    let _e23 = size;
    let _e25 = p_1;
    rectanglePos = floor((vec2<f32>(_e23) * _e25));
    let _e29 = rectangleSize;
    let _e31 = rectanglePos;
    top = (_e29.y * (_e31.y + 1f));
    let _e37 = rectangleSize;
    let _e39 = rectanglePos;
    bottom = (_e37.y * _e39.y);
    let _e43 = rectangleSize;
    let _e45 = rectanglePos;
    left = (_e43.x * _e45.x);
    let _e49 = rectangleSize;
    let _e51 = rectanglePos;
    right = (_e49.x * (_e51.x + 1f));
    let _e57 = p_1;
    let _e59 = left;
    let _e62 = p_1;
    let _e64 = right;
    minX = min(abs((_e57.x - _e59)), abs((_e62.x - _e64)));
    let _e69 = p_1;
    let _e71 = top;
    let _e74 = p_1;
    let _e76 = bottom;
    minY = min(abs((_e69.y - _e71)), abs((_e74.y - _e76)));
    let _e81 = minX;
    let _e82 = minY;
    return min(_e81, _e82);
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var currentProg: f32;
    var a: f32 = 1f;
    var currentProg_1: f32;
    var rectanglePos_1: vec2<f32>;
    var r: f32;
    var cp: f32;
    var rectangleSize_1: f32;
    var delta: f32;
    var offset: f32;
    var p_2: vec2<f32>;
    var a_1: vec4<f32>;
    var b: vec4<f32>;
    var s: f32;
    var currentProg_2: f32;
    var a_2: f32 = 1f;

    uv_5 = uv_4;
    let _e17 = progress;
    let _e18 = pause;
    if (_e17 < _e18) {
        {
            let _e20 = progress;
            let _e21 = pause;
            currentProg = (_e20 / _e21);
            let _e26 = uv_5;
            let _e27 = getDelta(_e26);
            let _e28 = dividerWidth;
            if (_e27 < _e28) {
                {
                    let _e31 = currentProg;
                    a = (1f - _e31);
                }
            }
            let _e38 = uv_5;
            let _e39 = getFromColor(_e38);
            let _e40 = a;
            return mix(vec4<f32>(0f, 0f, 0f, 1f), _e39, vec4(_e40));
        }
    } else {
        let _e43 = progress;
        let _e45 = pause;
        if (_e43 < (1f - _e45)) {
            {
                let _e48 = uv_5;
                let _e49 = getDelta(_e48);
                let _e50 = dividerWidth;
                if (_e49 < _e50) {
                    {
                        return vec4<f32>(0f, 0f, 0f, 1f);
                    }
                }
                let _e57 = progress;
                let _e58 = pause;
                let _e61 = pause;
                currentProg_1 = ((_e57 - _e58) / (1f - (_e61 * 2f)));
                let _e67 = size;
                let _e69 = uv_5;
                rectanglePos_1 = floor((vec2<f32>(_e67) * _e69));
                let _e73 = rectanglePos_1;
                let _e74 = rand(_e73);
                r = (_e74 - 0.1f);
                let _e80 = r;
                let _e82 = currentProg_1;
                cp = smoothstep(0f, (1f - _e80), _e82);
                let _e86 = size;
                rectangleSize_1 = (1f / f32(_e86.x));
                let _e91 = rectanglePos_1;
                let _e93 = rectangleSize_1;
                delta = (_e91.x * _e93);
                let _e96 = rectangleSize_1;
                let _e99 = delta;
                offset = ((_e96 / 2f) + _e99);
                let _e102 = uv_5;
                p_2 = _e102;
                let _e105 = p_2;
                let _e107 = offset;
                let _e109 = cp;
                let _e116 = offset;
                p_2.x = ((((_e105.x - _e107) / abs((_e109 - 0.5f))) * 0.5f) + _e116);
                let _e118 = p_2;
                let _e119 = getFromColor(_e118);
                a_1 = _e119;
                let _e121 = p_2;
                let _e122 = getToColor(_e121);
                b = _e122;
                let _e124 = size;
                let _e127 = uv_5;
                let _e129 = delta;
                let _e135 = cp;
                s = step(abs(((f32(_e124.x) * (_e127.x - _e129)) - 0.5f)), abs((_e135 - 0.5f)));
                let _e141 = b;
                let _e142 = a_1;
                let _e143 = cp;
                let _e149 = s;
                let _e150 = (mix(_e141, _e142, vec4(step(_e143, 0.5f))).xyz * _e149);
                return vec4<f32>(_e150.x, _e150.y, _e150.z, 1f);
            }
        } else {
            {
                let _e156 = progress;
                let _e159 = pause;
                let _e161 = pause;
                currentProg_2 = (((_e156 - 1f) + _e159) / _e161);
                let _e166 = uv_5;
                let _e167 = getDelta(_e166);
                let _e168 = dividerWidth;
                if (_e167 < _e168) {
                    {
                        let _e170 = currentProg_2;
                        a_2 = _e170;
                    }
                }
                let _e176 = uv_5;
                let _e177 = getToColor(_e176);
                let _e178 = a_2;
                return mix(vec4<f32>(0f, 0f, 0f, 1f), _e177, vec4(_e178));
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
    var a_3: f32;
    var local: vec4<f32>;

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    let _e34 = U.params[0];
    size = vec2<i32>(_e34.xy);
    let _e40 = U.params[1];
    pause = _e40.x;
    let _e45 = U.params[2];
    dividerWidth = _e45.x;
    chukcut_init_globals();
    let _e47 = v_uv_1;
    let _e50 = v_uv_1;
    let _e54 = transition(vec2<f32>(_e47.x, (1f - _e50.y)));
    c_2 = _e54;
    let _e56 = c_2;
    a_3 = clamp(_e56.w, 0f, 1f);
    let _e62 = a_3;
    if (_e62 > 0.00001f) {
        let _e65 = c_2;
        let _e67 = a_3;
        let _e69 = (_e65.xyz / vec3(_e67));
        let _e70 = a_3;
        local = vec4<f32>(_e69.x, _e69.y, _e69.z, _e70);
    } else {
        local = vec4(0f);
    }
    let _e78 = local;
    o_color = _e78;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}

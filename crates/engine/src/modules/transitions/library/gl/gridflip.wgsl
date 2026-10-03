// GridFlip, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: TimDonselaar
// License: MIT
// ported by gre from https://gist.github.com/TimDonselaar/9bcd1c4b5934ba60087bdb55c2ea92e5
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
var<private> bgcolor: vec4<f32>;
var<private> randomness: f32;

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
    let _e19 = co_1;
    return fract((sin(dot(_e19.xy, vec2<f32>(12.9898f, 78.233f))) * 43758.547f));
}

fn getDelta(p: vec2<f32>) -> f32 {
    var p_1: vec2<f32>;
    var rectanglePos: vec2<f32>;
    var rectangleSize: vec2<f32>;
    var top: f32;
    var bottom: f32;
    var left: f32;
    var right: f32;
    var minX: f32;
    var minY: f32;

    p_1 = p;
    let _e19 = size;
    let _e21 = p_1;
    rectanglePos = floor((vec2<f32>(_e19) * _e21));
    let _e26 = size;
    let _e31 = size;
    rectangleSize = vec2<f32>((1f / vec2<f32>(_e26).x), (1f / vec2<f32>(_e31).y));
    let _e37 = rectangleSize;
    let _e39 = rectanglePos;
    top = (_e37.y * (_e39.y + 1f));
    let _e45 = rectangleSize;
    let _e47 = rectanglePos;
    bottom = (_e45.y * _e47.y);
    let _e51 = rectangleSize;
    let _e53 = rectanglePos;
    left = (_e51.x * _e53.x);
    let _e57 = rectangleSize;
    let _e59 = rectanglePos;
    right = (_e57.x * (_e59.x + 1f));
    let _e65 = p_1;
    let _e67 = left;
    let _e70 = p_1;
    let _e72 = right;
    minX = min(abs((_e65.x - _e67)), abs((_e70.x - _e72)));
    let _e77 = p_1;
    let _e79 = top;
    let _e82 = p_1;
    let _e84 = bottom;
    minY = min(abs((_e77.y - _e79)), abs((_e82.y - _e84)));
    let _e89 = minX;
    let _e90 = minY;
    return min(_e89, _e90);
}

fn getDividerSize() -> f32 {
    var rectangleSize_1: vec2<f32>;

    let _e18 = size;
    let _e23 = size;
    rectangleSize_1 = vec2<f32>((1f / vec2<f32>(_e18).x), (1f / vec2<f32>(_e23).y));
    let _e29 = rectangleSize_1;
    let _e31 = rectangleSize_1;
    let _e34 = dividerWidth;
    return (min(_e29.x, _e31.y) * _e34);
}

fn transition(p_2: vec2<f32>) -> vec4<f32> {
    var p_3: vec2<f32>;
    var currentProg: f32;
    var a: f32 = 1f;
    var currentProg_1: f32;
    var q: vec2<f32>;
    var rectanglePos_1: vec2<f32>;
    var r: f32;
    var cp: f32;
    var rectangleSize_2: f32;
    var delta: f32;
    var offset: f32;
    var a_1: vec4<f32>;
    var b: vec4<f32>;
    var s: f32;
    var currentProg_2: f32;
    var a_2: f32 = 1f;

    p_3 = p_2;
    let _e19 = progress;
    let _e20 = pause;
    if (_e19 < _e20) {
        {
            let _e22 = progress;
            let _e23 = pause;
            currentProg = (_e22 / _e23);
            let _e28 = p_3;
            let _e29 = getDelta(_e28);
            let _e30 = getDividerSize();
            if (_e29 < _e30) {
                {
                    let _e33 = currentProg;
                    a = (1f - _e33);
                }
            }
            let _e35 = bgcolor;
            let _e36 = p_3;
            let _e37 = getFromColor(_e36);
            let _e38 = a;
            return mix(_e35, _e37, vec4(_e38));
        }
    } else {
        let _e41 = progress;
        let _e43 = pause;
        if (_e41 < (1f - _e43)) {
            {
                let _e46 = p_3;
                let _e47 = getDelta(_e46);
                let _e48 = getDividerSize();
                if (_e47 < _e48) {
                    {
                        let _e50 = bgcolor;
                        return _e50;
                    }
                } else {
                    {
                        let _e51 = progress;
                        let _e52 = pause;
                        let _e55 = pause;
                        currentProg_1 = ((_e51 - _e52) / (1f - (_e55 * 2f)));
                        let _e61 = p_3;
                        q = _e61;
                        let _e63 = size;
                        let _e65 = q;
                        rectanglePos_1 = floor((vec2<f32>(_e63) * _e65));
                        let _e69 = rectanglePos_1;
                        let _e70 = rand(_e69);
                        let _e71 = randomness;
                        r = (_e70 - _e71);
                        let _e76 = r;
                        let _e78 = currentProg_1;
                        cp = smoothstep(0f, (1f - _e76), _e78);
                        let _e82 = size;
                        rectangleSize_2 = (1f / vec2<f32>(_e82).x);
                        let _e87 = rectanglePos_1;
                        let _e89 = rectangleSize_2;
                        delta = (_e87.x * _e89);
                        let _e92 = rectangleSize_2;
                        let _e95 = delta;
                        offset = ((_e92 / 2f) + _e95);
                        let _e99 = p_3;
                        let _e101 = offset;
                        let _e103 = cp;
                        let _e110 = offset;
                        p_3.x = ((((_e99.x - _e101) / abs((_e103 - 0.5f))) * 0.5f) + _e110);
                        let _e112 = p_3;
                        let _e113 = getFromColor(_e112);
                        a_1 = _e113;
                        let _e115 = p_3;
                        let _e116 = getToColor(_e115);
                        b = _e116;
                        let _e118 = size;
                        let _e121 = q;
                        let _e123 = delta;
                        let _e129 = cp;
                        s = step(abs(((vec2<f32>(_e118).x * (_e121.x - _e123)) - 0.5f)), abs((_e129 - 0.5f)));
                        let _e135 = bgcolor;
                        let _e136 = b;
                        let _e137 = a_1;
                        let _e138 = cp;
                        let _e143 = s;
                        return mix(_e135, mix(_e136, _e137, vec4(step(_e138, 0.5f))), vec4(_e143));
                    }
                }
            }
        } else {
            {
                let _e146 = progress;
                let _e149 = pause;
                let _e151 = pause;
                currentProg_2 = (((_e146 - 1f) + _e149) / _e151);
                let _e156 = p_3;
                let _e157 = getDelta(_e156);
                let _e158 = getDividerSize();
                if (_e157 < _e158) {
                    {
                        let _e160 = currentProg_2;
                        a_2 = _e160;
                    }
                }
                let _e161 = bgcolor;
                let _e162 = p_3;
                let _e163 = getToColor(_e162);
                let _e164 = a_2;
                return mix(_e161, _e163, vec4(_e164));
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

    let _e17 = U;
    progress = _e17.state.x;
    let _e21 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e21);
    let _e24 = dims;
    let _e27 = dims;
    ratio = (f32(_e24.x) / f32(max(_e27.y, 1i)));
    let _e36 = U.params[0];
    size = vec2<i32>(_e36.xy);
    let _e42 = U.params[1];
    pause = _e42.x;
    let _e47 = U.params[2];
    dividerWidth = _e47.x;
    let _e52 = U.params[3];
    bgcolor = _e52;
    let _e56 = U.params[4];
    randomness = _e56.x;
    chukcut_init_globals();
    let _e58 = v_uv_1;
    let _e61 = v_uv_1;
    let _e65 = transition(vec2<f32>(_e58.x, (1f - _e61.y)));
    c_2 = _e65;
    let _e67 = c_2;
    a_3 = clamp(_e67.w, 0f, 1f);
    let _e73 = a_3;
    if (_e73 > 0.00001f) {
        let _e76 = c_2;
        let _e78 = a_3;
        let _e80 = (_e76.xyz / vec3(_e78));
        let _e81 = a_3;
        local = vec4<f32>(_e80.x, _e80.y, _e80.z, _e81);
    } else {
        local = vec4(0f);
    }
    let _e89 = local;
    o_color = _e89;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e32 = o_color;
    return FragmentOutput(_e32);
}

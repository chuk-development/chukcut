// TilesWave, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: numb3r23
// License: MIT
// Ported from https://gist.github.com/numb3r23/169781bb76f310e2bfde
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
var<private> tileCount: vec2<i32>;
var<private> flipX: bool;
var<private> flipY: bool;

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

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var tileSize: vec2<f32>;
    var posInTile: vec2<f32>;
    var tileNum: vec2<f32>;
    var countTiles: f32;
    var offset: f32;
    var timeOffset: f32;
    var sinTime: f32;
    var texC: vec2<f32>;
    var local: f32;
    var local_1: f32;
    var globalUV: vec2<f32>;
    var local_2: f32;
    var local_3: f32;
    var globalUV_1: vec2<f32>;

    uv_5 = uv_4;
    let _e18 = tileCount;
    tileSize = (vec2(1f) / vec2<f32>(_e18));
    let _e23 = uv_5;
    let _e24 = tileCount;
    posInTile = fract((_e23 * vec2<f32>(_e24)));
    let _e29 = uv_5;
    let _e30 = tileCount;
    tileNum = floor((_e29 * vec2<f32>(_e30)));
    let _e35 = tileCount;
    let _e37 = tileCount;
    countTiles = f32((_e35.x * _e37.y));
    let _e42 = tileNum;
    let _e44 = tileNum;
    let _e46 = tileCount;
    let _e51 = countTiles;
    offset = ((_e42.y + (_e44.x * f32(_e46.y))) / _e51);
    let _e54 = progress;
    let _e55 = offset;
    let _e57 = countTiles;
    timeOffset = clamp(((_e54 - _e55) * _e57), 0f, 0.5f);
    let _e64 = timeOffset;
    sinTime = (1f - abs(cos((fract(_e64) * 3.1415925f))));
    let _e72 = posInTile;
    texC = _e72;
    let _e74 = sinTime;
    if (_e74 <= 0.5f) {
        {
            let _e77 = flipX;
            if _e77 {
                {
                    let _e78 = texC;
                    let _e80 = sinTime;
                    let _e82 = texC;
                    let _e85 = sinTime;
                    if ((_e78.x < _e80) || (_e82.x > (1f - _e85))) {
                        let _e89 = uv_5;
                        let _e90 = getFromColor(_e89);
                        return _e90;
                    }
                    let _e92 = texC;
                    if (_e92.x < 0.5f) {
                        let _e96 = texC;
                        let _e98 = sinTime;
                        let _e103 = sinTime;
                        local = (((_e96.x - _e98) * 0.5f) / (0.5f - _e103));
                    } else {
                        let _e106 = texC;
                        let _e113 = sinTime;
                        local = ((((_e106.x - 0.5f) * 0.5f) / (0.5f - _e113)) + 0.5f);
                    }
                    let _e119 = local;
                    texC.x = _e119;
                }
            }
            let _e120 = flipY;
            if _e120 {
                {
                    let _e121 = texC;
                    let _e123 = sinTime;
                    let _e125 = texC;
                    let _e128 = sinTime;
                    if ((_e121.y < _e123) || (_e125.y > (1f - _e128))) {
                        let _e132 = uv_5;
                        let _e133 = getFromColor(_e132);
                        return _e133;
                    }
                    let _e135 = texC;
                    if (_e135.y < 0.5f) {
                        let _e139 = texC;
                        let _e141 = sinTime;
                        let _e146 = sinTime;
                        local_1 = (((_e139.y - _e141) * 0.5f) / (0.5f - _e146));
                    } else {
                        let _e149 = texC;
                        let _e156 = sinTime;
                        local_1 = ((((_e149.y - 0.5f) * 0.5f) / (0.5f - _e156)) + 0.5f);
                    }
                    let _e162 = local_1;
                    texC.y = _e162;
                }
            }
            let _e163 = tileNum;
            let _e164 = tileSize;
            let _e166 = texC;
            let _e167 = tileSize;
            globalUV = ((_e163 * _e164) + (_e166 * _e167));
            let _e171 = globalUV;
            let _e172 = getFromColor(_e171);
            return _e172;
        }
    } else {
        {
            let _e173 = flipX;
            if _e173 {
                {
                    let _e174 = texC;
                    let _e176 = sinTime;
                    let _e178 = texC;
                    let _e181 = sinTime;
                    if ((_e174.x > _e176) || (_e178.x < (1f - _e181))) {
                        let _e185 = uv_5;
                        let _e186 = getToColor(_e185);
                        return _e186;
                    }
                    let _e188 = texC;
                    if (_e188.x < 0.5f) {
                        let _e192 = texC;
                        let _e194 = sinTime;
                        let _e199 = sinTime;
                        local_2 = (((_e192.x - _e194) * 0.5f) / (0.5f - _e199));
                    } else {
                        let _e202 = texC;
                        let _e209 = sinTime;
                        local_2 = ((((_e202.x - 0.5f) * 0.5f) / (0.5f - _e209)) + 0.5f);
                    }
                    let _e215 = local_2;
                    texC.x = _e215;
                    let _e218 = texC;
                    texC.x = (1f - _e218.x);
                }
            }
            let _e221 = flipY;
            if _e221 {
                {
                    let _e222 = texC;
                    let _e224 = sinTime;
                    let _e226 = texC;
                    let _e229 = sinTime;
                    if ((_e222.y > _e224) || (_e226.y < (1f - _e229))) {
                        let _e233 = uv_5;
                        let _e234 = getToColor(_e233);
                        return _e234;
                    }
                    let _e236 = texC;
                    if (_e236.y < 0.5f) {
                        let _e240 = texC;
                        let _e242 = sinTime;
                        let _e247 = sinTime;
                        local_3 = (((_e240.y - _e242) * 0.5f) / (0.5f - _e247));
                    } else {
                        let _e250 = texC;
                        let _e257 = sinTime;
                        local_3 = ((((_e250.y - 0.5f) * 0.5f) / (0.5f - _e257)) + 0.5f);
                    }
                    let _e263 = local_3;
                    texC.y = _e263;
                    let _e266 = texC;
                    texC.y = (1f - _e266.y);
                }
            }
            let _e269 = tileNum;
            let _e270 = tileSize;
            let _e272 = texC;
            let _e273 = tileSize;
            globalUV_1 = ((_e269 * _e270) + (_e272 * _e273));
            let _e277 = globalUV_1;
            let _e278 = getToColor(_e277);
            return _e278;
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
    var local_4: vec4<f32>;

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    let _e34 = U.params[0];
    tileCount = vec2<i32>(_e34.xy);
    let _e40 = U.params[1];
    flipX = (_e40.x > 0.5f);
    let _e47 = U.params[2];
    flipY = (_e47.x > 0.5f);
    chukcut_init_globals();
    let _e51 = v_uv_1;
    let _e54 = v_uv_1;
    let _e58 = transition(vec2<f32>(_e51.x, (1f - _e54.y)));
    c_2 = _e58;
    let _e60 = c_2;
    a = clamp(_e60.w, 0f, 1f);
    let _e66 = a;
    if (_e66 > 0.00001f) {
        let _e69 = c_2;
        let _e71 = a;
        let _e73 = (_e69.xyz / vec3(_e71));
        let _e74 = a;
        local_4 = vec4<f32>(_e73.x, _e73.y, _e73.z, _e74);
    } else {
        local_4 = vec4(0f);
    }
    let _e82 = local_4;
    o_color = _e82;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}

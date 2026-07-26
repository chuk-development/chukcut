// Pass 1. Samples both the source frame and pass 0's target, which is what
// makes this a real two-pass graph rather than two independent effects.
precision highp float;
varying vec2 uv0;

uniform sampler2D inputImageTexture;
uniform sampler2D desaturatedTexture;

uniform float u_intensity;
uniform float u_time;
uniform vec3 u_tint;

// Declared to exercise the reserved-word rewrite: `sample` is a perfectly
// ordinary identifier here and a qualifier in GLSL 450.
vec4 tinted(sampler2D tex, vec2 uv, vec3 colour)
{
    vec4 sample = texture2D(tex, uv);
    return vec4(sample.rgb * colour, sample.a);
}

void main()
{
    vec4 base = texture2D(inputImageTexture, uv0);
    vec4 grey = tinted(desaturatedTexture, uv0, u_tint);
    float wave = 0.5 + 0.5 * sin(u_time);
    gl_FragColor = mix(base, grey, u_intensity * wave);
}

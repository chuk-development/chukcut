// Pass 0. Writes rt/desaturateRT.rt.
precision highp float;
varying vec2 uv0;

uniform sampler2D inputImageTexture;

float luma(vec4 c)
{
    return dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
}

void main()
{
    vec4 base = texture2D(inputImageTexture, uv0);
    gl_FragColor = vec4(vec3(luma(base)), base.a);
}

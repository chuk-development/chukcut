attribute vec3 attPosition;
attribute vec2 attUV;

varying vec2 uv0;

void main()
{
    uv0 = attUV;
    gl_Position = vec4(attPosition, 1.0);
}

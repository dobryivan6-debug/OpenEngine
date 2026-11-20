#version 120
attribute vec2 position;
attribute vec2 texcoord;
attribute vec4 color;
uniform vec2 canvas;
uniform vec2 fit;
varying vec2 uv;
varying vec4 tint;
void main() {
    gl_Position = vec4((2.0 * position / canvas - 1.0) * fit, 0.0, 1.0);
    uv = texcoord;
    tint = color;
}

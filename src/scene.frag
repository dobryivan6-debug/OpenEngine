#version 120
uniform sampler2D image;
uniform sampler2D waterMask;
uniform sampler2D normals;
uniform sampler2D flowMap;
uniform sampler2D irisMask;
uniform int mode;
uniform vec4 water;
uniform vec2 scroll;
uniform vec4 shake;
uniform vec2 iris;
uniform float clockTime;
uniform vec4 pointer;
uniform vec2 screen;
uniform float softness;
varying vec2 uv;
varying vec4 tint;

void main() {
    if (mode == 1) {
        gl_FragColor = texture2D(image, uv) * tint;
        return;
    }
    if (mode == 2) {
        float distance = length(2.0 * uv - 1.0);
        float falloff = 1.0 - smoothstep(mix(0.38, 0.0, softness), 1.0, distance);
        gl_FragColor = vec4(tint.rgb, tint.a * falloff);
        return;
    }

    vec2 sampleAt = uv;
    if (water.x > 0.0) {
        vec2 ratio = vec2(screen.x / screen.y, water.w);
        vec2 motion = clockTime * scroll;
        float phase = clockTime * water.y * water.y;
        vec2 a = texture2D(normals, (uv + phase + motion) * water.z * ratio).xy * 2.0 - 1.0;
        vec2 b = texture2D(normals, (uv * 1.333 - phase + motion) * water.z * ratio).xy * 2.0 - 1.0;
        sampleAt += (a + b) * water.x * water.x * texture2D(waterMask, uv).r;
    }
    if (shake.x > 0.0) {
        vec2 flow = (texture2D(flowMap, uv).rg - vec2(0.498)) * 2.0;
        float wave = clamp((sin(clockTime * shake.y) * 0.5 + 0.5 - shake.z) / shake.w, 0.0, 1.0);
        sampleAt += (2.0 * wave - 1.0) * shake.x * shake.x * flow;
    }
    if (iris.y > 0.0) {
        float phase = clockTime * iris.x;
        sampleAt += vec2(sin(phase * 3.1), sin(phase * 2.3 + 0.8)) * iris.y * texture2D(irisMask, uv).r;
    }
    if (pointer.w > 0.0) {
        vec2 delta = (gl_FragCoord.xy - pointer.xy) / screen.y;
        float radius = length(delta);
        float ring = radius - pointer.z * 0.55;
        float distortion = sin(ring * 105.0) * exp(-abs(ring) * 34.0) * pointer.w;
        sampleAt += delta * distortion * 0.055;
    }
    gl_FragColor = texture2D(image, sampleAt) * tint;
}

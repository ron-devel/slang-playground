// Same over-covering fullscreen-triangle trick as
// renderer-android/src/shaders/fullscreen_triangle.vert (three vertices
// covering the whole viewport, no vertex buffer needed), but this one
// also emits UV coordinates for blit.frag to sample with — the other
// vertex shader has no use for those, since its pipeline never samples
// anything.
//
// Compiled with:
//   glslangValidator -V --target-env vulkan1.2 blit.vert -o blit.vert.spv
#version 450

layout(location = 0) out vec2 uv;

void main() {
    vec2 positions[3] = vec2[](
        vec2(-1.0, -1.0),
        vec2(3.0, -1.0),
        vec2(-1.0, 3.0)
    );
    vec2 position = positions[gl_VertexIndex];
    gl_Position = vec4(position, 0.0, 1.0);
    uv = position * 0.5 + 0.5;
}

// Emits a single triangle whose three vertices over-cover the entire
// viewport ([-1,-1], [3,-1], [-1,3] in NDC), so every pixel in the
// target image is deterministically inside the triangle — no vertex
// buffer needed, and no rasterization-edge ambiguity for the test to
// worry about.
//
// Compiled with:
//   glslangValidator -V --target-env vulkan1.2 fullscreen_triangle.vert -o fullscreen_triangle.vert.spv
#version 450

void main() {
    vec2 positions[3] = vec2[](
        vec2(-1.0, -1.0),
        vec2(3.0, -1.0),
        vec2(-1.0, 3.0)
    );
    gl_Position = vec4(positions[gl_VertexIndex], 0.0, 1.0);
}

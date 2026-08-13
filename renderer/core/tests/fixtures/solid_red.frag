// Outputs solid opaque red for every fragment — paired with
// fullscreen_triangle.vert, this makes every pixel of the render
// target deterministically red, for a graphics-pipeline test to verify
// against.
//
// Compiled with:
//   glslangValidator -V --target-env vulkan1.2 solid_red.frag -o solid_red.frag.spv
#version 450

layout(location = 0) out vec4 out_color;

void main() {
    out_color = vec4(1.0, 0.0, 0.0, 1.0);
}

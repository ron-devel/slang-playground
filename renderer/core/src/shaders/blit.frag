// Samples whatever a compute shader wrote into `output_texture` (see
// SwapchainRenderer::set_compute_shader) and draws it directly — the
// fixed second half of the "compute writes an image, this blits it to
// the swapchain" two-stage shape the web playground's own WebGPU canvas
// also uses.
//
// Compiled with:
//   glslangValidator -V --target-env vulkan1.2 blit.frag -o blit.frag.spv
#version 450

layout(location = 0) in vec2 uv;
layout(location = 0) out vec4 out_color;

layout(binding = 0) uniform sampler2D output_texture;

void main() {
    out_color = texture(output_texture, uv);
}

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
    // Y is flipped here (not just `position * 0.5 + 0.5`): Vulkan's NDC
    // has y=-1 at the top of the screen, but the web playground's own
    // WebGPU pass-through shader (pass_through.ts) was written against
    // WebGPU's y=-1-at-the-bottom NDC convention, mapping that same
    // corner to uv (0, 0). Compute shaders (this crate's and the
    // playground's own) always write outputTexture row 0 as texel row
    // 0 regardless of target — there's no flip inside drawPixel
    // (rendering.slang) — so which screen edge row 0 ends up displayed
    // at is purely a function of this blit/pass-through mapping.
    // Matching the browser's uv assignment exactly (rather than
    // reusing WebGPU's raw NDC-to-uv formula under Vulkan's opposite Y
    // convention) is what keeps a shader's up/down orientation
    // identical on both targets.
    uv = vec2(position.x, -position.y) * 0.5 + 0.5;
}

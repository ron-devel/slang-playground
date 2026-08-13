//! Runs against llvmpipe/lavapipe, same as the other tests here. There's
//! no real swapchain/display available headlessly, so this can't prove
//! full end-to-end presentation — that needs a real device (see
//! renderer-android) — but it does prove create_present_render_pass
//! builds a valid render pass/pipeline/framebuffer combination and
//! actually executes without a Vulkan error, which is the real risk
//! area for a new render-pass variant.

use ash::vk;
use renderer_core::Instance;
use std::sync::Arc;

const WIDTH: u32 = 4;
const HEIGHT: u32 = 4;
const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

#[test]
fn executes_a_render_pass_targeting_present_src_khr() {
    let instance = Arc::new(
        Instance::new("renderer-core tests", &[]).expect("failed to create Vulkan instance"),
    );
    let device = instance
        .create_device(&[])
        .expect("failed to create a logical device");

    let extent = vk::Extent2D {
        width: WIDTH,
        height: HEIGHT,
    };
    // No TRANSFER_SRC usage: unlike graphics_pipeline.rs's test, this
    // doesn't copy the result out — a real swapchain image wouldn't be
    // usable for that here anyway (see module doc comment above).
    let image = device
        .create_color_image(extent, FORMAT, vk::ImageUsageFlags::COLOR_ATTACHMENT)
        .expect("failed to create color image");
    let render_pass = device
        .create_present_render_pass(FORMAT)
        .expect("failed to create present render pass");
    let framebuffer = device
        .create_framebuffer(&render_pass, &image)
        .expect("failed to create framebuffer");

    let vertex_shader = device
        .create_shader_module(include_bytes!("fixtures/fullscreen_triangle.vert.spv"))
        .expect("failed to create vertex shader module");
    let fragment_shader = device
        .create_shader_module(include_bytes!("fixtures/solid_red.frag.spv"))
        .expect("failed to create fragment shader module");
    let pipeline = device
        .create_graphics_pipeline(
            render_pass.handle(),
            &vertex_shader,
            &fragment_shader,
            extent,
        )
        .expect("failed to create graphics pipeline");

    let raw = device.raw();
    unsafe {
        let command_pool = raw
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(device.queue_family_index()),
                None,
            )
            .expect("failed to create command pool");
        let command_buffers = raw
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .expect("failed to allocate command buffer");
        let command_buffer = command_buffers[0];

        raw.begin_command_buffer(command_buffer, &vk::CommandBufferBeginInfo::default())
            .expect("failed to begin command buffer");

        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.0, 0.0, 0.0, 0.0],
            },
        }];
        let render_pass_begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(render_pass.handle())
            .framebuffer(framebuffer.handle())
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            })
            .clear_values(&clear_values);
        raw.cmd_begin_render_pass(
            command_buffer,
            &render_pass_begin_info,
            vk::SubpassContents::INLINE,
        );
        raw.cmd_bind_pipeline(
            command_buffer,
            vk::PipelineBindPoint::GRAPHICS,
            pipeline.pipeline(),
        );
        raw.cmd_draw(command_buffer, 3, 1, 0, 0);
        raw.cmd_end_render_pass(command_buffer);

        raw.end_command_buffer(command_buffer)
            .expect("failed to end command buffer");

        let command_buffers_to_submit = [command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers_to_submit);
        raw.queue_submit(device.queue(), &[submit_info], vk::Fence::null())
            .expect("failed to submit command buffer");
        raw.queue_wait_idle(device.queue())
            .expect("failed to wait for the queue to go idle");

        raw.destroy_command_pool(command_pool, None);
    }
}

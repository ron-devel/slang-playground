//! Runs against llvmpipe/lavapipe, same as the other tests here — no
//! real display, window, or swapchain needed. Renders offscreen into a
//! color image, copies it into a host-visible buffer, and reads the
//! pixels back for real, rather than trusting that no Vulkan call
//! returned an error.
//!
//! Uses a "full-screen triangle" (see fixtures/fullscreen_triangle.vert)
//! that over-covers the entire viewport, paired with a fragment shader
//! that always outputs solid red (fixtures/solid_red.frag) — so every
//! pixel in the target image is deterministically red if the whole
//! pipeline actually ran correctly, with no rasterization-edge ambiguity
//! to reason about.

use ash::vk;
use renderer_core::Instance;
use std::sync::Arc;

const WIDTH: u32 = 4;
const HEIGHT: u32 = 4;
const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

#[test]
fn renders_a_fullscreen_triangle_and_reads_back_the_pixels() {
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
    let image = device
        .create_color_image(
            extent,
            FORMAT,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("failed to create color image");
    let render_pass = device
        .create_render_pass(FORMAT)
        .expect("failed to create render pass");
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
        .create_graphics_pipeline(&render_pass, &vertex_shader, &fragment_shader, extent)
        .expect("failed to create graphics pipeline");

    let buffer_size = (WIDTH * HEIGHT * 4) as vk::DeviceSize;
    let readback_buffer = device
        .create_buffer(buffer_size, vk::BufferUsageFlags::TRANSFER_DST)
        .expect("failed to create readback buffer");

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

        let copy_region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: WIDTH,
                height: HEIGHT,
                depth: 1,
            });
        raw.cmd_copy_image_to_buffer(
            command_buffer,
            image.handle(),
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            readback_buffer.handle(),
            &[copy_region],
        );

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

    let bytes = readback_buffer
        .read()
        .expect("failed to read back the image contents");
    let pixels: Vec<[u8; 4]> = bytes
        .chunks_exact(4)
        .map(|chunk| chunk.try_into().unwrap())
        .collect();

    let expected_pixel = [255u8, 0, 0, 255]; // opaque solid red
    assert!(
        pixels.iter().all(|&pixel| pixel == expected_pixel),
        "expected every pixel to be solid red, got: {pixels:?}"
    );
}

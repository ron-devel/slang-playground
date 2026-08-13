//! Runs against llvmpipe/lavapipe, same as compute_pipeline.rs (which
//! covers the STORAGE_BUFFER descriptor type) — this covers the
//! STORAGE_IMAGE path through the same `create_compute_pipeline`, at a
//! deliberately non-zero, non-default binding to prove the binding/
//! entry-point parameters are actually threaded through correctly,
//! rather than merely working by coincidence with the old hardcoded
//! values. Reads back via a copy to a host-visible buffer, since images
//! (unlike buffers) can't be mapped directly.

use ash::vk;
use renderer_core::Instance;
use std::sync::Arc;

const WIDTH: u32 = 4;
const HEIGHT: u32 = 4;
const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
const BINDING: u32 = 2; // must match `binding = 2` in write_pattern_image.comp

#[test]
fn runs_a_compute_shader_that_writes_an_image_and_reads_back_its_output() {
    let instance = Arc::new(
        Instance::new("renderer-core tests", &[]).expect("failed to create Vulkan instance"),
    );
    let device = instance
        .create_device(&[])
        .expect("failed to create a logical device");

    let spirv = include_bytes!("fixtures/write_pattern_image.comp.spv");
    let shader = device
        .create_shader_module(spirv)
        .expect("failed to create shader module");
    let pipeline = device
        .create_compute_pipeline(&shader, "main", BINDING, vk::DescriptorType::STORAGE_IMAGE)
        .expect("failed to create compute pipeline");

    let extent = vk::Extent2D {
        width: WIDTH,
        height: HEIGHT,
    };
    let image = device
        .create_color_image(
            extent,
            FORMAT,
            vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("failed to create output image");

    let buffer_size = (WIDTH * HEIGHT * 4) as vk::DeviceSize;
    let buffer = device
        .create_buffer(buffer_size, vk::BufferUsageFlags::TRANSFER_DST)
        .expect("failed to create readback buffer");

    let raw = device.raw();
    unsafe {
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_IMAGE)
            .descriptor_count(1)];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(&pool_sizes)
            .max_sets(1);
        let descriptor_pool = raw
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create descriptor pool");

        let set_layouts = [pipeline.descriptor_set_layout()];
        let allocate_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&set_layouts);
        let descriptor_sets = raw
            .allocate_descriptor_sets(&allocate_info)
            .expect("failed to allocate descriptor set");
        let descriptor_set = descriptor_sets[0];

        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(image.view())
            .image_layout(vk::ImageLayout::GENERAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(&image_info);
        raw.update_descriptor_sets(&[write], &[]);

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

        let subresource_range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let to_general = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image.handle())
            .subresource_range(subresource_range)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::SHADER_WRITE);
        raw.cmd_pipeline_barrier(
            command_buffer,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_general],
        );

        raw.cmd_bind_pipeline(
            command_buffer,
            vk::PipelineBindPoint::COMPUTE,
            pipeline.pipeline(),
        );
        raw.cmd_bind_descriptor_sets(
            command_buffer,
            vk::PipelineBindPoint::COMPUTE,
            pipeline.pipeline_layout(),
            0,
            &[descriptor_set],
            &[],
        );
        raw.cmd_dispatch(command_buffer, 1, 1, 1);

        let to_transfer_src = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image.handle())
            .subresource_range(subresource_range)
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        raw.cmd_pipeline_barrier(
            command_buffer,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer_src],
        );

        let region = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_extent(vk::Extent3D {
                width: WIDTH,
                height: HEIGHT,
                depth: 1,
            });
        raw.cmd_copy_image_to_buffer(
            command_buffer,
            image.handle(),
            vk::ImageLayout::GENERAL,
            buffer.handle(),
            &[region],
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
        raw.destroy_descriptor_pool(descriptor_pool, None);
    }

    let bytes = buffer.read().expect("failed to read back buffer contents");
    let expected_pixel = [51u8, 102, 153, 255]; // (0.2, 0.4, 0.6, 1.0) as rgba8
    for (i, pixel) in bytes.chunks_exact(4).enumerate() {
        assert!(
            pixel
                .iter()
                .zip(expected_pixel.iter())
                .all(|(&actual, &expected)| actual.abs_diff(expected) <= 1),
            "pixel {i} was {pixel:?}, expected approximately {expected_pixel:?}"
        );
    }
}

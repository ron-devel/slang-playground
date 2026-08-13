//! Runs against llvmpipe/lavapipe, same as compute_image_pipeline.rs
//! (which covers a single STORAGE_IMAGE binding) — this covers the
//! *multi*-binding path through the same `create_compute_pipeline`
//! (a uniform buffer at binding 0, a storage image at binding 1), the
//! shape SwapchainRenderer actually builds for a compute shader that
//! declares TIME/FRAME_ID uniforms alongside outputTexture.

use ash::vk;
use renderer_core::Instance;
use std::sync::Arc;

const WIDTH: u32 = 4;
const HEIGHT: u32 = 4;
const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
const UNIFORM_BINDING: u32 = 0;
const IMAGE_BINDING: u32 = 1;
const UNIFORM_VALUE: f32 = 0.75;

#[test]
fn runs_a_compute_shader_with_both_a_uniform_buffer_and_an_image_binding() {
    let instance = Arc::new(
        Instance::new("renderer-core tests", &[]).expect("failed to create Vulkan instance"),
    );
    let device = instance
        .create_device(&[])
        .expect("failed to create a logical device");

    let spirv = include_bytes!("fixtures/write_uniform_to_image.comp.spv");
    let shader = device
        .create_shader_module(spirv)
        .expect("failed to create shader module");
    let pipeline = device
        .create_compute_pipeline(
            &shader,
            "main",
            &[
                (UNIFORM_BINDING, vk::DescriptorType::UNIFORM_BUFFER),
                (IMAGE_BINDING, vk::DescriptorType::STORAGE_IMAGE),
            ],
        )
        .expect("failed to create compute pipeline");

    let uniform_buffer = device
        .create_buffer(
            std::mem::size_of::<f32>() as vk::DeviceSize,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
        )
        .expect("failed to create uniform buffer");
    let raw = device.raw();
    unsafe {
        let ptr = raw
            .map_memory(
                uniform_buffer.memory(),
                0,
                std::mem::size_of::<f32>() as vk::DeviceSize,
                vk::MemoryMapFlags::empty(),
            )
            .expect("failed to map uniform buffer");
        (ptr as *mut f32).write_unaligned(UNIFORM_VALUE);
        raw.unmap_memory(uniform_buffer.memory());
    }

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
    let readback_buffer = device
        .create_buffer(buffer_size, vk::BufferUsageFlags::TRANSFER_DST)
        .expect("failed to create readback buffer");

    unsafe {
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1),
        ];
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
        let descriptor_set = raw
            .allocate_descriptor_sets(&allocate_info)
            .expect("failed to allocate descriptor set")[0];

        let buffer_info = [vk::DescriptorBufferInfo::default()
            .buffer(uniform_buffer.handle())
            .offset(0)
            .range(vk::WHOLE_SIZE)];
        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(image.view())
            .image_layout(vk::ImageLayout::GENERAL)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(descriptor_set)
                .dst_binding(UNIFORM_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffer_info),
            vk::WriteDescriptorSet::default()
                .dst_set(descriptor_set)
                .dst_binding(IMAGE_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&image_info),
        ];
        raw.update_descriptor_sets(&writes, &[]);

        let command_pool = raw
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(device.queue_family_index()),
                None,
            )
            .expect("failed to create command pool");
        let command_buffer = raw
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .expect("failed to allocate command buffer")[0];

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
            readback_buffer.handle(),
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

    let bytes = readback_buffer
        .read()
        .expect("failed to read back buffer contents");
    let expected_red = (UNIFORM_VALUE * 255.0).round() as u8;
    for (i, pixel) in bytes.chunks_exact(4).enumerate() {
        assert!(
            pixel[0].abs_diff(expected_red) <= 1
                && pixel[1] == 0
                && pixel[2] == 0
                && pixel[3] == 255,
            "pixel {i} was {pixel:?}, expected approximately [{expected_red}, 0, 0, 255] \
             (the uniform buffer's value read back through the shader into the image)"
        );
    }
}

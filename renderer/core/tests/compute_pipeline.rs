//! Runs against llvmpipe/lavapipe, same as instance.rs/device.rs — no
//! real GPU or display needed. Loads a checked-in, precompiled SPIR-V
//! compute shader (see fixtures/write_pattern.comp) that writes a
//! deterministic pattern into a storage buffer, dispatches it for real,
//! and asserts the buffer readback matches — proving the whole
//! shader-module/pipeline/descriptor-set/dispatch/readback round trip
//! actually executes correctly, not just that no call returned an error.

use ash::vk;
use renderer_core::Instance;

const WORKGROUP_COUNT: u32 = 4;
const LOCAL_SIZE_X: u32 = 64; // must match `local_size_x` in write_pattern.comp
const ELEMENT_COUNT: u32 = WORKGROUP_COUNT * LOCAL_SIZE_X;

#[test]
fn runs_a_compute_shader_and_reads_back_its_output() {
    let instance = Instance::new("renderer-core tests").expect("failed to create Vulkan instance");
    let device = instance
        .create_device()
        .expect("failed to create a logical device");

    let spirv = include_bytes!("fixtures/write_pattern.comp.spv");
    let shader = device
        .create_shader_module(spirv)
        .expect("failed to create shader module");
    let pipeline = device
        .create_compute_pipeline(&shader)
        .expect("failed to create compute pipeline");

    let buffer_size = (ELEMENT_COUNT as usize * std::mem::size_of::<u32>()) as vk::DeviceSize;
    let buffer = device
        .create_buffer(buffer_size, vk::BufferUsageFlags::STORAGE_BUFFER)
        .expect("failed to create buffer");

    let raw = device.raw();
    unsafe {
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
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

        let buffer_info = [vk::DescriptorBufferInfo::default()
            .buffer(buffer.handle())
            .offset(0)
            .range(buffer_size)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&buffer_info);
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
        raw.cmd_dispatch(command_buffer, WORKGROUP_COUNT, 1, 1);
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
    let values: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_ne_bytes(chunk.try_into().unwrap()))
        .collect();

    let expected: Vec<u32> = (0..ELEMENT_COUNT).map(|i| i * 2 + 1).collect();
    assert_eq!(
        values, expected,
        "compute shader output didn't match the expected pattern"
    );
}

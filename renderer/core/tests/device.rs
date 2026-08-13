//! Runs against llvmpipe/lavapipe, same as instance.rs — no real GPU or
//! display needed.

use ash::vk;
use renderer_core::Instance;

#[test]
fn creates_a_device_and_queue_that_can_execute_commands() {
    let instance = Instance::new("renderer-core tests").expect("failed to create Vulkan instance");
    let device = instance
        .create_device()
        .expect("failed to create a logical device");

    let raw = device.raw();

    // Real proof the queue works, not just that creation succeeded:
    // allocate a command buffer, submit a no-op, and wait for it to
    // actually finish executing.
    unsafe {
        let pool = raw
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(device.queue_family_index()),
                None,
            )
            .expect("failed to create a command pool");

        let command_buffers = raw
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .expect("failed to allocate a command buffer");
        let command_buffer = command_buffers[0];

        raw.begin_command_buffer(command_buffer, &vk::CommandBufferBeginInfo::default())
            .expect("failed to begin the command buffer");
        raw.end_command_buffer(command_buffer)
            .expect("failed to end the command buffer");

        let command_buffers_to_submit = [command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers_to_submit);
        raw.queue_submit(device.queue(), &[submit_info], vk::Fence::null())
            .expect("failed to submit the command buffer");
        raw.queue_wait_idle(device.queue())
            .expect("failed to wait for the queue to go idle");

        raw.destroy_command_pool(pool, None);
    }
}

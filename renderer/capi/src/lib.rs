//! Self-contained C ABI for running a single compute shader dispatch
//! end-to-end and reading back its output. Deliberately not exposing
//! granular `Instance`/`Device`/`Buffer`/`Pipeline` handles yet — nothing
//! outside this one call needs to manage their lifetimes across the ABI
//! boundary, and a single "do everything, give me the result" function
//! sidesteps that problem entirely rather than needing the same
//! `Arc`-based ownership treatment `renderer-android`'s JNI shim needed.
//! That can follow later if a real caller needs granular control
//! (persistent resources across multiple dispatches, a real render
//! surface, ...).
//!
//! This crate is specifically for non-JVM embedders (C/C++ on embedded
//! Linux, desktop) — `renderer-android` calls straight into
//! `renderer-core`'s plain Rust API instead, since both sides there are
//! Rust.

use ash::vk;
use renderer_core::{Device, Instance};
use std::slice;
use std::sync::Arc;

/// Deliberately not POSIX `errno` values: those aren't reliably portable
/// across the platforms this project targets (Android, desktop
/// Linux/macOS, eventually Windows), so this is a small self-contained
/// status code instead — same convention as `bridge-adb`'s C ABI.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererCapiStatus {
    Ok = 0,
    ErrorInvalidArgument = 1,
    /// Any Vulkan/driver-level failure (instance/device creation,
    /// pipeline creation, submission, ...). Not split further yet since
    /// no caller has needed to distinguish cases.
    ErrorVulkan = 2,
}

/// Runs a single compute shader dispatch end-to-end: creates a Vulkan
/// instance + device, loads `spirv` as a compute shader, binds a single
/// storage buffer of exactly `out_buffer_len` bytes at binding 0,
/// dispatches `(workgroup_count_x, workgroup_count_y, workgroup_count_z)`
/// workgroups, waits for completion, and copies the buffer's final
/// contents into `out_buffer`.
///
/// # Safety
/// `spirv` must point to `spirv_len` valid, readable bytes. `out_buffer`
/// must point to `out_buffer_len` valid, writable bytes.
#[no_mangle]
pub unsafe extern "C" fn renderer_capi_run_compute_sample(
    spirv: *const u8,
    spirv_len: usize,
    workgroup_count_x: u32,
    workgroup_count_y: u32,
    workgroup_count_z: u32,
    out_buffer: *mut u8,
    out_buffer_len: usize,
) -> RendererCapiStatus {
    if spirv.is_null() || out_buffer.is_null() {
        return RendererCapiStatus::ErrorInvalidArgument;
    }
    // SAFETY: caller's contract, per this function's doc comment above.
    let spirv = slice::from_raw_parts(spirv, spirv_len);

    let result = run_compute_sample(
        spirv,
        [workgroup_count_x, workgroup_count_y, workgroup_count_z],
        out_buffer_len,
    );

    match result {
        Ok(data) => {
            // SAFETY: caller's contract, per this function's doc comment
            // above; `data.len() == out_buffer_len` per
            // `Device::create_buffer`'s exact-size allocation below.
            let out = slice::from_raw_parts_mut(out_buffer, out_buffer_len);
            out.copy_from_slice(&data);
            RendererCapiStatus::Ok
        }
        Err(_) => RendererCapiStatus::ErrorVulkan,
    }
}

fn run_compute_sample(
    spirv: &[u8],
    workgroup_count: [u32; 3],
    buffer_size: usize,
) -> Result<Vec<u8>, renderer_core::Error> {
    let instance = Arc::new(Instance::new("renderer-capi compute sample", &[])?);
    let device = instance.create_device(&[])?;

    let shader = device.create_shader_module(spirv)?;
    let pipeline = device.create_compute_pipeline(
        &shader,
        "main",
        &[(0, vk::DescriptorType::STORAGE_BUFFER)],
    )?;
    let buffer = device.create_buffer(
        buffer_size as vk::DeviceSize,
        vk::BufferUsageFlags::STORAGE_BUFFER,
    )?;

    dispatch_and_wait(&device, &pipeline, &buffer, workgroup_count)?;

    buffer.read()
}

/// Descriptor set / command buffer plumbing for a single dispatch — kept
/// separate from `run_compute_sample` mainly to keep that function
/// readable; not meant to imply this is reusable machinery yet (it isn't
/// — no pooling, no reuse across multiple dispatches).
fn dispatch_and_wait(
    device: &Device,
    pipeline: &renderer_core::ComputePipeline<'_>,
    buffer: &renderer_core::Buffer<'_>,
    workgroup_count: [u32; 3],
) -> Result<(), renderer_core::Error> {
    let raw = device.raw();
    // SAFETY: every object referenced below (`pipeline`, `buffer`) was
    // created from this same `device` and outlives this function call.
    unsafe {
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(&pool_sizes)
            .max_sets(1);
        let descriptor_pool = raw.create_descriptor_pool(&pool_info, None)?;

        let set_layouts = [pipeline.descriptor_set_layout()];
        let allocate_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&set_layouts);
        let descriptor_sets = raw.allocate_descriptor_sets(&allocate_info)?;
        let descriptor_set = descriptor_sets[0];

        let buffer_info = [vk::DescriptorBufferInfo::default()
            .buffer(buffer.handle())
            .offset(0)
            .range(buffer.size())];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&buffer_info);
        raw.update_descriptor_sets(&[write], &[]);

        let command_pool = raw.create_command_pool(
            &vk::CommandPoolCreateInfo::default().queue_family_index(device.queue_family_index()),
            None,
        )?;
        let command_buffers = raw.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )?;
        let command_buffer = command_buffers[0];

        raw.begin_command_buffer(command_buffer, &vk::CommandBufferBeginInfo::default())?;
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
        raw.cmd_dispatch(
            command_buffer,
            workgroup_count[0],
            workgroup_count[1],
            workgroup_count[2],
        );
        raw.end_command_buffer(command_buffer)?;

        let command_buffers_to_submit = [command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers_to_submit);
        raw.queue_submit(device.queue(), &[submit_info], vk::Fence::null())?;
        raw.queue_wait_idle(device.queue())?;

        raw.destroy_command_pool(command_pool, None);
        raw.destroy_descriptor_pool(descriptor_pool, None);
    }

    Ok(())
}

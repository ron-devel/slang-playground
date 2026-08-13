//! A generic swapchain-backed renderer: everything needed to render and
//! present frames to an already-created `vk::SurfaceKHR`, with no
//! knowledge of how that surface or its underlying native window were
//! created. Each platform shim (Android today; Wayland, SDL3/GLFW, ...
//! later) owns the platform-specific bits — building the `Instance` with
//! the right surface extension, creating the `vk::SurfaceKHR` itself, and
//! the native window's own lifetime — and hands this the result.

use crate::{Device, Error, Instance};
use ash::khr;
use ash::vk;
use std::sync::Arc;

/// Owns a swapchain and everything needed to render and present a fixed
/// graphics pipeline to it: per-image framebuffers, a render pass, a
/// pipeline built from the given shader bytes, and single-frame-in-flight
/// synchronization (simplest correct approach for a first working
/// version; multi-frame pipelining can follow once basic rendering is
/// proven across platforms).
///
/// Also owns destroying `surface` (unlike creating it, destruction is
/// generic — `vkDestroySurfaceKHR` needs no platform-specific extension),
/// so callers only need to create it, not tear it down.
pub struct SwapchainRenderer {
    device: Device,
    // Kept alive for as long as `device`/`surface_loader`/etc borrow from
    // it, even though nothing reads this field directly.
    _instance: Arc<Instance>,
    surface_loader: khr::surface::Instance,
    surface: vk::SurfaceKHR,
    extent: vk::Extent2D,
    swapchain_loader: khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    swapchain_image_views: Vec<vk::ImageView>,
    render_pass: vk::RenderPass,
    framebuffers: Vec<vk::Framebuffer>,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    in_flight: vk::Fence,
}

impl SwapchainRenderer {
    /// `surface` must have been created against `instance` (e.g. via a
    /// platform-specific `vkCreate*SurfaceKHR` call) and not yet owned by
    /// anything else — this takes ownership of destroying it.
    /// `initial_extent` is used only if the surface doesn't report its
    /// own current extent (some platforms always report their native
    /// window's actual size instead, in which case this is ignored).
    pub fn new(
        device: Device,
        instance: Arc<Instance>,
        surface: vk::SurfaceKHR,
        initial_extent: vk::Extent2D,
        vertex_shader_spirv: &[u8],
        fragment_shader_spirv: &[u8],
    ) -> Result<Self, Error> {
        let surface_loader = khr::surface::Instance::new(instance.entry(), instance.raw());

        let physical_device = device.physical_device();
        // SAFETY: `physical_device` and `surface` both come from this
        // same live instance.
        let capabilities = unsafe {
            surface_loader.get_physical_device_surface_capabilities(physical_device, surface)?
        };
        let formats = unsafe {
            surface_loader.get_physical_device_surface_formats(physical_device, surface)?
        };
        let present_modes = unsafe {
            surface_loader.get_physical_device_surface_present_modes(physical_device, surface)?
        };

        let surface_format = formats
            .iter()
            .find(|f| f.format == vk::Format::R8G8B8A8_UNORM)
            .or_else(|| formats.first())
            .copied()
            .ok_or(Error::NoSuitableDevice)?;
        // FIFO is the one present mode every Vulkan implementation must
        // support (vsync'd) — simplest safe choice for a first working
        // version; MAILBOX (lower-latency, no tearing) can follow later
        // if it's actually available and worth the complexity.
        let present_mode = present_modes
            .into_iter()
            .find(|&mode| mode == vk::PresentModeKHR::FIFO)
            .unwrap_or(vk::PresentModeKHR::FIFO);

        let extent = if capabilities.current_extent.width != u32::MAX {
            capabilities.current_extent
        } else {
            vk::Extent2D {
                width: initial_extent.width.clamp(
                    capabilities.min_image_extent.width,
                    capabilities.max_image_extent.width,
                ),
                height: initial_extent.height.clamp(
                    capabilities.min_image_extent.height,
                    capabilities.max_image_extent.height,
                ),
            }
        };

        let mut image_count = capabilities.min_image_count + 1;
        if capabilities.max_image_count > 0 {
            image_count = image_count.min(capabilities.max_image_count);
        }

        let swapchain_loader = khr::swapchain::Device::new(instance.raw(), device.raw());
        let swapchain_create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(surface_format.format)
            .image_color_space(surface_format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            .clipped(true);
        // SAFETY: all inputs above were derived from this same physical
        // device/surface combination.
        let swapchain = unsafe { swapchain_loader.create_swapchain(&swapchain_create_info, None)? };
        // SAFETY: `swapchain` was just created from this same device.
        let swapchain_images = unsafe { swapchain_loader.get_swapchain_images(swapchain)? };

        let raw = device.raw();
        let swapchain_image_views: Vec<vk::ImageView> = swapchain_images
            .iter()
            .map(|&image| {
                let create_info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(surface_format.format)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    });
                // SAFETY: `image` came from this same swapchain/device.
                unsafe { raw.create_image_view(&create_info, None) }
            })
            .collect::<Result<_, _>>()?;

        let render_pass = device.create_present_render_pass(surface_format.format)?;

        // SAFETY: each view came from this same device and outlives the
        // framebuffer created from it (both torn down together in Drop).
        let framebuffers: Vec<vk::Framebuffer> = unsafe {
            swapchain_image_views
                .iter()
                .map(|&view| {
                    let attachments = [view];
                    let create_info = vk::FramebufferCreateInfo::default()
                        .render_pass(render_pass.handle())
                        .attachments(&attachments)
                        .width(extent.width)
                        .height(extent.height)
                        .layers(1);
                    raw.create_framebuffer(&create_info, None)
                })
                .collect::<Result<_, _>>()?
        };

        let vertex_shader = device.create_shader_module(vertex_shader_spirv)?;
        let fragment_shader = device.create_shader_module(fragment_shader_spirv)?;
        let pipeline = device.create_graphics_pipeline(
            render_pass.handle(),
            &vertex_shader,
            &fragment_shader,
            extent,
        )?;
        // The pipeline retains what it needs from these at creation time
        // (per the Vulkan spec) and from render_pass's handle below, so
        // it's safe to let vertex_shader/fragment_shader drop here.
        let pipeline_layout = pipeline.pipeline_layout();
        let pipeline_handle = pipeline.pipeline();
        // SAFETY: the pipeline object itself has already copied what it
        // needs from `pipeline`; we're keeping only the raw handles
        // (`pipeline_handle`/`pipeline_layout`) alive ourselves instead
        // of the owning wrapper, so this type can hold `device` and
        // everything derived from it together for its whole lifetime —
        // `renderer-core`'s owning wrapper types (`RenderPass`,
        // `GraphicsPipeline`, `Framebuffer`) borrow `&'a ash::Device`
        // from a `&Device`, which the borrow checker can't express here
        // (a classic self-referential-struct shape). Forgetting rather
        // than dropping `pipeline`/`render_pass` hands their destruction
        // over to this type's own Drop impl instead of leaking them.
        std::mem::forget(pipeline);
        let render_pass_handle = render_pass.handle();
        std::mem::forget(render_pass);
        // ShaderModule's Drop impl doesn't actually need to run (its
        // module was only needed at pipeline-creation time above), but
        // it still borrows `&device` for its whole lifetime as far as
        // the borrow checker is concerned (any type with a destructor
        // has its borrow extended to the drop point, not just last use)
        // — dropping these explicitly now, rather than letting that
        // happen implicitly at the end of this function, is what frees
        // `device` to be moved into the struct below.
        drop(vertex_shader);
        drop(fragment_shader);

        // SAFETY: `device.queue_family_index()` is a valid queue family
        // on this same device.
        let command_pool = unsafe {
            raw.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(device.queue_family_index())
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?
        };
        // SAFETY: `command_pool` was just created from this same device.
        let command_buffer = unsafe {
            raw.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0]
        };

        // SAFETY: no additional invariant beyond a live device.
        let (image_available, render_finished, in_flight) = unsafe {
            let semaphore_info = vk::SemaphoreCreateInfo::default();
            let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
            (
                raw.create_semaphore(&semaphore_info, None)?,
                raw.create_semaphore(&semaphore_info, None)?,
                raw.create_fence(&fence_info, None)?,
            )
        };

        Ok(Self {
            device,
            _instance: instance,
            surface_loader,
            surface,
            extent,
            swapchain_loader,
            swapchain,
            swapchain_image_views,
            render_pass: render_pass_handle,
            framebuffers,
            pipeline_layout,
            pipeline: pipeline_handle,
            command_pool,
            command_buffer,
            image_available,
            render_finished,
            in_flight,
        })
    }

    /// Rebuilds the graphics pipeline from new shader bytes, replacing
    /// the one currently in use — everything else (swapchain, render
    /// pass, framebuffers) is untouched, since only the pipeline depends
    /// on shader content.
    ///
    /// The new pipeline is built *before* the old one is torn down: if
    /// building it fails (e.g. malformed SPIR-V — this is the one piece
    /// of renderer state driven by data arriving over the network, not
    /// something already validated at build time like the other shaders
    /// this crate loads), the old pipeline is left in place and still
    /// working, rather than leaving this renderer with no pipeline at
    /// all over a single bad update.
    pub fn set_shaders(
        &mut self,
        vertex_shader_spirv: &[u8],
        fragment_shader_spirv: &[u8],
    ) -> Result<(), Error> {
        let vertex_shader = self.device.create_shader_module(vertex_shader_spirv)?;
        let fragment_shader = self.device.create_shader_module(fragment_shader_spirv)?;
        let pipeline = self.device.create_graphics_pipeline(
            self.render_pass,
            &vertex_shader,
            &fragment_shader,
            self.extent,
        )?;
        let new_pipeline_layout = pipeline.pipeline_layout();
        let new_pipeline_handle = pipeline.pipeline();
        // SAFETY/ownership: same reasoning as in `new` above — keep only
        // the raw handles, hand their destruction to this type's own
        // Drop impl.
        std::mem::forget(pipeline);
        drop(vertex_shader);
        drop(fragment_shader);

        let raw = self.device.raw();
        // SAFETY: waiting for the device to be idle before destroying
        // the old pipeline — it might still be referenced by a
        // previously submitted, not-yet-finished command buffer (this
        // renderer only ever has one frame in flight, so this is never a
        // meaningful stall in practice).
        unsafe {
            let _ = raw.device_wait_idle();
            raw.destroy_pipeline(self.pipeline, None);
            raw.destroy_pipeline_layout(self.pipeline_layout, None);
        }

        self.pipeline = new_pipeline_handle;
        self.pipeline_layout = new_pipeline_layout;

        Ok(())
    }

    /// Renders and presents one frame. Returns `Ok(false)` (not an
    /// error) when the swapchain is out of date (e.g. the window was
    /// resized) — recreating it is future work; for now this just skips
    /// the frame rather than rendering with stale dimensions.
    pub fn render_frame(&mut self) -> Result<bool, Error> {
        let raw = self.device.raw();
        // SAFETY: `self.in_flight` was signaled by this same device's
        // previous frame (or its own SIGNALED creation flag, the first
        // time).
        unsafe {
            raw.wait_for_fences(&[self.in_flight], true, u64::MAX)?;
        }

        // SAFETY: `self.swapchain` is live and `self.image_available` is
        // not currently pending another wait.
        let acquire_result = unsafe {
            self.swapchain_loader.acquire_next_image(
                self.swapchain,
                u64::MAX,
                self.image_available,
                vk::Fence::null(),
            )
        };
        let image_index = match acquire_result {
            Ok((index, _suboptimal)) => index,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => return Ok(false),
            Err(err) => return Err(err.into()),
        };

        // SAFETY: no other use of `self.in_flight` is pending (we just
        // waited on it above).
        unsafe {
            raw.reset_fences(&[self.in_flight])?;
            raw.reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())?;

            raw.begin_command_buffer(self.command_buffer, &vk::CommandBufferBeginInfo::default())?;
            let clear_values = [vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 1.0],
                },
            }];
            let render_pass_begin_info = vk::RenderPassBeginInfo::default()
                .render_pass(self.render_pass)
                .framebuffer(self.framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.extent,
                })
                .clear_values(&clear_values);
            raw.cmd_begin_render_pass(
                self.command_buffer,
                &render_pass_begin_info,
                vk::SubpassContents::INLINE,
            );
            raw.cmd_bind_pipeline(
                self.command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline,
            );
            raw.cmd_draw(self.command_buffer, 3, 1, 0, 0);
            raw.cmd_end_render_pass(self.command_buffer);
            raw.end_command_buffer(self.command_buffer)?;

            let wait_semaphores = [self.image_available];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let command_buffers = [self.command_buffer];
            let signal_semaphores = [self.render_finished];
            let submit_info = vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&command_buffers)
                .signal_semaphores(&signal_semaphores);
            raw.queue_submit(self.device.queue(), &[submit_info], self.in_flight)?;

            let swapchains = [self.swapchain];
            let image_indices = [image_index];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);
            match self
                .swapchain_loader
                .queue_present(self.device.queue(), &present_info)
            {
                Ok(_) => {}
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => return Ok(false),
                Err(err) => return Err(err.into()),
            }
        }

        Ok(true)
    }
}

impl Drop for SwapchainRenderer {
    fn drop(&mut self) {
        let raw = self.device.raw();
        // SAFETY: waiting for the device to be fully idle before
        // destroying anything it might still be using.
        unsafe {
            let _ = raw.device_wait_idle();

            raw.destroy_fence(self.in_flight, None);
            raw.destroy_semaphore(self.render_finished, None);
            raw.destroy_semaphore(self.image_available, None);
            raw.destroy_command_pool(self.command_pool, None);
            raw.destroy_pipeline(self.pipeline, None);
            raw.destroy_pipeline_layout(self.pipeline_layout, None);
            for &framebuffer in &self.framebuffers {
                raw.destroy_framebuffer(framebuffer, None);
            }
            raw.destroy_render_pass(self.render_pass, None);
            for &view in &self.swapchain_image_views {
                raw.destroy_image_view(view, None);
            }
            self.swapchain_loader
                .destroy_swapchain(self.swapchain, None);
            self.surface_loader.destroy_surface(self.surface, None);
        }
    }
}

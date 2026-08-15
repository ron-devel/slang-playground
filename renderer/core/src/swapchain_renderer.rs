//! A generic swapchain-backed renderer: everything needed to render and
//! present frames to an already-created `vk::SurfaceKHR`, with no
//! knowledge of how that surface or its underlying native window were
//! created. Each platform shim (Android today; Wayland, SDL3/GLFW, ...
//! later) owns the platform-specific bits — building the `Instance` with
//! the right surface extension, creating the `vk::SurfaceKHR` itself, and
//! the native window's own lifetime — and hands this the result.

use crate::{Device, DeviceInfo, Error, Instance};
use ash::khr;
use ash::vk;
use std::sync::Arc;

const BLIT_VERTEX_SHADER: &[u8] = include_bytes!("shaders/blit.vert.spv");
const BLIT_FRAGMENT_SHADER: &[u8] = include_bytes!("shaders/blit.frag.spv");
// Matches the web playground's own `outputTexture` resource, whose
// `[format("rgba8")]` attribute (see `rendering.slang` in the
// slang-compilation-engine package) maps to this Vulkan format.
const COMPUTE_OUTPUT_IMAGE_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
// This crate's own fixed blit shader controls its own binding layout,
// unlike the compute shader's output-texture binding (arbitrary,
// supplied by the caller of `set_compute_shader` — see its docs).
const BLIT_SAMPLER_BINDING: u32 = 0;
// Observed empirically (not documented by the compiler as a guarantee,
// but consistent across every compiled example checked so far): when a
// shader declares global scalar uniforms, Slang always places the
// implicit packed uniform block at descriptor binding 0, shifting
// explicitly-declared resources like the output texture to binding 1+
// instead. `UniformBufferLayout::output_texture_binding` on the caller's
// side already accounts for that shift; this crate only needs to know
// where the uniform block itself lands.
const UNIFORM_BUFFER_BINDING: u32 = 0;

/// Describes the packed uniform buffer a compute shader expects, if any
/// — see `SwapchainRenderer::set_compute_shader`. `size` is the buffer's
/// total byte size; `time_offset`/`frame_id_offset`/
/// `mouse_position_offset` are the byte offsets within it of the
/// auto-provided values this crate knows how to supply on its own
/// (elapsed time in seconds, a monotonically increasing frame counter,
/// and a packed `float4` touch/pointer state — see
/// `SwapchainRenderer::touch_down`) — `None` for any of these means the
/// shader didn't declare that particular one.
pub struct UniformBufferLayout {
    pub size: u32,
    pub time_offset: Option<u32>,
    pub frame_id_offset: Option<u32>,
    pub mouse_position_offset: Option<u32>,
}

/// A compute shader's packed uniform buffer, kept persistently mapped
/// for its whole lifetime — every frame needs to write into it (to
/// refresh time/frame values), so there's nothing to gain from mapping
/// and unmapping around each of those writes instead.
struct UniformBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut u8,
    time_offset: Option<u32>,
    frame_id_offset: Option<u32>,
    mouse_position_offset: Option<u32>,
}

/// Touch/pointer state driving a shader's MOUSE_POSITION uniform (see
/// `SwapchainRenderer::touch_down`/`touch_move`/`touch_up`), tracked the
/// same way the web playground's own canvas does (`RenderCanvas.vue`'s
/// `canvasCurrentMousePos`/`canvasLastMouseDownPos`/`canvasIsMouseDown`/
/// `canvasMouseClicked`) so a shader behaves identically on both
/// targets. Reset on every `set_compute_shader` call, matching the
/// browser resetting its own mouse state on every Run.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct TouchState {
    current_x: f32,
    current_y: f32,
    last_down_x: f32,
    last_down_y: f32,
    is_down: bool,
    ever_clicked: bool,
}

impl TouchState {
    fn down(&mut self, x: f32, y: f32) {
        self.current_x = x;
        self.current_y = y;
        self.last_down_x = x;
        self.last_down_y = y;
        self.is_down = true;
        self.ever_clicked = true;
    }

    fn moved(&mut self, x: f32, y: f32) {
        if self.is_down {
            self.current_x = x;
            self.current_y = y;
        }
    }

    fn up(&mut self) {
        self.is_down = false;
    }

    /// Packs this state into the `float4` layout the web playground's
    /// own MOUSE_POSITION encoding uses (see `RenderCanvas.vue`'s
    /// `writeUniformData`): xy is the current pointer position; zw is
    /// the last touch-down position, sign-encoding "currently
    /// down"/"ever touched" the same way Shadertoy's `iMouse` does.
    fn encode(&self) -> [f32; 4] {
        let down_sign = if self.is_down { -1.0 } else { 1.0 };
        let clicked_sign = if self.ever_clicked { -1.0 } else { 1.0 };
        [
            self.current_x,
            self.current_y,
            self.last_down_x * down_sign,
            self.last_down_y * clicked_sign,
        ]
    }
}

#[cfg(test)]
mod touch_state_tests {
    use super::TouchState;

    #[test]
    fn starts_at_zero_with_positive_signs() {
        assert_eq!(TouchState::default().encode(), [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn down_sets_current_and_last_down_with_both_signs_negative() {
        let mut touch = TouchState::default();
        touch.down(10.0, 20.0);
        assert_eq!(touch.encode(), [10.0, 20.0, -10.0, -20.0]);
    }

    #[test]
    fn move_while_down_updates_current_position_only() {
        let mut touch = TouchState::default();
        touch.down(10.0, 20.0);
        touch.moved(30.0, 40.0);
        assert_eq!(touch.encode(), [30.0, 40.0, -10.0, -20.0]);
    }

    #[test]
    fn move_while_not_down_is_ignored() {
        let mut touch = TouchState::default();
        touch.moved(30.0, 40.0);
        assert_eq!(touch.encode(), [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn up_clears_the_down_sign_but_keeps_the_ever_clicked_sign() {
        let mut touch = TouchState::default();
        touch.down(10.0, 20.0);
        touch.up();
        assert_eq!(touch.encode(), [10.0, 20.0, 10.0, -20.0]);
    }

    #[test]
    fn a_second_down_after_up_overwrites_last_down_and_re_negates_z() {
        let mut touch = TouchState::default();
        touch.down(10.0, 20.0);
        touch.up();
        touch.down(50.0, 60.0);
        assert_eq!(touch.encode(), [50.0, 60.0, -50.0, -60.0]);
    }
}

/// The output image + blit-to-swapchain pass a compute shader set via
/// `SwapchainRenderer::set_compute_shader` renders into, and the fixed
/// pipeline/descriptor set that samples it — built once (the first
/// `set_compute_shader` call) and reused across every later one, since
/// none of it depends on which compute shader is currently writing into
/// the image.
#[derive(Clone, Copy)]
struct BlitResources {
    output_image: vk::Image,
    output_image_view: vk::ImageView,
    output_image_memory: vk::DeviceMemory,
    sampler: vk::Sampler,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
}

/// A compute shader currently active on a `SwapchainRenderer`, dispatched
/// each frame before the blit pass that presents its output. The compute
/// pipeline/descriptor set are rebuilt on every `set_compute_shader` call
/// (the shader itself, and the binding its output texture lives at, can
/// both change); the `blit` half is carried forward unchanged across
/// those calls — see `BlitResources`.
struct ComputeStage {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    thread_group_size: [u32; 3],
    blit: BlitResources,
    uniforms: Option<UniformBuffer>,
}

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
    device_info: DeviceInfo,
    // 2-slot pool (start/end) reused every frame, not one pool per
    // frame-in-flight — this renderer only ever has one frame in flight
    // (see this struct's own docs), so there's only ever one
    // outstanding pair of queries to hold at a time.
    timestamp_query_pool: vk::QueryPool,
    // Nanoseconds per timestamp tick (`VkPhysicalDeviceLimits::timestampPeriod`),
    // needed to convert the raw tick delta `render_frame` reads back
    // into milliseconds.
    timestamp_period_ns: f32,
    // Set once the first frame has been submitted — before that, the
    // query pool holds no results yet to read back (see `render_frame`).
    has_submitted_frame: bool,
    // Updated with one frame of latency: read back at the *start* of
    // the render_frame call *after* the one that recorded it, once the
    // in-flight fence guarantees the GPU has finished writing it —
    // reading it back any earlier would mean stalling this frame's own
    // submission on a GPU round trip.
    last_gpu_frame_time_ms: Option<f32>,
    /// `None` until the first `set_compute_shader` call — until then,
    /// `render_frame` just presents whatever's in `pipeline` directly
    /// (e.g. the default/direct-graphics shader `new` was given, or
    /// whatever `set_shaders` last set), with no compute dispatch.
    compute: Option<ComputeStage>,
    // Source of truth for a compute shader's TIME/FRAME_ID uniforms (see
    // `UniformBufferLayout`) — kept here rather than reset whenever
    // `set_compute_shader` swaps to a new shader, since "time since this
    // renderer started" / "how many frames have been presented" are
    // properties of the renderer's own lifetime, not of whichever
    // specific shader happens to be running right now.
    start_time: std::time::Instant,
    frame_counter: u32,
    // Reset on every `set_compute_shader` call (unlike `start_time`/
    // `frame_counter` above) — see `TouchState`'s docs.
    touch: TouchState,
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

        // Captured before `device` is moved into this struct at the end
        // of this function.
        let device_info = device.info();

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
            None,
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

        // SAFETY: no additional invariant beyond a live device.
        let timestamp_query_pool = unsafe {
            raw.create_query_pool(
                &vk::QueryPoolCreateInfo::default()
                    .query_type(vk::QueryType::TIMESTAMP)
                    .query_count(2),
                None,
            )?
        };
        // SAFETY: `physical_device` came from this same live instance.
        let timestamp_period_ns = unsafe {
            instance
                .raw()
                .get_physical_device_properties(physical_device)
        }
        .limits
        .timestamp_period;

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
            device_info,
            timestamp_query_pool,
            timestamp_period_ns,
            has_submitted_frame: false,
            last_gpu_frame_time_ms: None,
            compute: None,
            start_time: std::time::Instant::now(),
            frame_counter: 0,
            touch: TouchState::default(),
        })
    }

    /// The swapchain's current surface size in pixels — fixed for this
    /// renderer's whole lifetime today (swapchain recreation on resize
    /// is future work, see `render_frame`'s own docs), so callers don't
    /// need to re-query this every frame.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Records a touch/pointer press at `(x, y)` (target's own surface
    /// pixel space, top-left origin) — matches the web playground's
    /// `mousedown` (see `RenderCanvas.vue`): updates both the current
    /// and "last down" position, and marks the pointer down and
    /// (permanently, until the next `set_compute_shader` reset) clicked.
    pub fn touch_down(&mut self, x: f32, y: f32) {
        self.touch.down(x, y);
    }

    /// Updates the current pointer position while it's down — matches
    /// the web playground's `mousemove`, which likewise only tracks
    /// movement while the mouse button is held (a no-op if nothing is
    /// currently down).
    pub fn touch_move(&mut self, x: f32, y: f32) {
        self.touch.moved(x, y);
    }

    /// Records the touch/pointer being released — matches the web
    /// playground's `mouseup`. The "ever clicked" flag set by
    /// `touch_down` is deliberately left alone (see `TouchState`'s
    /// docs).
    pub fn touch_up(&mut self) {
        self.touch.up();
    }

    /// Static GPU/driver identity for this renderer's device — see
    /// `Device::info`.
    pub fn device_info(&self) -> &DeviceInfo {
        &self.device_info
    }

    /// The most recently completed frame's GPU execution time (compute
    /// dispatch + blit/graphics pass, whichever this renderer is
    /// currently doing), in milliseconds. `None` until a frame has
    /// actually finished — see this struct's docs on the one-frame
    /// latency before this updates.
    pub fn last_gpu_frame_time_ms(&self) -> Option<f32> {
        self.last_gpu_frame_time_ms
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
            None,
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

    /// Builds a fresh output image + blit pass — called once, the first
    /// time `set_compute_shader` runs; every later call reuses what this
    /// returned the first time.
    fn create_blit_resources(&self) -> Result<BlitResources, Error> {
        let image = self.device.create_color_image(
            self.extent,
            COMPUTE_OUTPUT_IMAGE_FORMAT,
            vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED,
        )?;
        let output_image = image.handle();
        let output_image_view = image.view();
        let output_image_memory = image.memory();
        // SAFETY/ownership: same self-referential-struct reasoning as
        // everywhere else in this type — keep only the raw handles, hand
        // their destruction to this type's own Drop impl.
        std::mem::forget(image);

        let raw = self.device.raw();
        // SAFETY: no additional invariant beyond a live device.
        let sampler = unsafe {
            let sampler_info = vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .mipmap_mode(vk::SamplerMipmapMode::LINEAR);
            raw.create_sampler(&sampler_info, None)?
        };

        let subresource_range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        // One-time transition out of UNDEFINED. From here on this image
        // stays in GENERAL for its entire lifetime — valid for both a
        // compute shader's writes and the blit pass's reads — so
        // `render_frame` only ever needs a plain execution/memory
        // barrier between those two uses, never another layout
        // transition. Reuses `self.command_buffer` (idle at this point:
        // `set_compute_shader` only ever runs between frames, never
        // during `render_frame`'s own recording of it) rather than
        // allocating a one-shot command buffer just for this.
        // SAFETY: `self.command_buffer` is idle (see above), and
        // `output_image` was just created by this same device.
        unsafe {
            raw.reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())?;
            let begin_info = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            raw.begin_command_buffer(self.command_buffer, &begin_info)?;
            let barrier = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(output_image)
                .subresource_range(subresource_range)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::SHADER_READ);
            raw.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
            raw.end_command_buffer(self.command_buffer)?;
            let command_buffers = [self.command_buffer];
            let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
            raw.queue_submit(self.device.queue(), &[submit_info], vk::Fence::null())?;
            raw.queue_wait_idle(self.device.queue())?;
        }

        // Descriptor set layout/pool/set for the blit pass's combined
        // image sampler — fixed at BLIT_SAMPLER_BINDING, unlike the
        // compute pipeline's output-texture binding (see this type's
        // docs).
        // SAFETY: `output_image_view`/`sampler` were both just created
        // from this same device.
        let (descriptor_set_layout, descriptor_pool, descriptor_set) = unsafe {
            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(BLIT_SAMPLER_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
            let descriptor_set_layout = raw.create_descriptor_set_layout(&layout_info, None)?;

            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)];
            let pool_info = vk::DescriptorPoolCreateInfo::default()
                .pool_sizes(&pool_sizes)
                .max_sets(1);
            let descriptor_pool = raw.create_descriptor_pool(&pool_info, None)?;

            let set_layouts = [descriptor_set_layout];
            let allocate_info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(descriptor_pool)
                .set_layouts(&set_layouts);
            let descriptor_set = raw.allocate_descriptor_sets(&allocate_info)?[0];

            let image_info = [vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(output_image_view)
                .image_layout(vk::ImageLayout::GENERAL)];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(descriptor_set)
                .dst_binding(BLIT_SAMPLER_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&image_info);
            raw.update_descriptor_sets(&[write], &[]);

            (descriptor_set_layout, descriptor_pool, descriptor_set)
        };

        let vertex_shader = self.device.create_shader_module(BLIT_VERTEX_SHADER)?;
        let fragment_shader = self.device.create_shader_module(BLIT_FRAGMENT_SHADER)?;
        let pipeline = self.device.create_graphics_pipeline(
            self.render_pass,
            &vertex_shader,
            &fragment_shader,
            self.extent,
            Some(descriptor_set_layout),
        )?;
        let pipeline_layout = pipeline.pipeline_layout();
        let pipeline_handle = pipeline.pipeline();
        std::mem::forget(pipeline);
        drop(vertex_shader);
        drop(fragment_shader);

        Ok(BlitResources {
            output_image,
            output_image_view,
            output_image_memory,
            sampler,
            pipeline: pipeline_handle,
            pipeline_layout,
            descriptor_set_layout,
            descriptor_pool,
            descriptor_set,
        })
    }

    /// Runs a compute shader each frame against a fixed-size-to-the-
    /// swapchain storage image, then blits the result to the swapchain —
    /// matching the shape a compute-only shading model (like the web
    /// playground's own: a compute entry point writing into a single
    /// `outputTexture`, blitted to the WebGPU canvas by a fixed
    /// pass-through pass) needs, without this crate depending on
    /// anything playground- or protocol-specific.
    ///
    /// `output_texture_binding` is the descriptor binding the compiled
    /// shader expects its output image at — not assumed to be a fixed
    /// constant, since that's a property of how the shader was compiled,
    /// not of this crate.
    ///
    /// Once this has been called, `render_frame` presents the blit pass
    /// instead of `pipeline`/`set_shaders`' pipeline — there's currently
    /// no way back to direct-graphics mode short of dropping this
    /// `SwapchainRenderer` and building a new one, since nothing needs
    /// that yet.
    ///
    /// `uniforms` describes the shader's packed uniform buffer, if it
    /// has one (see `UniformBufferLayout`) — `None` for a shader like
    /// `simple-image.slang` that only touches its output texture.
    pub fn set_compute_shader(
        &mut self,
        compute_shader_spirv: &[u8],
        entry_point: &str,
        thread_group_size: [u32; 3],
        output_texture_binding: u32,
        uniforms: Option<UniformBufferLayout>,
    ) -> Result<(), Error> {
        let blit = match &self.compute {
            Some(existing) => existing.blit,
            None => self.create_blit_resources()?,
        };

        // Matches the web playground resetting its own mouse state on
        // every Run (see `RenderCanvas.vue`'s `resetMouse`) — done
        // unconditionally, not just when the new shader declares
        // MOUSE_POSITION, since "starting a new shader" is the same
        // event either way.
        self.touch = TouchState::default();

        let raw = self.device.raw();

        // Built before the pipeline below (which needs to know whether
        // a second binding is required at all) — torn down on failure
        // automatically by never being stored anywhere if an early `?`
        // returns before reaching `self.compute = Some(...)` at the end.
        let new_uniforms = match uniforms {
            Some(layout) if layout.size > 0 => {
                let buffer = self.device.create_buffer(
                    layout.size as vk::DeviceSize,
                    vk::BufferUsageFlags::UNIFORM_BUFFER,
                )?;
                let buffer_handle = buffer.handle();
                let memory = buffer.memory();
                // SAFETY/ownership: same self-referential-struct
                // reasoning as everywhere else in this type.
                std::mem::forget(buffer);
                // SAFETY: `memory` is this buffer's own, freshly
                // allocated, host-visible + host-coherent memory (see
                // `Device::create_buffer`), not mapped anywhere else.
                let mapped = unsafe {
                    raw.map_memory(
                        memory,
                        0,
                        layout.size as vk::DeviceSize,
                        vk::MemoryMapFlags::empty(),
                    )? as *mut u8
                };
                Some(UniformBuffer {
                    buffer: buffer_handle,
                    memory,
                    mapped,
                    time_offset: layout.time_offset,
                    frame_id_offset: layout.frame_id_offset,
                    mouse_position_offset: layout.mouse_position_offset,
                })
            }
            _ => None,
        };

        let compute_shader = self.device.create_shader_module(compute_shader_spirv)?;
        let mut pipeline_bindings =
            vec![(output_texture_binding, vk::DescriptorType::STORAGE_IMAGE)];
        if new_uniforms.is_some() {
            pipeline_bindings.push((UNIFORM_BUFFER_BINDING, vk::DescriptorType::UNIFORM_BUFFER));
        }
        let pipeline = self.device.create_compute_pipeline(
            &compute_shader,
            entry_point,
            &pipeline_bindings,
        )?;
        let descriptor_set_layout = pipeline.descriptor_set_layout();
        let pipeline_layout = pipeline.pipeline_layout();
        let pipeline_handle = pipeline.pipeline();
        // SAFETY/ownership: same reasoning as in `new`/`set_shaders`.
        std::mem::forget(pipeline);
        drop(compute_shader);

        // SAFETY: `descriptor_set_layout` was just created from this
        // same device; `blit.output_image_view` and (if present)
        // `new_uniforms.buffer` are each this same device's own live
        // resources.
        let (descriptor_pool, descriptor_set) = unsafe {
            let mut pool_sizes = vec![vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)];
            if new_uniforms.is_some() {
                pool_sizes.push(
                    vk::DescriptorPoolSize::default()
                        .ty(vk::DescriptorType::UNIFORM_BUFFER)
                        .descriptor_count(1),
                );
            }
            let pool_info = vk::DescriptorPoolCreateInfo::default()
                .pool_sizes(&pool_sizes)
                .max_sets(1);
            let descriptor_pool = raw.create_descriptor_pool(&pool_info, None)?;
            let set_layouts = [descriptor_set_layout];
            let allocate_info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(descriptor_pool)
                .set_layouts(&set_layouts);
            let descriptor_set = raw.allocate_descriptor_sets(&allocate_info)?[0];

            let image_info = [vk::DescriptorImageInfo::default()
                .image_view(blit.output_image_view)
                .image_layout(vk::ImageLayout::GENERAL)];
            let mut writes = vec![vk::WriteDescriptorSet::default()
                .dst_set(descriptor_set)
                .dst_binding(output_texture_binding)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&image_info)];

            // Declared here (not inside the `if let` below) so it lives
            // long enough for `update_descriptor_sets` at the end of
            // this block — `WriteDescriptorSet` only borrows its info
            // array, it doesn't own it.
            let buffer_info = new_uniforms.as_ref().map(|uniform_buffer| {
                [vk::DescriptorBufferInfo::default()
                    .buffer(uniform_buffer.buffer)
                    .offset(0)
                    .range(vk::WHOLE_SIZE)]
            });
            if let Some(buffer_info) = &buffer_info {
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(UNIFORM_BUFFER_BINDING)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .buffer_info(buffer_info),
                );
            }
            raw.update_descriptor_sets(&writes, &[]);

            (descriptor_pool, descriptor_set)
        };

        // SAFETY: waiting for the device to be idle before destroying
        // the old compute pipeline/descriptor set/uniform buffer (if
        // any) — same reasoning as `set_shaders`. `blit`'s own resources
        // are deliberately left alone here: they're either being
        // carried forward unchanged (the `Some` branch above) or were
        // just created fresh with nothing old to destroy (the `None`
        // branch).
        unsafe {
            let _ = raw.device_wait_idle();
        }
        if let Some(old) = self.compute.take() {
            // SAFETY: this device is idle (just waited above), so
            // nothing is still using these.
            unsafe {
                raw.destroy_descriptor_pool(old.descriptor_pool, None);
                raw.destroy_descriptor_set_layout(old.descriptor_set_layout, None);
                raw.destroy_pipeline(old.pipeline, None);
                raw.destroy_pipeline_layout(old.pipeline_layout, None);
                if let Some(old_uniforms) = old.uniforms {
                    raw.unmap_memory(old_uniforms.memory);
                    raw.destroy_buffer(old_uniforms.buffer, None);
                    raw.free_memory(old_uniforms.memory, None);
                }
            }
        }

        self.compute = Some(ComputeStage {
            pipeline: pipeline_handle,
            pipeline_layout,
            descriptor_set_layout,
            descriptor_pool,
            descriptor_set,
            thread_group_size,
            blit,
            uniforms: new_uniforms,
        });

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

        // The fence wait above guarantees the last submitted frame's
        // command buffer — including the timestamp queries it wrote —
        // has finished executing, so this can't observe a not-yet-ready
        // result; WAIT is passed anyway as a correctness backstop, not
        // because it's expected to actually block. Skipped on the very
        // first frame, before anything has been submitted to read back.
        if self.has_submitted_frame {
            let mut ticks = [0u64; 2];
            // SAFETY: `self.timestamp_query_pool` is live and its two
            // queries were both written by the last submitted frame,
            // per the reasoning above.
            unsafe {
                raw.get_query_pool_results(
                    self.timestamp_query_pool,
                    0,
                    &mut ticks,
                    vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
                )?;
            }
            let elapsed_ticks = ticks[1].wrapping_sub(ticks[0]);
            self.last_gpu_frame_time_ms =
                Some(elapsed_ticks as f32 * self.timestamp_period_ns / 1_000_000.0);
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

            // Must reset before rewriting below — a query pool's slots
            // can't be written twice without a reset between, even
            // across separate submissions like this one is.
            raw.cmd_reset_query_pool(self.command_buffer, self.timestamp_query_pool, 0, 2);
            raw.cmd_write_timestamp(
                self.command_buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                self.timestamp_query_pool,
                0,
            );

            if let Some(compute) = &self.compute {
                if let Some(uniforms) = &compute.uniforms {
                    // Host-side writes through a persistently-mapped,
                    // host-coherent pointer — visible to the GPU with no
                    // explicit flush, and ordered before this same
                    // command buffer's dispatch below by the time the
                    // queue actually executes it (submission happens
                    // after this whole recording block, further down).
                    // SAFETY (both writes below): `uniforms.mapped`
                    // points at this buffer's own live, still-mapped
                    // memory; the offsets came from the caller's own
                    // ShaderUpdate and are trusted the same way
                    // `thread_group_size` and every other
                    // caller-supplied field already is (see
                    // `set_compute_shader`'s docs). Already inside this
                    // function's own outer `unsafe` block.
                    if let Some(offset) = uniforms.time_offset {
                        let elapsed = self.start_time.elapsed().as_secs_f32();
                        uniforms
                            .mapped
                            .add(offset as usize)
                            .cast::<f32>()
                            .write_unaligned(elapsed);
                    }
                    if let Some(offset) = uniforms.frame_id_offset {
                        uniforms
                            .mapped
                            .add(offset as usize)
                            .cast::<f32>()
                            .write_unaligned(self.frame_counter as f32);
                    }
                    if let Some(offset) = uniforms.mouse_position_offset {
                        for (i, value) in self.touch.encode().into_iter().enumerate() {
                            uniforms
                                .mapped
                                .add(offset as usize + i * std::mem::size_of::<f32>())
                                .cast::<f32>()
                                .write_unaligned(value);
                        }
                    }
                }

                raw.cmd_bind_pipeline(
                    self.command_buffer,
                    vk::PipelineBindPoint::COMPUTE,
                    compute.pipeline,
                );
                raw.cmd_bind_descriptor_sets(
                    self.command_buffer,
                    vk::PipelineBindPoint::COMPUTE,
                    compute.pipeline_layout,
                    0,
                    &[compute.descriptor_set],
                    &[],
                );
                let [group_x, group_y, group_z] = compute.thread_group_size;
                // Guards against a divide-by-zero if a bad update
                // somehow supplied a zero component — `thread_group_size`
                // arrives as caller-supplied data (see `set_compute_shader`),
                // not something this crate itself validated.
                let dispatch_x = self.extent.width.div_ceil(group_x.max(1));
                let dispatch_y = self.extent.height.div_ceil(group_y.max(1));
                raw.cmd_dispatch(self.command_buffer, dispatch_x, dispatch_y, group_z.max(1));

                // Compute-write -> fragment-read hazard, both within
                // this same command buffer: `compute.blit.output_image`
                // stays in GENERAL for its whole lifetime (see
                // `create_blit_resources`), so this is a pure
                // execution/memory barrier, not a layout transition.
                let subresource_range = vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                };
                let barrier = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(compute.blit.output_image)
                    .subresource_range(subresource_range)
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ);
                raw.cmd_pipeline_barrier(
                    self.command_buffer,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );

                self.frame_counter = self.frame_counter.wrapping_add(1);
            }

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
            match &self.compute {
                Some(compute) => {
                    raw.cmd_bind_pipeline(
                        self.command_buffer,
                        vk::PipelineBindPoint::GRAPHICS,
                        compute.blit.pipeline,
                    );
                    raw.cmd_bind_descriptor_sets(
                        self.command_buffer,
                        vk::PipelineBindPoint::GRAPHICS,
                        compute.blit.pipeline_layout,
                        0,
                        &[compute.blit.descriptor_set],
                        &[],
                    );
                }
                None => {
                    raw.cmd_bind_pipeline(
                        self.command_buffer,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.pipeline,
                    );
                }
            }
            raw.cmd_draw(self.command_buffer, 3, 1, 0, 0);
            raw.cmd_write_timestamp(
                self.command_buffer,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                self.timestamp_query_pool,
                1,
            );
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
            self.has_submitted_frame = true;

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

            if let Some(compute) = self.compute.take() {
                raw.destroy_descriptor_pool(compute.descriptor_pool, None);
                raw.destroy_descriptor_set_layout(compute.descriptor_set_layout, None);
                raw.destroy_pipeline(compute.pipeline, None);
                raw.destroy_pipeline_layout(compute.pipeline_layout, None);
                if let Some(uniforms) = compute.uniforms {
                    raw.unmap_memory(uniforms.memory);
                    raw.destroy_buffer(uniforms.buffer, None);
                    raw.free_memory(uniforms.memory, None);
                }

                raw.destroy_descriptor_pool(compute.blit.descriptor_pool, None);
                raw.destroy_descriptor_set_layout(compute.blit.descriptor_set_layout, None);
                raw.destroy_pipeline(compute.blit.pipeline, None);
                raw.destroy_pipeline_layout(compute.blit.pipeline_layout, None);
                raw.destroy_sampler(compute.blit.sampler, None);
                raw.destroy_image_view(compute.blit.output_image_view, None);
                raw.destroy_image(compute.blit.output_image, None);
                raw.free_memory(compute.blit.output_image_memory, None);
            }

            raw.destroy_query_pool(self.timestamp_query_pool, None);
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

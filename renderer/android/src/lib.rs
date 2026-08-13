//! JNI shim wiring the Android app's `RenderThread` (Kotlin) to
//! `renderer-core`. Calls straight into `renderer-core`'s plain Rust API
//! — no C ABI round-trip needed here since both sides are Rust; the C
//! ABI (`renderer-capi`) is specifically for non-JVM embedders (C/C++ on
//! embedded Linux, desktop).
//!
//! Owns the swapchain/render-pass/pipeline/framebuffer machinery
//! directly via raw `ash` calls rather than `renderer-core`'s owning
//! wrapper types (`RenderPass`, `GraphicsPipeline`, `Framebuffer`): those
//! borrow `&'a ash::Device` from a `&Device`, which is fine for the
//! short-lived local usage `renderer-core`'s own tests need, but not for
//! `Renderer` here, which holds `Device` *and* everything derived from
//! it together in one struct for its whole lifetime (a classic
//! self-referential-struct shape Rust's borrow checker can't express
//! directly). `Device::raw()` exists exactly for cases like this.

use ash::khr;
use ash::vk;
use jni::objects::{JClass, JObject};
use jni::sys::{jboolean, jlong, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use renderer_core::{Device, Instance};
use std::sync::Arc;

const VERTEX_SHADER: &[u8] = include_bytes!("shaders/fullscreen_triangle.vert.spv");
const FRAGMENT_SHADER: &[u8] = include_bytes!("shaders/solid_red.frag.spv");

/// Owns everything needed to render and present frames to one Android
/// surface: the Vulkan instance/device (via `renderer-core`), the
/// swapchain and its per-image framebuffers, a fixed graphics pipeline
/// (today's hardcoded test triangle — receiving a real shader from the
/// bridge is future work), and single-frame-in-flight synchronization
/// (simplest correct approach for a first working version; multi-frame
/// pipelining can follow once basic rendering is proven).
struct Renderer {
    device: Device,
    // Kept alive for as long as `device`/`surface_loader`/etc borrow
    // from it, even though nothing reads this field directly.
    _instance: Arc<Instance>,
    native_window: *mut ndk_sys::ANativeWindow,
    surface_loader: khr::surface::Instance,
    surface: vk::SurfaceKHR,
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

impl Renderer {
    fn new(native_window: *mut ndk_sys::ANativeWindow) -> Result<Self, renderer_core::Error> {
        let instance = Arc::new(Instance::new(
            "slang-playground-android",
            &[khr::surface::NAME, khr::android_surface::NAME],
        )?);
        let device = instance.create_device(&[khr::swapchain::NAME])?;

        let surface_loader = khr::surface::Instance::new(instance.entry(), instance.raw());
        let android_surface_loader =
            khr::android_surface::Instance::new(instance.entry(), instance.raw());

        // SAFETY: `native_window` is a valid, live ANativeWindow for as
        // long as this Renderer exists (owned by the caller, released in
        // Drop below).
        let surface_create_info = vk::AndroidSurfaceCreateInfoKHR::default()
            .window(native_window as *mut std::ffi::c_void);
        let surface =
            unsafe { android_surface_loader.create_android_surface(&surface_create_info, None)? };

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
            .ok_or(renderer_core::Error::NoSuitableDevice)?;
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
            // SAFETY: `native_window` is valid, per this function's own
            // safety note above.
            let width = unsafe { ndk_sys::ANativeWindow_getWidth(native_window) } as u32;
            let height = unsafe { ndk_sys::ANativeWindow_getHeight(native_window) } as u32;
            vk::Extent2D {
                width: width.clamp(
                    capabilities.min_image_extent.width,
                    capabilities.max_image_extent.width,
                ),
                height: height.clamp(
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

        let vertex_shader = device.create_shader_module(VERTEX_SHADER)?;
        let fragment_shader = device.create_shader_module(FRAGMENT_SHADER)?;
        let pipeline = device.create_graphics_pipeline(
            &render_pass,
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
        // of the owning wrapper, consistent with this struct's approach
        // for render_pass/framebuffers above. Dropping `pipeline` here
        // without calling its Drop impl would leak it, so instead we
        // forget it and take ownership of destroying pipeline_handle/
        // pipeline_layout ourselves in this Renderer's own Drop impl.
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
            native_window,
            surface_loader,
            surface,
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

    /// Renders and presents one frame. Returns `Ok(false)` (not an
    /// error) when the swapchain is out of date (e.g. the window was
    /// resized) — recreating it is future work; for now this just skips
    /// the frame rather than rendering with stale dimensions.
    fn render_frame(&mut self) -> Result<bool, vk::Result> {
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
            Err(err) => return Err(err),
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
                    extent: self.swapchain_extent(),
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
                Err(err) => return Err(err),
            }
        }

        Ok(true)
    }

    fn swapchain_extent(&self) -> vk::Extent2D {
        // SAFETY: `self.native_window` is valid for as long as this
        // Renderer exists.
        vk::Extent2D {
            width: unsafe { ndk_sys::ANativeWindow_getWidth(self.native_window) } as u32,
            height: unsafe { ndk_sys::ANativeWindow_getHeight(self.native_window) } as u32,
        }
    }
}

impl Drop for Renderer {
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
            ndk_sys::ANativeWindow_release(self.native_window);
        }
    }
}

/// # Safety
/// `env`/`surface` must be a valid JNI environment/`android.view.Surface`
/// object pair from the current JNI call.
unsafe fn native_window_from_surface(
    env: &mut JNIEnv,
    surface: &JObject,
) -> Option<*mut ndk_sys::ANativeWindow> {
    // ndk-sys's ANativeWindow_fromSurface resolves to the same jni-sys
    // version the `jni` crate itself uses internally (re-exported as
    // `jni::sys`), so these are used as-is with no cast needed.
    let raw_env = env.get_raw();
    let raw_surface = surface.as_raw();
    // SAFETY: forwarding this function's own safety contract.
    let window = unsafe { ndk_sys::ANativeWindow_fromSurface(raw_env, raw_surface) };
    (!window.is_null()).then_some(window)
}

/// Creates the Vulkan instance/device/surface/swapchain/pipeline for
/// `surface` and returns an opaque handle to it, or `0` on failure
/// (Vulkan/driver errors and a null `ANativeWindow` are expected and
/// recoverable here, so this reports failure via the return value rather
/// than panicking across the JNI boundary, which would abort the
/// process).
#[no_mangle]
pub extern "system" fn Java_dev_slangplayground_app_renderer_RenderThread_nativeCreateRenderer(
    mut env: JNIEnv,
    _class: JClass,
    surface: JObject,
) -> jlong {
    // SAFETY: `env`/`surface` come directly from this JNI call.
    let Some(native_window) = (unsafe { native_window_from_surface(&mut env, &surface) }) else {
        return 0;
    };

    match Renderer::new(native_window) {
        Ok(renderer) => Box::into_raw(Box::new(renderer)) as jlong,
        Err(_) => {
            // SAFETY: `native_window` was just acquired above and
            // nothing else has taken ownership of it since Renderer::new
            // failed before constructing a Renderer to own it.
            unsafe {
                ndk_sys::ANativeWindow_release(native_window);
            }
            0
        }
    }
}

/// Renders and presents one frame. Returns `false` if `handle` is `0` or
/// the frame was skipped (e.g. the swapchain is temporarily out of
/// date), `true` on a normally rendered/presented frame.
///
/// # Safety
/// `handle` must be a value previously returned by `nativeCreateRenderer`
/// on this same library instance, not already passed to
/// `nativeDestroyRenderer`.
#[no_mangle]
pub extern "system" fn Java_dev_slangplayground_app_renderer_RenderThread_nativeRenderFrame(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    if handle == 0 {
        return JNI_FALSE;
    }
    // SAFETY: per this function's contract above.
    let renderer = unsafe { &mut *(handle as *mut Renderer) };
    match renderer.render_frame() {
        Ok(true) => JNI_TRUE,
        Ok(false) | Err(_) => JNI_FALSE,
    }
}

/// Frees a handle returned by `nativeCreateRenderer`. `handle` must not
/// be used afterward. A `0` handle (a prior creation failure) is a no-op.
///
/// # Safety
/// `handle` must be a value previously returned by
/// `nativeCreateRenderer` on this same library instance, not already
/// passed to this function.
#[no_mangle]
pub extern "system" fn Java_dev_slangplayground_app_renderer_RenderThread_nativeDestroyRenderer(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    // SAFETY: per this function's contract above.
    unsafe {
        drop(Box::from_raw(handle as *mut Renderer));
    }
}

//! JNI shim wiring the Android app's `RenderThread` (Kotlin) to
//! `renderer-core`. Calls straight into `renderer-core`'s plain Rust API
//! — no C ABI round-trip needed here since both sides are Rust; the C
//! ABI (`renderer-capi`) is specifically for non-JVM embedders (C/C++ on
//! embedded Linux, desktop).
//!
//! Owns only the genuinely Android-specific slice of getting pixels on
//! screen: building the `Instance` with `VK_KHR_android_surface`,
//! creating a `vk::SurfaceKHR` from an `ANativeWindow`
//! (`ANativeWindow_fromSurface` + `vkCreateAndroidSurfaceKHR`), and that
//! window's own lifetime. Everything from there on — swapchain, render
//! pass, pipeline, per-frame rendering/presentation — is
//! `renderer_core::SwapchainRenderer`, shared with whatever platform shim
//! (Wayland, SDL3/GLFW, ...) comes next.

mod bridge;
mod pending_perf;
mod pending_shader;
mod touch_input;

use ash::khr;
use ash::vk;
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jfloat, jlong, jstring, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use renderer_core::{DeviceInfo, Instance, SwapchainRenderer};
use std::sync::Arc;

const VERTEX_SHADER: &[u8] = include_bytes!("shaders/fullscreen_triangle.vert.spv");
const FRAGMENT_SHADER: &[u8] = include_bytes!("shaders/solid_red.frag.spv");

/// RAII wrapper releasing an `ANativeWindow` acquired via
/// `ANativeWindow_fromSurface`. Kept as its own type (rather than a bare
/// field in `Renderer`) so its `Drop` impl runs automatically in the
/// right order relative to `Renderer`'s other field — see the comment on
/// `Renderer::swapchain_renderer` below.
struct NativeWindow(*mut ndk_sys::ANativeWindow);

impl Drop for NativeWindow {
    fn drop(&mut self) {
        // SAFETY: this pointer is owned solely by this wrapper.
        unsafe {
            ndk_sys::ANativeWindow_release(self.0);
        }
    }
}

/// Renders and presents to one Android surface.
struct Renderer {
    // Declared first so it's dropped — destroying its VkSurfaceKHR along
    // the way — before `_native_window` below releases the window that
    // surface was created from (Rust drops struct fields in declaration
    // order).
    swapchain_renderer: SwapchainRenderer,
    _native_window: NativeWindow,
    // Purely a `PerfSample.frame_id` label for the bridge, distinct from
    // `SwapchainRenderer`'s own internal `frame_counter` (which only
    // advances while a compute shader is set) — this advances on every
    // `render_frame` call, so a `PerfSample` sent while presenting the
    // fixed test triangle still has a meaningful, ever-increasing id.
    frame_counter: u32,
}

impl Renderer {
    fn new(native_window: *mut ndk_sys::ANativeWindow) -> Result<Self, renderer_core::Error> {
        let instance = Arc::new(Instance::new(
            "slang-playground-android",
            &[khr::surface::NAME, khr::android_surface::NAME],
        )?);
        let device = instance.create_device(&[khr::swapchain::NAME])?;

        let android_surface_loader =
            khr::android_surface::Instance::new(instance.entry(), instance.raw());

        // SAFETY: `native_window` is a valid, live ANativeWindow for as
        // long as this function runs — it's released either by the
        // caller (on this function's error paths, since nothing has
        // taken ownership of it yet) or by the `NativeWindow` wrapper
        // constructed below (on success).
        let surface_create_info = vk::AndroidSurfaceCreateInfoKHR::default()
            .window(native_window as *mut std::ffi::c_void);
        let surface =
            unsafe { android_surface_loader.create_android_surface(&surface_create_info, None)? };

        // SAFETY: `native_window` is valid, per this function's own
        // safety note above.
        let initial_extent = vk::Extent2D {
            width: unsafe { ndk_sys::ANativeWindow_getWidth(native_window) } as u32,
            height: unsafe { ndk_sys::ANativeWindow_getHeight(native_window) } as u32,
        };

        let swapchain_renderer = SwapchainRenderer::new(
            device,
            instance,
            surface,
            initial_extent,
            VERTEX_SHADER,
            FRAGMENT_SHADER,
        )?;

        Ok(Self {
            swapchain_renderer,
            _native_window: NativeWindow(native_window),
            frame_counter: 0,
        })
    }

    fn render_frame(&mut self) -> Result<bool, renderer_core::Error> {
        // Checked every frame rather than pushed to the renderer the
        // moment it arrives: the update arrives on the bridge
        // connection's own thread (see `bridge.rs`), which has no direct
        // handle to this Renderer (owned by RenderThread's thread, via
        // an opaque JNI handle) to push it to — the render loop already
        // ticks every frame, so it's the natural place to pick it up
        // instead. A bad update (e.g. malformed SPIR-V) is dropped
        // silently rather than surfaced anywhere: `set_compute_shader`
        // already leaves the previous, still-working pipeline in place
        // on failure, so there's nothing else to do about it here.
        if let Some(update) = pending_shader::take() {
            let uniforms =
                (update.uniform_buffer_size > 0).then_some(renderer_core::UniformBufferLayout {
                    size: update.uniform_buffer_size,
                    time_offset: update.time_offset,
                    frame_id_offset: update.frame_id_offset,
                    mouse_position_offset: update.mouse_position_offset,
                });
            let _ = self.swapchain_renderer.set_compute_shader(
                &update.compute_spirv,
                &update.entry_point,
                update.thread_group_size,
                update.output_texture_binding,
                uniforms,
            );
        }

        // Applied in arrival order, same reasoning as `pending_shader`
        // above but queued rather than latest-wins — see
        // `touch_input`'s docs.
        for event in touch_input::drain() {
            match event {
                touch_input::TouchEvent::Down { x, y } => {
                    self.swapchain_renderer.touch_down(x, y);
                }
                touch_input::TouchEvent::Move { x, y } => {
                    self.swapchain_renderer.touch_move(x, y);
                }
                touch_input::TouchEvent::Up => self.swapchain_renderer.touch_up(),
            }
        }

        self.frame_counter = self.frame_counter.wrapping_add(1);
        let result = self.swapchain_renderer.render_frame();
        if let Some(gpu_time_ms) = self.swapchain_renderer.last_gpu_frame_time_ms() {
            pending_perf::set_perf_sample(pending_perf::PerfSampleRecord {
                frame_id: self.frame_counter,
                gpu_time_ms,
            });
        }
        result
    }

    /// Static GPU/driver identity for this renderer's device, as a JSON
    /// object — queried once by Kotlin at startup (see
    /// `RenderThread.kt`) and merged there with `android.os.Build`
    /// fields (not available to query from here, since those are JVM
    /// statics with no Vulkan equivalent) into the combined `DeviceInfo`
    /// Kotlin then queues via `nativeQueueDeviceInfoForBridge` for
    /// `bridge.rs`'s connection task to relay to the web playground.
    fn device_info_json(&self) -> String {
        device_info_json(self.swapchain_renderer.device_info())
    }

    fn last_gpu_frame_time_ms(&self) -> Option<f32> {
        self.swapchain_renderer.last_gpu_frame_time_ms()
    }
}

/// Escapes `s` for embedding as a JSON string body (between the
/// surrounding quotes) — needed because `gpu_name` comes from the
/// driver, not a value this crate controls the shape of.
fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn device_info_json(info: &DeviceInfo) -> String {
    format!(
        r#"{{"gpuName":"{}","driverVersion":{},"vendorId":{},"deviceId":{},"apiVersion":{}}}"#,
        escape_json_string(&info.gpu_name),
        info.driver_version,
        info.vendor_id,
        info.device_id,
        info.api_version,
    )
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

/// Returns the most recently completed frame's GPU execution time in
/// milliseconds, or `-1.0` if `handle` is `0` or no frame has finished
/// yet (see `SwapchainRenderer::last_gpu_frame_time_ms`'s docs on the
/// one-frame latency).
///
/// # Safety
/// `handle` must be a value previously returned by `nativeCreateRenderer`
/// on this same library instance, not already passed to
/// `nativeDestroyRenderer`.
#[no_mangle]
pub extern "system" fn Java_dev_slangplayground_app_renderer_RenderThread_nativeGetLastGpuFrameTimeMs(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jfloat {
    if handle == 0 {
        return -1.0;
    }
    // SAFETY: per this function's contract above.
    let renderer = unsafe { &*(handle as *const Renderer) };
    renderer.last_gpu_frame_time_ms().unwrap_or(-1.0)
}

/// Returns this renderer's static GPU/driver identity as a JSON object
/// (see `device_info_json`), or `null` if `handle` is `0`.
///
/// # Safety
/// `handle` must be a value previously returned by `nativeCreateRenderer`
/// on this same library instance, not already passed to
/// `nativeDestroyRenderer`.
#[no_mangle]
pub extern "system" fn Java_dev_slangplayground_app_renderer_RenderThread_nativeGetDeviceInfoJson(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    if handle == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: per this function's contract above.
    let renderer = unsafe { &*(handle as *const Renderer) };
    let json = renderer.device_info_json();
    match env.new_string(json) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Queues `DeviceInfo` for `bridge.rs`'s connection task to send once
/// it's next connected (see `pending_perf`). Called once by Kotlin (see
/// `RenderThread.kt`) right after it builds its own `DeviceInfo` —
/// merging this crate's `nativeGetDeviceInfoJson` output with
/// `android.os.Build.*`, which this function's caller already has and
/// this crate has no way to query on its own. No handle parameter, same
/// reasoning as `nativeTouchEvent`: a single global mailbox, since this
/// app only ever has one bridge connection at a time.
///
/// # Safety
/// `env` and every `JString` parameter come directly from this JNI call.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub extern "system" fn Java_dev_slangplayground_app_renderer_RenderThread_nativeQueueDeviceInfoForBridge(
    mut env: JNIEnv,
    _class: JClass,
    gpu_name: JString,
    driver_version: jlong,
    vendor_id: jlong,
    device_id: jlong,
    api_version: jlong,
    android_model: JString,
    android_manufacturer: JString,
    android_release: JString,
    android_sdk_int: jni::sys::jint,
    android_fingerprint: JString,
) {
    let Some(gpu_name) = bridge::jstring_to_string(&mut env, &gpu_name) else {
        return;
    };
    let Some(android_model) = bridge::jstring_to_string(&mut env, &android_model) else {
        return;
    };
    let Some(android_manufacturer) = bridge::jstring_to_string(&mut env, &android_manufacturer)
    else {
        return;
    };
    let Some(android_release) = bridge::jstring_to_string(&mut env, &android_release) else {
        return;
    };
    let Some(android_fingerprint) = bridge::jstring_to_string(&mut env, &android_fingerprint)
    else {
        return;
    };
    pending_perf::set_device_info(pending_perf::DeviceInfoRecord {
        gpu_name,
        driver_version: driver_version as u32,
        vendor_id: vendor_id as u32,
        device_id: device_id as u32,
        api_version: api_version as u32,
        android_model,
        android_manufacturer,
        android_release,
        android_sdk_int: android_sdk_int as u32,
        android_fingerprint,
    });
}

/// Forwards one touch event from `RenderSurfaceView.onTouchEvent` into
/// `touch_input`'s queue, for the render loop to apply on its next
/// frame (see `Renderer::render_frame`). `action` matches
/// `android.view.MotionEvent`'s `ACTION_DOWN`/`ACTION_UP`/`ACTION_MOVE`/
/// `ACTION_CANCEL` constants (0/1/2/3) — the Kotlin side is expected to
/// pass `event.actionMasked`, so multi-touch action codes (e.g.
/// `ACTION_POINTER_DOWN`) never reach here; only the primary pointer's
/// gestures matter for a single MOUSE_POSITION uniform. `ACTION_CANCEL`
/// is treated the same as `ACTION_UP` (the touch ended either way, just
/// not with a normal lift). Any other action code is ignored. There's
/// no handle parameter (unlike the other native functions here): like
/// `pending_shader`, this queue is a single global mailbox, since this
/// app only ever has one `Renderer` at a time.
#[no_mangle]
pub extern "system" fn Java_dev_slangplayground_app_renderer_RenderThread_nativeTouchEvent(
    _env: JNIEnv,
    _class: JClass,
    action: jni::sys::jint,
    x: jni::sys::jfloat,
    y: jni::sys::jfloat,
) {
    let event = match action {
        0 => Some(touch_input::TouchEvent::Down { x, y }),
        1 | 3 => Some(touch_input::TouchEvent::Up),
        2 => Some(touch_input::TouchEvent::Move { x, y }),
        _ => None,
    };
    if let Some(event) = event {
        touch_input::push(event);
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

//! JNI shim wiring the Android app's `RenderThread` (Kotlin) to
//! `renderer-core`. Calls straight into `renderer-core`'s plain Rust API
//! — no C ABI round-trip needed here since both sides are Rust; the C
//! ABI (`renderer-capi`) is specifically for non-JVM embedders (C/C++ on
//! embedded Linux, desktop).
//!
//! Deliberately minimal for now: just proves the whole toolchain path
//! (cargo-ndk cross-compile -> rust-android-gradle -> JNI linkage ->
//! real Vulkan instance/device creation on an actual Android driver)
//! works end to end. Native Vulkan rendering against the surface's
//! `ANativeWindow` is the next increment, once this is confirmed working.

use jni::objects::JClass;
use jni::sys::jlong;
use jni::JNIEnv;
use renderer_core::{Device, Instance};
use std::sync::Arc;

/// Owns the Vulkan instance + device for the lifetime of one Android
/// render surface. Boxed and handed to Kotlin as an opaque `jlong`
/// handle via `nativeCreateRenderer`/`nativeDestroyRenderer` — this is
/// exactly the ownership pattern the `Arc<Instance>` refactor in
/// `renderer-core` exists for for: Kotlin's GC (via `RenderThread`'s
/// explicit lifecycle, not actual GC) decides when this gets freed, not
/// the Rust borrow checker.
struct Renderer {
    #[allow(dead_code)] // not read from yet — proving creation works is the point of this step
    device: Device,
}

/// Creates a Vulkan instance + logical device and returns an opaque
/// handle to it, or `0` on failure (Vulkan/driver errors are expected
/// and recoverable here — e.g. a device with no usable Vulkan driver —
/// so this reports failure via the return value rather than panicking
/// across the JNI boundary, which would abort the process).
#[no_mangle]
pub extern "system" fn Java_dev_slangplayground_app_renderer_RenderThread_nativeCreateRenderer(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    let renderer = (|| -> Result<Renderer, renderer_core::Error> {
        let instance = Arc::new(Instance::new("slang-playground-android")?);
        let device = instance.create_device()?;
        Ok(Renderer { device })
    })();

    match renderer {
        Ok(renderer) => Box::into_raw(Box::new(renderer)) as jlong,
        Err(_) => 0,
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
    // SAFETY: per this function's contract above, `handle` came from
    // `nativeCreateRenderer` and hasn't been freed yet.
    unsafe {
        drop(Box::from_raw(handle as *mut Renderer));
    }
}

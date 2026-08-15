//! Single-slot mailboxes carrying perf data from the render loop
//! (`lib.rs`'s `Renderer::render_frame`, ticking every frame) and from
//! Kotlin (device/OS identity, via JNI) to the bridge connection task
//! (`bridge.rs`, a background thread) — the reverse direction of
//! `pending_shader`'s mailbox, with the same "only the latest matters"
//! reasoning: this app has exactly one `Renderer` and one bridge
//! connection at a time, and a `PerfSample` the bridge task's next poll
//! tick didn't get to before a newer one arrived is no more useful than
//! that newer one. No handle parameter on any of these, same as
//! `touch_input`'s queue — a single global mailbox, since this app only
//! ever has one `Renderer`/bridge connection at a time.

use std::sync::Mutex;

/// Static GPU/driver/OS identity for a `DeviceInfo` bridge message — see
/// `bridge_protocol::DeviceInfo`. Assembled by Kotlin (which already
/// merges `renderer-android`'s own GPU-side fields with
/// `android.os.Build.*`, neither of which `bridge.rs`'s connection task
/// has on its own) rather than reconstructed here.
pub struct DeviceInfoRecord {
    pub gpu_name: String,
    pub driver_version: u32,
    pub vendor_id: u32,
    pub device_id: u32,
    pub api_version: u32,
    pub android_model: String,
    pub android_manufacturer: String,
    pub android_release: String,
    pub android_sdk_int: u32,
    pub android_fingerprint: String,
}

pub struct PerfSampleRecord {
    pub frame_id: u32,
    pub gpu_time_ms: f32,
}

/// Set once, right after Kotlin builds its own `DeviceInfo` (see
/// `RenderThread.kt`). Taken (and cleared) by the bridge task on its
/// next poll tick once a connection exists, so it's sent once per
/// connection rather than repeated every tick — see `bridge.rs`.
static PENDING_DEVICE_INFO: Mutex<Option<DeviceInfoRecord>> = Mutex::new(None);

/// Overwritten every frame; the bridge task sends whatever's latest on
/// its own poll cadence, not necessarily every single frame — see
/// `bridge.rs`.
static PENDING_PERF_SAMPLE: Mutex<Option<PerfSampleRecord>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn set_device_info(info: DeviceInfoRecord) {
    *lock(&PENDING_DEVICE_INFO) = Some(info);
}

pub fn take_device_info() -> Option<DeviceInfoRecord> {
    lock(&PENDING_DEVICE_INFO).take()
}

pub fn set_perf_sample(sample: PerfSampleRecord) {
    *lock(&PENDING_PERF_SAMPLE) = Some(sample);
}

pub fn take_perf_sample() -> Option<PerfSampleRecord> {
    lock(&PENDING_PERF_SAMPLE).take()
}

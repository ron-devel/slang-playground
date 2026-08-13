//! Runs against whatever Vulkan implementation the system's loader
//! finds — in this sandbox and in CI, that's llvmpipe/lavapipe (a
//! software rasterizer, installed via `mesa-vulkan-drivers`), so this
//! needs no real GPU or display to run for real rather than just compile.

use renderer_core::{DeviceKind, Instance};

#[test]
fn creates_an_instance_and_finds_a_physical_device() {
    let instance =
        Instance::new("renderer-core tests", &[]).expect("failed to create Vulkan instance");

    let devices = instance
        .enumerate_physical_devices()
        .expect("failed to enumerate physical devices");

    assert!(
        !devices.is_empty(),
        "expected at least one physical device (e.g. llvmpipe) to be reported"
    );
    assert!(
        devices.iter().any(|d| d.kind == DeviceKind::Cpu),
        "expected a software (CPU) device like llvmpipe among: {devices:?}"
    );
}

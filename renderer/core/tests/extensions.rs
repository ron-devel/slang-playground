//! Runs against llvmpipe/lavapipe, same as the other tests here. Picks
//! whatever extensions the local Vulkan implementation actually reports
//! as available, rather than hardcoding a specific extension name, so
//! this stays portable across whatever machine it runs on.

use renderer_core::Instance;
use std::sync::Arc;

#[test]
fn creates_an_instance_with_an_enabled_extension() {
    // SAFETY: just loading the Vulkan library to query what's available;
    // no other invariant to uphold.
    let entry = unsafe { ash::Entry::load() }.expect("failed to load the Vulkan library");
    let available = unsafe { entry.enumerate_instance_extension_properties(None) }
        .expect("failed to enumerate instance extensions");
    let extension_name = available
        .first()
        .expect("expected at least one instance extension to be available")
        .extension_name_as_c_str()
        .expect("extension name should be a valid C string")
        .to_owned();

    // Creating the instance at all (not erroring) is the real assertion
    // here: an unsupported/unknown extension name would make
    // vkCreateInstance fail with VK_ERROR_EXTENSION_NOT_PRESENT.
    Instance::new("renderer-core tests", &[extension_name.as_c_str()])
        .expect("failed to create an instance with an enabled extension");
}

#[test]
fn creates_a_device_with_an_enabled_extension() {
    let instance = Arc::new(
        Instance::new("renderer-core tests", &[]).expect("failed to create Vulkan instance"),
    );
    let device = instance
        .create_device(&[])
        .expect("failed to create a logical device");

    let available = unsafe {
        instance
            .raw()
            .enumerate_device_extension_properties(device.physical_device())
    }
    .expect("failed to enumerate device extensions");
    let extension_name: std::ffi::CString = available
        .first()
        .expect("expected at least one device extension to be available")
        .extension_name_as_c_str()
        .expect("extension name should be a valid C string")
        .to_owned();

    // Same deal: a device that fails to create with an unsupported
    // extension name would surface as VK_ERROR_EXTENSION_NOT_PRESENT.
    let _device_with_extension = instance
        .create_device(&[extension_name.as_c_str()])
        .expect("failed to create a device with an enabled extension");
}

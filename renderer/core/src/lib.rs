//! Platform-agnostic Vulkan renderer core: no window/surface handling of
//! its own. Each platform (Android, Wayland/embedded Linux, desktop) owns
//! its own thin shim that hands this a native window handle; this crate
//! only knows about the Vulkan instance/device/pipeline machinery shared
//! across all of them.

use ash::vk;
use std::ffi::CStr;
use std::fmt;

#[derive(Debug)]
pub enum Error {
    Loading(ash::LoadingError),
    Vulkan(vk::Result),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Loading(err) => write!(f, "failed to load the Vulkan library: {err}"),
            Error::Vulkan(result) => write!(f, "Vulkan call failed: {result}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<ash::LoadingError> for Error {
    fn from(err: ash::LoadingError) -> Self {
        Error::Loading(err)
    }
}

impl From<vk::Result> for Error {
    fn from(result: vk::Result) -> Self {
        Error::Vulkan(result)
    }
}

/// A coarse, platform-agnostic classification of a physical device, kept
/// separate from `ash::vk::PhysicalDeviceType` so callers outside this
/// crate (eventually including a C ABI) don't need to depend on `ash`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    IntegratedGpu,
    DiscreteGpu,
    VirtualGpu,
    /// A software (CPU) implementation, e.g. llvmpipe/lavapipe — useful
    /// for headless testing where no real GPU is present.
    Cpu,
    Other,
}

impl From<vk::PhysicalDeviceType> for DeviceKind {
    fn from(device_type: vk::PhysicalDeviceType) -> Self {
        match device_type {
            vk::PhysicalDeviceType::INTEGRATED_GPU => DeviceKind::IntegratedGpu,
            vk::PhysicalDeviceType::DISCRETE_GPU => DeviceKind::DiscreteGpu,
            vk::PhysicalDeviceType::VIRTUAL_GPU => DeviceKind::VirtualGpu,
            vk::PhysicalDeviceType::CPU => DeviceKind::Cpu,
            _ => DeviceKind::Other,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PhysicalDeviceInfo {
    pub name: String,
    pub kind: DeviceKind,
}

/// Owns a Vulkan instance. No surface/swapchain/device selection yet —
/// this is deliberately just enough to prove the loader and instance
/// creation work, testable headlessly (no display or real GPU required)
/// against a software Vulkan implementation like llvmpipe.
pub struct Instance {
    // Must outlive `instance`: dropping the loader while the instance is
    // still alive would leave `instance`'s function pointers dangling.
    _entry: ash::Entry,
    instance: ash::Instance,
}

impl Instance {
    pub fn new(app_name: &str) -> Result<Self, Error> {
        // SAFETY: loading the Vulkan library and creating an instance are
        // both inherently unsafe FFI calls into the platform's Vulkan
        // loader; there's no additional invariant this function must
        // uphold beyond what `ash` documents for these calls.
        unsafe {
            let entry = ash::Entry::load()?;

            let app_name = std::ffi::CString::new(app_name).unwrap_or_default();
            let app_info = vk::ApplicationInfo::default()
                .application_name(&app_name)
                .api_version(vk::API_VERSION_1_2);
            let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);

            let instance = entry.create_instance(&create_info, None)?;

            Ok(Self {
                _entry: entry,
                instance,
            })
        }
    }

    pub fn enumerate_physical_devices(&self) -> Result<Vec<PhysicalDeviceInfo>, Error> {
        // SAFETY: `self.instance` is a valid, live instance for as long
        // as `self` exists.
        let devices = unsafe { self.instance.enumerate_physical_devices()? };

        Ok(devices
            .into_iter()
            .map(|device| {
                // SAFETY: `device` came from the enumeration call above
                // against this same live instance.
                let props = unsafe { self.instance.get_physical_device_properties(device) };
                let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
                    .to_string_lossy()
                    .into_owned();
                PhysicalDeviceInfo {
                    name,
                    kind: props.device_type.into(),
                }
            })
            .collect())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: no other Vulkan objects derived from this instance
        // outlive it — this crate doesn't hand out any yet.
        unsafe {
            self.instance.destroy_instance(None);
        }
    }
}

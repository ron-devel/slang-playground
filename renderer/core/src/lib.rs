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
    /// No physical device exposed a queue family supporting both
    /// graphics and compute.
    NoSuitableDevice,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Loading(err) => write!(f, "failed to load the Vulkan library: {err}"),
            Error::Vulkan(result) => write!(f, "Vulkan call failed: {result}"),
            Error::NoSuitableDevice => {
                write!(
                    f,
                    "no physical device with a graphics+compute queue family was found"
                )
            }
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

    /// Picks the first physical device with a queue family supporting
    /// both graphics and compute, and creates a logical device + queue
    /// from it. Selection is deliberately simple for now (first match,
    /// no preference for e.g. a discrete GPU over an integrated one) —
    /// there's no real multi-device scenario to design against yet.
    pub fn create_device(&self) -> Result<Device<'_>, Error> {
        // SAFETY: `self.instance` is a valid, live instance for as long
        // as `self` exists.
        let physical_devices = unsafe { self.instance.enumerate_physical_devices()? };

        let (physical_device, queue_family_index) = physical_devices
            .iter()
            .find_map(|&device| {
                // SAFETY: `device` came from the enumeration call above
                // against this same live instance.
                let queue_families = unsafe {
                    self.instance
                        .get_physical_device_queue_family_properties(device)
                };
                queue_families
                    .iter()
                    .position(|family| {
                        family
                            .queue_flags
                            .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                    })
                    .map(|index| (device, index as u32))
            })
            .ok_or(Error::NoSuitableDevice)?;

        let queue_priorities = [1.0];
        let queue_create_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&queue_priorities)];
        let device_create_info =
            vk::DeviceCreateInfo::default().queue_create_infos(&queue_create_infos);

        // SAFETY: `physical_device` and `queue_family_index` were just
        // read from this same live instance.
        let device = unsafe {
            self.instance
                .create_device(physical_device, &device_create_info, None)?
        };
        // SAFETY: `queue_family_index` was just validated to exist on
        // `physical_device`, and index 0 exists because we requested one
        // queue from it above.
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };

        Ok(Device {
            _instance: self,
            device,
            physical_device,
            queue_family_index,
            queue,
        })
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

/// A logical device and single queue derived from an `Instance`, tied to
/// it by lifetime so the device can never outlive the instance it came
/// from (required — destroying an instance while a device derived from
/// it is still alive is undefined behavior per the Vulkan spec).
pub struct Device<'a> {
    _instance: &'a Instance,
    device: ash::Device,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
    queue: vk::Queue,
}

impl Device<'_> {
    /// Escape hatch to the underlying `ash::Device` for everything this
    /// crate doesn't wrap yet (command pools/buffers, pipelines, ...).
    /// This crate will grow purpose-built wrappers for those as they're
    /// actually needed, rather than modeling the entire Vulkan API
    /// up front.
    pub fn raw(&self) -> &ash::Device {
        &self.device
    }

    pub fn physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    pub fn queue(&self) -> vk::Queue {
        self.queue
    }

    pub fn queue_family_index(&self) -> u32 {
        self.queue_family_index
    }
}

impl Drop for Device<'_> {
    fn drop(&mut self) {
        // SAFETY: no other Vulkan objects derived from this device
        // outlive it — this crate doesn't hand out any yet, and `raw()`
        // callers are responsible for cleaning up anything they create
        // through it before the device is dropped.
        unsafe {
            self.device.destroy_device(None);
        }
    }
}

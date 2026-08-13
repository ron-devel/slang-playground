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
    Io(std::io::Error),
    /// No physical device exposed a queue family supporting both
    /// graphics and compute.
    NoSuitableDevice,
    /// No memory type on the device matched the requested properties
    /// (e.g. host-visible + host-coherent) for a given allocation.
    NoSuitableMemoryType,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Loading(err) => write!(f, "failed to load the Vulkan library: {err}"),
            Error::Vulkan(result) => write!(f, "Vulkan call failed: {result}"),
            Error::Io(err) => write!(f, "I/O error: {err}"),
            Error::NoSuitableDevice => {
                write!(
                    f,
                    "no physical device with a graphics+compute queue family was found"
                )
            }
            Error::NoSuitableMemoryType => {
                write!(f, "no memory type matched the requested properties")
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

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
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
            instance: self,
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
    instance: &'a Instance,
    device: ash::Device,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
    queue: vk::Queue,
}

impl Device<'_> {
    /// Escape hatch to the underlying `ash::Device` for everything this
    /// crate doesn't wrap yet (command pools/buffers, descriptor sets,
    /// ...). This crate will grow purpose-built wrappers for those as
    /// they're actually needed, rather than modeling the entire Vulkan
    /// API up front.
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

    /// Allocates a buffer backed by host-visible, host-coherent memory —
    /// the simplest memory type to read back from the host, which is all
    /// this crate needs today. A device-local + staging-buffer path for
    /// real rendering performance is future work, once there's a reason
    /// to need it.
    pub fn create_buffer(
        &self,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
    ) -> Result<Buffer<'_>, Error> {
        // SAFETY: `self.device` is a valid, live device for as long as
        // `self` exists; `self.physical_device` was validated to support
        // it when this `Device` was created.
        unsafe {
            let buffer_create_info = vk::BufferCreateInfo::default()
                .size(size)
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            let buffer = self.device.create_buffer(&buffer_create_info, None)?;

            let memory_requirements = self.device.get_buffer_memory_requirements(buffer);
            let memory_properties = self
                .instance
                .instance
                .get_physical_device_memory_properties(self.physical_device);
            let memory_type_index = find_memory_type_index(
                &memory_requirements,
                &memory_properties,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or(Error::NoSuitableMemoryType)?;

            let allocate_info = vk::MemoryAllocateInfo::default()
                .allocation_size(memory_requirements.size)
                .memory_type_index(memory_type_index);
            let memory = self.device.allocate_memory(&allocate_info, None)?;
            self.device.bind_buffer_memory(buffer, memory, 0)?;

            Ok(Buffer {
                device: &self.device,
                buffer,
                memory,
                size,
            })
        }
    }

    /// Loads a SPIR-V module from `spirv`. The bytes are the raw
    /// contents of a `.spv` file (e.g. via `include_bytes!`) — not
    /// GLSL/HLSL/Slang source.
    pub fn create_shader_module(&self, spirv: &[u8]) -> Result<ShaderModule<'_>, Error> {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(spirv))?;
        // SAFETY: `self.device` is a valid, live device for as long as
        // `self` exists.
        unsafe {
            let create_info = vk::ShaderModuleCreateInfo::default().code(&code);
            let module = self.device.create_shader_module(&create_info, None)?;
            Ok(ShaderModule {
                device: &self.device,
                module,
            })
        }
    }

    /// Creates a compute pipeline from `shader`'s `"main"` entry point,
    /// with a single descriptor set layout containing one storage-buffer
    /// binding (binding 0) — the minimal shape this crate needs today.
    /// This will grow (multiple bindings, push constants, ...) once a
    /// real shader needs more than that.
    pub fn create_compute_pipeline(
        &self,
        shader: &ShaderModule<'_>,
    ) -> Result<ComputePipeline<'_>, Error> {
        // SAFETY: `self.device` is a valid, live device for as long as
        // `self` exists, and `shader` was created from this same device.
        unsafe {
            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)];
            let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
            let descriptor_set_layout = self
                .device
                .create_descriptor_set_layout(&layout_info, None)?;

            let set_layouts = [descriptor_set_layout];
            let pipeline_layout_info =
                vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
            let pipeline_layout = self
                .device
                .create_pipeline_layout(&pipeline_layout_info, None)?;

            let entry_point = std::ffi::CString::new("main").expect("no interior NUL");
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(shader.module)
                .name(&entry_point);
            let create_info = vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(pipeline_layout);

            let pipelines = self
                .device
                .create_compute_pipelines(vk::PipelineCache::null(), &[create_info], None)
                .map_err(|(_, result)| Error::Vulkan(result))?;

            Ok(ComputePipeline {
                device: &self.device,
                pipeline: pipelines[0],
                pipeline_layout,
                descriptor_set_layout,
            })
        }
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

fn find_memory_type_index(
    requirements: &vk::MemoryRequirements,
    properties: &vk::PhysicalDeviceMemoryProperties,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    (0..properties.memory_type_count).find(|&index| {
        let type_is_allowed = requirements.memory_type_bits & (1 << index) != 0;
        type_is_allowed
            && properties.memory_types[index as usize]
                .property_flags
                .contains(flags)
    })
}

/// A buffer with its own dedicated memory allocation, backed by
/// host-visible/host-coherent memory (see `Device::create_buffer`).
pub struct Buffer<'a> {
    device: &'a ash::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: vk::DeviceSize,
}

impl Buffer<'_> {
    pub fn handle(&self) -> vk::Buffer {
        self.buffer
    }

    pub fn size(&self) -> vk::DeviceSize {
        self.size
    }

    /// Copies this buffer's entire contents out into a fresh `Vec<u8>`.
    /// Valid because this crate only ever creates host-visible,
    /// host-coherent buffers — mapping and reading is always safe to do
    /// directly, no explicit cache-flush step required.
    pub fn read(&self) -> Result<Vec<u8>, Error> {
        // SAFETY: `self.memory` is this buffer's own dedicated
        // allocation, not currently mapped elsewhere, and is
        // host-visible/host-coherent per `Device::create_buffer`.
        unsafe {
            let ptr =
                self.device
                    .map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty())?;
            let mut data = vec![0u8; self.size as usize];
            std::ptr::copy_nonoverlapping(ptr as *const u8, data.as_mut_ptr(), self.size as usize);
            self.device.unmap_memory(self.memory);
            Ok(data)
        }
    }
}

impl Drop for Buffer<'_> {
    fn drop(&mut self) {
        // SAFETY: this buffer and memory are this struct's own, not
        // shared with anything else this crate hands out.
        unsafe {
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

/// A loaded SPIR-V shader module.
pub struct ShaderModule<'a> {
    device: &'a ash::Device,
    module: vk::ShaderModule,
}

impl Drop for ShaderModule<'_> {
    fn drop(&mut self) {
        // SAFETY: this module is this struct's own; any pipeline created
        // from it retains what it needs at creation time per the Vulkan
        // spec, so destroying this after pipeline creation is safe.
        unsafe {
            self.device.destroy_shader_module(self.module, None);
        }
    }
}

/// A compute pipeline created by `Device::create_compute_pipeline`, along
/// with the descriptor set layout and pipeline layout it owns.
pub struct ComputePipeline<'a> {
    device: &'a ash::Device,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
}

impl ComputePipeline<'_> {
    pub fn pipeline(&self) -> vk::Pipeline {
        self.pipeline
    }

    pub fn pipeline_layout(&self) -> vk::PipelineLayout {
        self.pipeline_layout
    }

    pub fn descriptor_set_layout(&self) -> vk::DescriptorSetLayout {
        self.descriptor_set_layout
    }
}

impl Drop for ComputePipeline<'_> {
    fn drop(&mut self) {
        // SAFETY: these are this struct's own objects, not shared with
        // anything else this crate hands out.
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
        }
    }
}

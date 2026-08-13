//! Platform-agnostic Vulkan renderer core: no window/surface handling of
//! its own. Each platform (Android, Wayland/embedded Linux, desktop) owns
//! its own thin shim that hands this a native window handle; this crate
//! only knows about the Vulkan instance/device/pipeline machinery shared
//! across all of them.

use ash::vk;
use std::ffi::CStr;
use std::fmt;
use std::sync::Arc;

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

/// Owns a Vulkan instance. No surface/swapchain support of its own —
/// see [`Instance::raw`] and [`Instance::entry`] for platform shims that
/// need to create one via a platform-specific extension.
pub struct Instance {
    // Must outlive `instance`: dropping the loader while the instance is
    // still alive would leave `instance`'s function pointers dangling.
    entry: ash::Entry,
    instance: ash::Instance,
}

impl Instance {
    /// `required_extensions` are instance extension names to enable
    /// (e.g. `ash::khr::surface::NAME` and a platform-specific surface
    /// extension like `ash::khr::android_surface::NAME`) — this crate
    /// doesn't know or care what they're for; that's entirely up to
    /// whichever per-platform shim is calling this. Empty for the
    /// headless case this crate's own tests use.
    pub fn new(app_name: &str, required_extensions: &[&CStr]) -> Result<Self, Error> {
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
            let extension_ptrs: Vec<*const std::ffi::c_char> =
                required_extensions.iter().map(|ext| ext.as_ptr()).collect();
            let create_info = vk::InstanceCreateInfo::default()
                .application_info(&app_info)
                .enabled_extension_names(&extension_ptrs);

            let instance = entry.create_instance(&create_info, None)?;

            Ok(Self { entry, instance })
        }
    }

    /// Escape hatch to the underlying `ash::Instance`, for platform
    /// shims that need to call extension functions this crate doesn't
    /// wrap (e.g. `vkCreateAndroidSurfaceKHR`).
    pub fn raw(&self) -> &ash::Instance {
        &self.instance
    }

    /// Escape hatch to the underlying `ash::Entry` — needed alongside
    /// [`Instance::raw`] to construct extension function-pointer loaders
    /// like `ash::khr::android_surface::Instance::new(entry, instance)`.
    pub fn entry(&self) -> &ash::Entry {
        &self.entry
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
    ///
    /// `required_device_extensions` are device extension names to enable
    /// (e.g. `ash::khr::swapchain::NAME` for a device that will present
    /// to a swapchain) — same "this crate doesn't know or care what
    /// they're for" deal as `Instance::new`'s `required_extensions`.
    ///
    /// Takes `&Arc<Instance>` (rather than `&self`) because the returned
    /// `Device` shares ownership of the instance via a clone of the Arc —
    /// see `Device`'s docs for why.
    pub fn create_device(
        self: &Arc<Instance>,
        required_device_extensions: &[&CStr],
    ) -> Result<Device, Error> {
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
        let extension_ptrs: Vec<*const std::ffi::c_char> = required_device_extensions
            .iter()
            .map(|ext| ext.as_ptr())
            .collect();
        let device_create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_create_infos)
            .enabled_extension_names(&extension_ptrs);

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
            instance: Arc::clone(self),
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

/// A logical device and single queue derived from an `Instance`, sharing
/// ownership of it via `Arc` so the instance is guaranteed to outlive any
/// `Device` created from it (required — destroying an instance while a
/// device derived from it is still alive is undefined behavior per the
/// Vulkan spec) regardless of who ends up holding the `Device` or for how
/// long. A borrowed lifetime can't express that across an FFI/JNI
/// boundary where the owner is Kotlin's GC or a C caller's manual
/// free-call discipline, not the Rust borrow checker — hence `Arc` rather
/// than the `&'a Instance` this crate used before those embedders
/// existed.
pub struct Device {
    instance: Arc<Instance>,
    device: ash::Device,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
    queue: vk::Queue,
}

impl Device {
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

impl Drop for Device {
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

impl Device {
    /// Creates a 2D color image (with a matching view) backed by its own
    /// dedicated device-local memory allocation — meant to be rendered
    /// into and read back via a copy to a host-visible buffer (see
    /// `create_buffer`), not mapped directly.
    pub fn create_color_image(
        &self,
        extent: vk::Extent2D,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
    ) -> Result<Image<'_>, Error> {
        // SAFETY: `self.device` is a valid, live device for as long as
        // `self` exists; `self.physical_device` was validated to support
        // it when this `Device` was created.
        unsafe {
            let create_info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(format)
                .extent(vk::Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .initial_layout(vk::ImageLayout::UNDEFINED);
            let image = self.device.create_image(&create_info, None)?;

            let memory_requirements = self.device.get_image_memory_requirements(image);
            let memory_properties = self
                .instance
                .instance
                .get_physical_device_memory_properties(self.physical_device);
            let memory_type_index = find_memory_type_index(
                &memory_requirements,
                &memory_properties,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or(Error::NoSuitableMemoryType)?;

            let allocate_info = vk::MemoryAllocateInfo::default()
                .allocation_size(memory_requirements.size)
                .memory_type_index(memory_type_index);
            let memory = self.device.allocate_memory(&allocate_info, None)?;
            self.device.bind_image_memory(image, memory, 0)?;

            let view_create_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            let view = self.device.create_image_view(&view_create_info, None)?;

            Ok(Image {
                device: &self.device,
                image,
                view,
                memory,
                format,
                extent,
            })
        }
    }

    /// Creates a render pass with a single color attachment (cleared on
    /// load, stored, transitioned to `TRANSFER_SRC_OPTIMAL` at the end so
    /// it's immediately ready to copy out of) and a single subpass. This
    /// will grow (depth attachment, multiple subpasses, ...) once a real
    /// use needs more than that.
    pub fn create_render_pass(&self, color_format: vk::Format) -> Result<RenderPass<'_>, Error> {
        // SAFETY: `self.device` is a valid, live device for as long as
        // `self` exists.
        unsafe {
            let attachments = [vk::AttachmentDescription::default()
                .format(color_format)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)];

            let color_attachment_refs = [vk::AttachmentReference::default()
                .attachment(0)
                .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
            let subpasses = [vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&color_attachment_refs)];

            // Without this, there's no ordering guarantee between this
            // subpass's color attachment write and a later transfer read
            // of the same image (e.g. copying it out) — needed for
            // correctness on real hardware/drivers even though a single
            // in-order software rasterizer might happen to work without
            // it.
            let dependencies = [vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_stage_mask(vk::PipelineStageFlags::TRANSFER)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)];

            let create_info = vk::RenderPassCreateInfo::default()
                .attachments(&attachments)
                .subpasses(&subpasses)
                .dependencies(&dependencies);
            let render_pass = self.device.create_render_pass(&create_info, None)?;

            Ok(RenderPass {
                device: &self.device,
                render_pass,
            })
        }
    }

    /// Like `create_render_pass`, but transitions to `PRESENT_SRC_KHR`
    /// instead of `TRANSFER_SRC_OPTIMAL` — for presenting to a swapchain
    /// rather than copying the result out to a buffer. Kept as a
    /// separate method rather than a parameterized variant of
    /// `create_render_pass`, since the correct subpass dependency
    /// differs too, not just the final layout: presenting needs an
    /// *entry* dependency (wait for the acquire semaphore before writing
    /// the color attachment) rather than the *exit* dependency
    /// `create_render_pass` has (order the write before a later transfer
    /// read) — synchronizing the actual present call itself is handled
    /// separately, via the semaphore passed to `vkQueuePresentKHR`, not
    /// a subpass dependency here.
    pub fn create_present_render_pass(
        &self,
        color_format: vk::Format,
    ) -> Result<RenderPass<'_>, Error> {
        // SAFETY: `self.device` is a valid, live device for as long as
        // `self` exists.
        unsafe {
            let attachments = [vk::AttachmentDescription::default()
                .format(color_format)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];

            let color_attachment_refs = [vk::AttachmentReference::default()
                .attachment(0)
                .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
            let subpasses = [vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&color_attachment_refs)];

            let dependencies = [vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];

            let create_info = vk::RenderPassCreateInfo::default()
                .attachments(&attachments)
                .subpasses(&subpasses)
                .dependencies(&dependencies);
            let render_pass = self.device.create_render_pass(&create_info, None)?;

            Ok(RenderPass {
                device: &self.device,
                render_pass,
            })
        }
    }

    /// Creates a framebuffer binding `image` as the render pass's single
    /// color attachment.
    pub fn create_framebuffer(
        &self,
        render_pass: &RenderPass<'_>,
        image: &Image<'_>,
    ) -> Result<Framebuffer<'_>, Error> {
        // SAFETY: `self.device` is a valid, live device for as long as
        // `self` exists, and `render_pass`/`image` were created from
        // this same device.
        unsafe {
            let attachments = [image.view];
            let create_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass.render_pass)
                .attachments(&attachments)
                .width(image.extent.width)
                .height(image.extent.height)
                .layers(1);
            let framebuffer = self.device.create_framebuffer(&create_info, None)?;

            Ok(Framebuffer {
                device: &self.device,
                framebuffer,
            })
        }
    }

    /// Creates a graphics pipeline with no vertex input state (the
    /// vertex shader is expected to generate its own positions, e.g.
    /// from `gl_VertexIndex`, as this crate's own tests do) and no
    /// descriptor sets — the minimal shape needed today. Viewport and
    /// scissor are fixed at `extent` rather than dynamic state, since
    /// there's no real resize scenario to design against yet. This will
    /// grow (vertex input layout, descriptor sets, dynamic viewport,
    /// depth/stencil, blending, ...) once real mesh/material rendering
    /// needs it.
    pub fn create_graphics_pipeline(
        &self,
        render_pass: &RenderPass<'_>,
        vertex_shader: &ShaderModule<'_>,
        fragment_shader: &ShaderModule<'_>,
        extent: vk::Extent2D,
    ) -> Result<GraphicsPipeline<'_>, Error> {
        // SAFETY: `self.device` is a valid, live device for as long as
        // `self` exists, and `render_pass`/`vertex_shader`/
        // `fragment_shader` were created from this same device.
        unsafe {
            let pipeline_layout_info = vk::PipelineLayoutCreateInfo::default();
            let pipeline_layout = self
                .device
                .create_pipeline_layout(&pipeline_layout_info, None)?;

            let entry_point = std::ffi::CString::new("main").expect("no interior NUL");
            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(vertex_shader.module)
                    .name(&entry_point),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(fragment_shader.module)
                    .name(&entry_point),
            ];

            let vertex_input_state = vk::PipelineVertexInputStateCreateInfo::default();
            let input_assembly_state = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

            let viewports = [vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: extent.width as f32,
                height: extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            }];
            let scissors = [vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            }];
            let viewport_state = vk::PipelineViewportStateCreateInfo::default()
                .viewports(&viewports)
                .scissors(&scissors);

            let rasterization_state = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                .line_width(1.0);

            let multisample_state = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);

            let color_blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)];
            let color_blend_state = vk::PipelineColorBlendStateCreateInfo::default()
                .attachments(&color_blend_attachments);

            let create_info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input_state)
                .input_assembly_state(&input_assembly_state)
                .viewport_state(&viewport_state)
                .rasterization_state(&rasterization_state)
                .multisample_state(&multisample_state)
                .color_blend_state(&color_blend_state)
                .layout(pipeline_layout)
                .render_pass(render_pass.render_pass)
                .subpass(0);

            let pipelines = self
                .device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[create_info], None)
                .map_err(|(_, result)| Error::Vulkan(result))?;

            Ok(GraphicsPipeline {
                device: &self.device,
                pipeline: pipelines[0],
                pipeline_layout,
            })
        }
    }
}

/// A 2D color image with its own view and dedicated device-local memory
/// allocation, meant to be rendered into and read back via a copy to a
/// host-visible buffer — there's no direct host mapping of image memory.
pub struct Image<'a> {
    device: &'a ash::Device,
    image: vk::Image,
    view: vk::ImageView,
    memory: vk::DeviceMemory,
    format: vk::Format,
    extent: vk::Extent2D,
}

impl Image<'_> {
    pub fn handle(&self) -> vk::Image {
        self.image
    }

    pub fn view(&self) -> vk::ImageView {
        self.view
    }

    pub fn format(&self) -> vk::Format {
        self.format
    }

    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }
}

impl Drop for Image<'_> {
    fn drop(&mut self) {
        // SAFETY: this image, view, and memory are this struct's own,
        // not shared with anything else this crate hands out.
        unsafe {
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

/// A render pass created by `Device::create_render_pass`.
pub struct RenderPass<'a> {
    device: &'a ash::Device,
    render_pass: vk::RenderPass,
}

impl RenderPass<'_> {
    pub fn handle(&self) -> vk::RenderPass {
        self.render_pass
    }
}

impl Drop for RenderPass<'_> {
    fn drop(&mut self) {
        // SAFETY: this render pass is this struct's own.
        unsafe {
            self.device.destroy_render_pass(self.render_pass, None);
        }
    }
}

/// A framebuffer created by `Device::create_framebuffer`.
pub struct Framebuffer<'a> {
    device: &'a ash::Device,
    framebuffer: vk::Framebuffer,
}

impl Framebuffer<'_> {
    pub fn handle(&self) -> vk::Framebuffer {
        self.framebuffer
    }
}

impl Drop for Framebuffer<'_> {
    fn drop(&mut self) {
        // SAFETY: this framebuffer is this struct's own.
        unsafe {
            self.device.destroy_framebuffer(self.framebuffer, None);
        }
    }
}

/// A graphics pipeline created by `Device::create_graphics_pipeline`,
/// along with the pipeline layout it owns.
pub struct GraphicsPipeline<'a> {
    device: &'a ash::Device,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
}

impl GraphicsPipeline<'_> {
    pub fn pipeline(&self) -> vk::Pipeline {
        self.pipeline
    }

    pub fn pipeline_layout(&self) -> vk::PipelineLayout {
        self.pipeline_layout
    }
}

impl Drop for GraphicsPipeline<'_> {
    fn drop(&mut self) {
        // SAFETY: these are this struct's own objects, not shared with
        // anything else this crate hands out.
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
        }
    }
}

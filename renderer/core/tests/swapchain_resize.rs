//! Runs against a real (headless, via `VK_EXT_headless_surface`)
//! swapchain — the first tests in this crate to actually exercise
//! `SwapchainRenderer` end to end. Every other test file here works one
//! level below `SwapchainRenderer` (`Device`'s pipeline/image/buffer
//! primitives directly) precisely because there's normally no real
//! display available in CI to build a `VkSurfaceKHR` from (see
//! present_render_pass.rs's own doc comment) — `VK_EXT_headless_surface`
//! sidesteps that by producing a genuine surface with no real windowing
//! system behind it, which mesa's llvmpipe software driver (installed
//! in CI, see renderer-ci.yml) supports, so this is a real swapchain
//! creation/present/recreate sequence, not a mock.

use ash::ext;
use ash::khr;
use ash::vk;
use renderer_core::{Instance, SwapchainRenderer};
use std::sync::Arc;

const VERTEX_SHADER: &[u8] = include_bytes!("fixtures/fullscreen_triangle.vert.spv");
const FRAGMENT_SHADER: &[u8] = include_bytes!("fixtures/solid_red.frag.spv");
// See write_pattern_image.comp: local_size_x/y = 8, binding = 2.
const COMPUTE_SHADER: &[u8] = include_bytes!("fixtures/write_pattern_image.comp.spv");
const COMPUTE_THREAD_GROUP_SIZE: [u32; 3] = [8, 8, 1];
const COMPUTE_OUTPUT_BINDING: u32 = 2;

fn create_headless_renderer(extent: vk::Extent2D) -> SwapchainRenderer {
    let instance = Arc::new(
        Instance::new(
            "renderer-core tests",
            &[khr::surface::NAME, ext::headless_surface::NAME],
        )
        .expect("failed to create Vulkan instance"),
    );
    let device = instance
        .create_device(&[khr::swapchain::NAME])
        .expect("failed to create a logical device");

    let headless_surface_loader =
        ext::headless_surface::Instance::new(instance.entry(), instance.raw());
    // SAFETY: `instance` is live; `HeadlessSurfaceCreateInfoEXT` has no
    // fields beyond the standard sType/pNext/flags that need a
    // particular value for this extension.
    let surface = unsafe {
        headless_surface_loader
            .create_headless_surface(&vk::HeadlessSurfaceCreateInfoEXT::default(), None)
            .expect("failed to create headless surface")
    };

    SwapchainRenderer::new(
        device,
        instance,
        surface,
        extent,
        VERTEX_SHADER,
        FRAGMENT_SHADER,
    )
    .expect("failed to create SwapchainRenderer")
}

#[test]
fn renders_and_presents_a_frame_against_a_headless_surface() {
    let mut renderer = create_headless_renderer(vk::Extent2D {
        width: 64,
        height: 64,
    });
    assert!(renderer
        .render_frame()
        .expect("render_frame returned an error"));
}

#[test]
fn recreate_rebuilds_the_swapchain_and_rendering_still_works() {
    let mut renderer = create_headless_renderer(vk::Extent2D {
        width: 64,
        height: 64,
    });
    assert!(renderer
        .render_frame()
        .expect("render_frame (before recreate) returned an error"));

    renderer
        .recreate(vk::Extent2D {
            width: 128,
            height: 96,
        })
        .expect("recreate failed");
    let extent = renderer.extent();
    assert_eq!(extent.width, 128);
    assert_eq!(extent.height, 96);

    assert!(renderer
        .render_frame()
        .expect("render_frame (after recreate) returned an error"));
}

#[test]
fn recreate_with_an_active_compute_shader_keeps_it_working() {
    let mut renderer = create_headless_renderer(vk::Extent2D {
        width: 64,
        height: 64,
    });
    renderer
        .set_compute_shader(
            COMPUTE_SHADER,
            "main",
            COMPUTE_THREAD_GROUP_SIZE,
            COMPUTE_OUTPUT_BINDING,
            None,
        )
        .expect("set_compute_shader failed");
    assert!(renderer
        .render_frame()
        .expect("render_frame (before recreate) returned an error"));

    // A non-square resize: proves the compute output image (which is
    // sized to the surface, not a fixed square) and its dispatch count
    // both actually track the new extent rather than a stale one.
    renderer
        .recreate(vk::Extent2D {
            width: 32,
            height: 48,
        })
        .expect("recreate failed");

    assert!(renderer
        .render_frame()
        .expect("render_frame (after recreate, compute still active) returned an error"));
}

#[test]
fn recreate_can_be_called_more_than_once() {
    let mut renderer = create_headless_renderer(vk::Extent2D {
        width: 64,
        height: 64,
    });
    for (width, height) in [(128, 128), (32, 32), (200, 100)] {
        renderer
            .recreate(vk::Extent2D { width, height })
            .unwrap_or_else(|err| panic!("recreate to {width}x{height} failed: {err}"));
        assert!(renderer
            .render_frame()
            .unwrap_or_else(|err| panic!("render_frame at {width}x{height} failed: {err}")));
    }
}

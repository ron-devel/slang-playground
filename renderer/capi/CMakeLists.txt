# Imports the already-built renderer-capi Rust library rather than
# driving the Cargo build itself. Before configuring, from the
# `renderer` directory:
#   cargo build [--release] -p renderer-capi

# Defaults to matching CMAKE_BUILD_TYPE (Release/RelWithDebInfo/MinSizeRel
# -> Rust "release", anything else -> "debug") — still overridable with
# -DRENDERER_CAPI_RUST_PROFILE=..., since that pre-populates the cache
# entry before this runs and the block below is skipped in that case.
if(NOT DEFINED RENDERER_CAPI_RUST_PROFILE)
    if(CMAKE_BUILD_TYPE MATCHES "^(Release|RelWithDebInfo|MinSizeRel)$")
        set(RENDERER_CAPI_RUST_PROFILE "release")
    else()
        set(RENDERER_CAPI_RUST_PROFILE "debug")
    endif()
endif()
set(RENDERER_CAPI_RUST_PROFILE "${RENDERER_CAPI_RUST_PROFILE}" CACHE STRING "Cargo profile to link against (debug or release)")
set(RENDERER_CAPI_TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../target/${RENDERER_CAPI_RUST_PROFILE}")
set(RENDERER_CAPI_LIBRARY "${RENDERER_CAPI_TARGET_DIR}/librenderer_capi${CMAKE_SHARED_LIBRARY_SUFFIX}")

if(NOT EXISTS "${RENDERER_CAPI_LIBRARY}")
    if(RENDERER_CAPI_RUST_PROFILE STREQUAL "release")
        set(_cargo_build_cmd "cargo build --release -p renderer-capi")
    else()
        set(_cargo_build_cmd "cargo build -p renderer-capi")
    endif()
    message(WARNING
        "renderer-capi has not been built for the '${RENDERER_CAPI_RUST_PROFILE}' "
        "profile yet (expected ${RENDERER_CAPI_LIBRARY}). Run "
        "'${_cargo_build_cmd}' from the renderer/ directory first."
    )
endif()

# SHARED rather than STATIC: a shared library resolves Rust's own runtime
# dependencies (pthread, etc.) internally, so consumers just link
# renderer_capi with no platform-specific extra libs to guess at.
add_library(renderer_capi SHARED IMPORTED GLOBAL)
set_target_properties(renderer_capi PROPERTIES
    IMPORTED_LOCATION "${RENDERER_CAPI_LIBRARY}"
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_SOURCE_DIR}/include"
)

# Vendored Khronos Vulkan-Headers (see third_party/vulkan-headers/VENDORED.md)
# — today's sample doesn't need them (the C ABI only uses plain C types,
# no raw Vulkan handles cross the boundary yet), but they're wired in now
# for when the C ABI or a future sample needs to touch Vulkan types
# directly, without depending on whatever Vulkan SDK, if any, happens to
# be installed on a given build machine.
set(RENDERER_CAPI_VULKAN_HEADERS_DIR "${CMAKE_CURRENT_SOURCE_DIR}/third_party/vulkan-headers/include")

if(BUILD_SAMPLES)
    add_executable(renderer_capi_compute_sample examples/c/compute_sample.c)
    target_link_libraries(renderer_capi_compute_sample PRIVATE renderer_capi)
    target_include_directories(renderer_capi_compute_sample PRIVATE
        "${RENDERER_CAPI_VULKAN_HEADERS_DIR}"
    )
    # Baked in as an absolute path rather than assumed relative to
    # whatever directory the built binary happens to be run from.
    target_compile_definitions(renderer_capi_compute_sample PRIVATE
        "WRITE_PATTERN_SPV_PATH=\"${CMAKE_CURRENT_SOURCE_DIR}/examples/fixtures/write_pattern.comp.spv\""
    )
    # So the built executable finds librenderer_capi without needing
    # LD_LIBRARY_PATH set manually.
    set_target_properties(renderer_capi_compute_sample PROPERTIES
        BUILD_RPATH "${RENDERER_CAPI_TARGET_DIR}"
    )
endif()

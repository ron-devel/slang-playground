# Vendored: Khronos Vulkan-Headers

Source: https://github.com/KhronosGroup/Vulkan-Headers
Version: v1.3.290 (commit tag)
License: Apache-2.0 (see LICENSE.md / LICENSES/)

Only the plain C headers (`include/vulkan/*.h`, `include/vk_video/*.h`) are
vendored here — not the C++ (`.hpp`/`.cppm`) bindings, registry XML, or
docs, since this repo's C/C++ side only needs the C API.

Vendored directly (not a git submodule) so this builds standalone without
depending on whatever Vulkan SDK, if any, happens to be installed on a
given machine. To update: re-clone the desired tag and replace the
contents of `include/` with its `include/vulkan/*.h` and
`include/vk_video/*.h`.

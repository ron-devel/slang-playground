/* Loads the same compute shader renderer-core's own Rust tests use
 * (../fixtures/write_pattern.comp.spv — writes `idx * 2 + 1` into a
 * storage buffer, deterministic and easy to check), runs it through the
 * C ABI end to end, and verifies the result — proving the C ABI works
 * for real, not just that it links.
 *
 * Build with CMake (from the `renderer` directory; add
 * -DRENDERER_CAPI_RUST_PROFILE=release to match a --release cargo build):
 *   cargo build -p renderer-capi
 *   cmake -S . -B build && cmake --build build
 *   ./build/capi/renderer_capi_compute_sample
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include "renderer_capi.h"

#define WORKGROUP_COUNT 4
#define LOCAL_SIZE_X 64 /* must match local_size_x in write_pattern.comp */
#define ELEMENT_COUNT (WORKGROUP_COUNT * LOCAL_SIZE_X)

static uint8_t *read_file(const char *path, size_t *out_len) {
    FILE *file = fopen(path, "rb");
    if (!file) {
        fprintf(stderr, "failed to open %s\n", path);
        return NULL;
    }
    fseek(file, 0, SEEK_END);
    long size = ftell(file);
    fseek(file, 0, SEEK_SET);

    uint8_t *data = malloc((size_t)size);
    if (fread(data, 1, (size_t)size, file) != (size_t)size) {
        fprintf(stderr, "failed to read %s\n", path);
        fclose(file);
        free(data);
        return NULL;
    }
    fclose(file);

    *out_len = (size_t)size;
    return data;
}

int main(void) {
    size_t spirv_len = 0;
    uint8_t *spirv = read_file(WRITE_PATTERN_SPV_PATH, &spirv_len);
    if (!spirv) {
        return 1;
    }

    uint32_t out_buffer[ELEMENT_COUNT];
    enum RendererCapiStatus status = renderer_capi_run_compute_sample(
        spirv, spirv_len,
        WORKGROUP_COUNT, 1, 1,
        (uint8_t *)out_buffer, sizeof(out_buffer));
    free(spirv);

    if (status != RENDERER_CAPI_STATUS_OK) {
        fprintf(stderr, "compute dispatch failed (status %d)\n", status);
        return 1;
    }

    for (uint32_t i = 0; i < ELEMENT_COUNT; i++) {
        uint32_t expected = i * 2 + 1;
        if (out_buffer[i] != expected) {
            fprintf(stderr, "mismatch at index %u: expected %u, got %u\n",
                    i, expected, out_buffer[i]);
            return 1;
        }
    }

    printf("compute dispatch succeeded, all %d values matched the expected pattern\n",
           ELEMENT_COUNT);
    return 0;
}

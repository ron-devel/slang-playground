/* Manual smoke test against a real local adb server, using the C ABI
 * instead of the Rust API (see ../track_devices.rs for the Rust
 * equivalent and full instructions). Not run in CI — this needs a
 * machine with adb installed and a device already visible to
 * `adb devices`.
 *
 * Build with CMake (from the `bridge` directory; add
 * -DBRIDGE_ADB_RUST_PROFILE=release to match a --release cargo build):
 *   cargo build -p bridge-adb
 *   cmake -S . -B build && cmake --build build
 *   ./build/adb/bridge_adb_track_devices
 *
 * ...or build it directly, linking the static library (needs a few
 * extra system libs CMake's shared-library linking avoids needing):
 *   cargo build --release -p bridge-adb
 *   cc -o track_devices examples/c/track_devices.c \
 *       -I adb/include -L target/release -l bridge_adb -l pthread -l dl -l m
 *   LD_LIBRARY_PATH=target/release ./track_devices
 */

#include <stdio.h>
#include "bridge_adb.h"

int main(void) {
    struct BridgeAdbClient *client = NULL;
    /* 5000ms timeout so this fails fast if no adb server is running,
     * instead of hanging indefinitely. Pass -1 to wait forever. */
    enum BridgeAdbStatus status = bridge_adb_connect("127.0.0.1", 5037, 5000, &client);
    if (status == BRIDGE_ADB_STATUS_ERROR_TIMED_OUT) {
        fprintf(stderr, "timed out connecting to adb server\n");
        return 1;
    }
    if (status != BRIDGE_ADB_STATUS_OK) {
        fprintf(stderr, "failed to connect to adb server (status %d)\n", status);
        return 1;
    }

    printf("Connected to adb server, watching for device list changes (Ctrl+C to quit)...\n");

    struct BridgeAdbDeviceList list;
    while (bridge_adb_next_snapshot(client, &list)) {
        printf("--- device list update ---\n");
        if (list.count == 0) {
            printf("  (no devices connected)\n");
        }
        for (size_t i = 0; i < list.count; i++) {
            printf("  %s\t%s\n", list.devices[i].serial, list.devices[i].state);
        }
        bridge_adb_free_device_list(list);
    }

    printf("adb server closed the connection\n");
    bridge_adb_disconnect(client);
    return 0;
}

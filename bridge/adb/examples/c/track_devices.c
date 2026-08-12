/* Manual smoke test against a real local adb server, using the C ABI
 * instead of the Rust API (see ../track_devices.rs for the Rust
 * equivalent and full instructions). Not run in CI — this needs a
 * machine with adb installed and a device already visible to
 * `adb devices`.
 *
 * Build (from the `bridge` directory):
 *   cargo build --release -p bridge-adb
 *   cc -o track_devices examples/c/track_devices.c \
 *       -I adb/include -L target/release -l bridge_adb -l pthread -l dl -l m
 *
 * Run:
 *   LD_LIBRARY_PATH=target/release ./track_devices
 */

#include <stdio.h>
#include "bridge_adb.h"

int main(void) {
    struct BridgeAdbClient *client = bridge_adb_connect("127.0.0.1", 5037);
    if (!client) {
        fprintf(stderr, "failed to connect to adb server\n");
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

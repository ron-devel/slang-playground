//! C ABI for using `TrackDevicesClient` from non-Rust hosts (e.g. an SDL3
//! desktop app). Every call here is synchronous/blocking — each handle
//! owns a small dedicated Tokio runtime that drives the async client
//! underneath, so callers never need to know this is built on Tokio.
//!
//! Ownership: `bridge_adb_connect` returns a handle owned by the caller,
//! to be freed exactly once with `bridge_adb_disconnect`. Each successful
//! `bridge_adb_next_snapshot` call allocates a `BridgeAdbDeviceList` owned
//! by the caller, to be freed exactly once with
//! `bridge_adb_free_device_list` before the next call (or after the last
//! one, before disconnecting).

use crate::{Device, TrackDevicesClient};
use std::ffi::{c_char, CStr, CString};
use std::net::SocketAddr;
use std::ptr;

/// Opaque handle to a connected adb `track-devices` client.
pub struct BridgeAdbClient {
    runtime: tokio::runtime::Runtime,
    inner: TrackDevicesClient,
}

#[repr(C)]
pub struct BridgeAdbDevice {
    /// Null-terminated. Owned by the enclosing `BridgeAdbDeviceList`; do
    /// not free individually.
    pub serial: *mut c_char,
    /// Null-terminated. Owned by the enclosing `BridgeAdbDeviceList`; do
    /// not free individually.
    pub state: *mut c_char,
}

#[repr(C)]
pub struct BridgeAdbDeviceList {
    pub devices: *mut BridgeAdbDevice,
    pub count: usize,
}

/// Connects to an adb server at `host:port` (typically `"127.0.0.1"`,
/// `5037`) and starts tracking devices. Returns NULL on failure (invalid
/// host string, or the connection/handshake failed).
///
/// # Safety
/// `host` must be a valid, null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn bridge_adb_connect(
    host: *const c_char,
    port: u16,
) -> *mut BridgeAdbClient {
    if host.is_null() {
        return ptr::null_mut();
    }
    let Ok(host) = CStr::from_ptr(host).to_str() else {
        return ptr::null_mut();
    };
    let Ok(ip) = host.parse() else {
        return ptr::null_mut();
    };
    let addr = SocketAddr::new(ip, port);

    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ptr::null_mut();
    };

    match runtime.block_on(TrackDevicesClient::connect(addr)) {
        Ok(inner) => Box::into_raw(Box::new(BridgeAdbClient { runtime, inner })),
        Err(_) => ptr::null_mut(),
    }
}

/// Blocks until the next device-list snapshot arrives and writes it to
/// `out_list`. Returns `true` on success, `false` if the connection
/// closed or an error occurred (in which case `out_list` is left
/// untouched).
///
/// # Safety
/// `client` must be a valid pointer returned by `bridge_adb_connect` and
/// not yet passed to `bridge_adb_disconnect`. `out_list` must be a valid,
/// writable pointer to a `BridgeAdbDeviceList`.
#[no_mangle]
pub unsafe extern "C" fn bridge_adb_next_snapshot(
    client: *mut BridgeAdbClient,
    out_list: *mut BridgeAdbDeviceList,
) -> bool {
    if client.is_null() || out_list.is_null() {
        return false;
    }
    let client = &mut *client;

    let Ok(Some(devices)) = client.runtime.block_on(client.inner.next_snapshot()) else {
        return false;
    };

    *out_list = device_list_to_c(devices);
    true
}

/// Frees a device list previously written by `bridge_adb_next_snapshot`.
///
/// # Safety
/// `list` must be a list previously written by `bridge_adb_next_snapshot`,
/// and must not be freed more than once.
#[no_mangle]
pub unsafe extern "C" fn bridge_adb_free_device_list(list: BridgeAdbDeviceList) {
    if list.devices.is_null() {
        return;
    }
    let devices = Vec::from_raw_parts(list.devices, list.count, list.count);
    for device in devices {
        drop(CString::from_raw(device.serial));
        drop(CString::from_raw(device.state));
    }
}

/// Disconnects and frees a client handle. `client` must not be used
/// afterward.
///
/// # Safety
/// `client` must be a valid pointer returned by `bridge_adb_connect`, not
/// already passed to this function.
#[no_mangle]
pub unsafe extern "C" fn bridge_adb_disconnect(client: *mut BridgeAdbClient) {
    if client.is_null() {
        return;
    }
    drop(Box::from_raw(client));
}

fn device_list_to_c(devices: Vec<Device>) -> BridgeAdbDeviceList {
    let empty_cstring = || CString::new(Vec::new()).unwrap();
    let mut c_devices: Vec<BridgeAdbDevice> = devices
        .into_iter()
        .map(|device| BridgeAdbDevice {
            serial: CString::new(device.serial)
                .unwrap_or_else(|_| empty_cstring())
                .into_raw(),
            state: CString::new(device.state)
                .unwrap_or_else(|_| empty_cstring())
                .into_raw(),
        })
        .collect();
    // Ensures capacity == len, so bridge_adb_free_device_list can
    // reconstruct the Vec with Vec::from_raw_parts(ptr, count, count).
    c_devices.shrink_to_fit();
    let count = c_devices.len();
    let ptr = c_devices.as_mut_ptr();
    std::mem::forget(c_devices);
    BridgeAdbDeviceList {
        devices: ptr,
        count,
    }
}

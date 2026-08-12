use bridge_adb::ffi::{
    bridge_adb_connect, bridge_adb_disconnect, bridge_adb_free_device_list,
    bridge_adb_next_snapshot, BridgeAdbDeviceList,
};
use std::ffi::{CStr, CString};
use std::io::{Read, Write};
use std::net::TcpListener;

fn write_frame(stream: &mut impl Write, payload: &str) {
    let header = format!("{:04x}", payload.len());
    stream.write_all(header.as_bytes()).unwrap();
    stream.write_all(payload.as_bytes()).unwrap();
}

/// Calls the exact same #[no_mangle] extern "C" functions a C caller
/// would link against, against a fake adb server on a plain OS thread —
/// this is the FFI layer's memory-safety/logic test; the checked-in C
/// example is what actually proves real C linkage against the generated
/// header.
#[test]
fn ffi_round_trips_a_device_snapshot() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();

        let mut length_hex = [0u8; 4];
        socket.read_exact(&mut length_hex).unwrap();
        let length = u32::from_str_radix(std::str::from_utf8(&length_hex).unwrap(), 16).unwrap();
        let mut service = vec![0u8; length as usize];
        socket.read_exact(&mut service).unwrap();
        assert_eq!(service, b"host:track-devices");

        socket.write_all(b"OKAY").unwrap();
        write_frame(&mut socket, "emulator-5554\tdevice\n");

        // Keep the connection open past the assertions below.
        std::thread::sleep(std::time::Duration::from_secs(30));
    });

    let host = CString::new("127.0.0.1").unwrap();
    let client = unsafe { bridge_adb_connect(host.as_ptr(), addr.port()) };
    assert!(!client.is_null(), "bridge_adb_connect should succeed");

    let mut list = BridgeAdbDeviceList {
        devices: std::ptr::null_mut(),
        count: 0,
    };
    let ok = unsafe { bridge_adb_next_snapshot(client, &mut list) };
    assert!(ok, "bridge_adb_next_snapshot should succeed");
    assert_eq!(list.count, 1);

    unsafe {
        let device = &*list.devices;
        assert_eq!(
            CStr::from_ptr(device.serial).to_str().unwrap(),
            "emulator-5554"
        );
        assert_eq!(CStr::from_ptr(device.state).to_str().unwrap(), "device");

        bridge_adb_free_device_list(list);
        bridge_adb_disconnect(client);
    }
}

#[test]
fn connect_rejects_null_and_invalid_host() {
    unsafe {
        assert!(bridge_adb_connect(std::ptr::null(), 5037).is_null());

        let invalid_host = CString::new("not an ip address").unwrap();
        assert!(bridge_adb_connect(invalid_host.as_ptr(), 5037).is_null());
    }
}

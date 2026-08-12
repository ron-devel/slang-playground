# Imports the already-built bridge-adb Rust library rather than driving
# the Cargo build itself. Before configuring, from the `bridge` directory:
#   cargo build [--release] -p bridge-adb

set(BRIDGE_ADB_RUST_PROFILE "debug" CACHE STRING "Cargo profile to link against (debug or release)")
set(BRIDGE_ADB_TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../target/${BRIDGE_ADB_RUST_PROFILE}")
set(BRIDGE_ADB_LIBRARY "${BRIDGE_ADB_TARGET_DIR}/libbridge_adb${CMAKE_SHARED_LIBRARY_SUFFIX}")

if(NOT EXISTS "${BRIDGE_ADB_LIBRARY}")
    if(BRIDGE_ADB_RUST_PROFILE STREQUAL "release")
        set(_cargo_build_cmd "cargo build --release -p bridge-adb")
    else()
        set(_cargo_build_cmd "cargo build -p bridge-adb")
    endif()
    message(WARNING
        "bridge-adb has not been built for the '${BRIDGE_ADB_RUST_PROFILE}' "
        "profile yet (expected ${BRIDGE_ADB_LIBRARY}). Run "
        "'${_cargo_build_cmd}' from the bridge/ directory first."
    )
endif()

# SHARED rather than STATIC: a shared library resolves Rust's own runtime
# dependencies (pthread, etc.) internally, so consumers just link
# bridge_adb with no platform-specific extra libs to guess at — and it's
# the natural fit for how this will actually be loaded on Android (JNI)
# and likely by the desktop app too.
add_library(bridge_adb SHARED IMPORTED GLOBAL)
set_target_properties(bridge_adb PROPERTIES
    IMPORTED_LOCATION "${BRIDGE_ADB_LIBRARY}"
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_SOURCE_DIR}/include"
)

if(BUILD_SAMPLES)
    add_executable(bridge_adb_track_devices examples/c/track_devices.c)
    target_link_libraries(bridge_adb_track_devices PRIVATE bridge_adb)
    # So the built executable finds libbridge_adb without needing
    # LD_LIBRARY_PATH set manually.
    set_target_properties(bridge_adb_track_devices PROPERTIES
        BUILD_RPATH "${BRIDGE_ADB_TARGET_DIR}"
    )
endif()

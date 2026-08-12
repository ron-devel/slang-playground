fn main() {
    let proto_file = "proto/bridge/v1.proto";
    println!("cargo:rerun-if-changed={proto_file}");

    let file_descriptor_set = protox::compile([proto_file], ["proto"])
        .expect("failed to compile bridge.proto with protox");

    prost_build::Config::new()
        .compile_fds(file_descriptor_set)
        .expect("failed to generate Rust types from bridge.proto");
}

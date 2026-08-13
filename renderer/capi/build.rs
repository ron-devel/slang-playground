fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let output_path = std::path::Path::new(&crate_dir)
        .join("include")
        .join("renderer_capi.h");

    std::fs::create_dir_all(output_path.parent().unwrap())
        .expect("failed to create include/ directory");

    let config = cbindgen::Config::from_root_or_default(&crate_dir);
    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
        .expect("failed to generate renderer_capi.h")
        .write_to_file(&output_path);
}

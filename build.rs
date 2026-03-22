fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let config_path = std::path::Path::new(&crate_dir).join("cbindgen.toml");
    let out_path = std::path::Path::new(&crate_dir).join("include/o3jc.h");

    let config = cbindgen::Config::from_file(&config_path)
        .expect("cbindgen.toml not found or invalid");

    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
        .expect("unable to generate C bindings")
        .write_to_file(out_path);

    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}

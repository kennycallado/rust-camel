fn main() -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }

    tonic_build::configure()
        .build_server(false)
        .compile_protos(&["proto/xml_bridge.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/xml_bridge.proto");
    Ok(())
}

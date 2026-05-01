fn main() -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }

    tonic_prost_build::configure()
        .build_server(false)
        .compile_protos(&["proto/jms_bridge.proto"], &["proto"])?;
    Ok(())
}

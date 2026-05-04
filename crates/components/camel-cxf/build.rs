fn main() -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }

    tonic_prost_build::configure().compile_protos(&["proto/cxf_bridge.proto"], &["proto"])?;
    Ok(())
}

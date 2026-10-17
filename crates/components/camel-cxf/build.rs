fn main() -> Result<(), Box<dyn std::error::Error>> {
    let descriptors = protox::compile(["proto/cxf_bridge.proto"], ["proto"])?;
    tonic_prost_build::configure().compile_fds(descriptors)?;
    println!("cargo:rerun-if-changed=proto/cxf_bridge.proto");
    println!("cargo:rerun-if-changed=proto");
    Ok(())
}

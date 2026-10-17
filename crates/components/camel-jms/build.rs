fn main() -> Result<(), Box<dyn std::error::Error>> {
    let descriptors = protox::compile(["proto/jms_bridge.proto"], ["proto"])?;
    tonic_prost_build::configure().compile_fds(descriptors)?;
    println!("cargo:rerun-if-changed=proto/jms_bridge.proto");
    println!("cargo:rerun-if-changed=proto");
    Ok(())
}

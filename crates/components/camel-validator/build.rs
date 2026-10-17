fn main() -> Result<(), Box<dyn std::error::Error>> {
    let descriptors = protox::compile(["proto/xml_bridge.proto"], ["proto"])?;
    tonic_prost_build::configure().compile_fds(descriptors)?;

    println!("cargo:rerun-if-changed=proto/xml_bridge.proto");
    println!("cargo:rerun-if-changed=proto");
    Ok(())
}

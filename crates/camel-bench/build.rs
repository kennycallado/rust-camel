//! Proto codegen for the XML bridge decomposition bench: compiles the
//! canonical `bridges/xml/src/main/proto/xml_bridge.proto` via
//! `tonic-prost-build` + `protox` (same pattern as
//! camel-xslt's build.rs, but referencing the canonical proto — no
//! third vendored copy).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = "../../bridges/xml/src/main/proto";
    let proto = format!("{proto_dir}/xml_bridge.proto");
    let descriptors = protox::compile([proto.as_str()], [proto_dir])?;
    tonic_prost_build::configure().compile_fds(descriptors)?;
    println!("cargo:rerun-if-changed={proto}");
    println!("cargo:rerun-if-changed={proto_dir}");
    Ok(())
}

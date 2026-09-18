//! Proto codegen for the XML bridge decomposition bench: compiles the
//! canonical `bridges/xml/src/main/proto/xml_bridge.proto` via
//! `tonic-prost-build` + `protoc-bin-vendored` (same pattern as
//! camel-xslt's build.rs, but referencing the canonical proto — no
//! third vendored copy).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = "../../bridges/xml/src/main/proto";
    let proto = format!("{proto_dir}/xml_bridge.proto");
    unsafe {
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }
    tonic_prost_build::configure().compile_protos(&[proto.as_str()], &[proto_dir])?;
    println!("cargo:rerun-if-changed={proto}");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read CARGO_MANIFEST_DIR at RUNTIME, not via env!() at build-script
    // compile time. With a shared target dir, the compiled build-script
    // binary is reused across worktrees of the same workspace version; a
    // compile-time bake would carry the FIRST worktree's absolute path and
    // break code generation when that worktree is later deleted. Cargo sets
    // this var fresh for every invocation, pointing at the package being built now.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let proto_dir = std::path::PathBuf::from(manifest_dir).join("tests");
    let proto_helloworld = proto_dir.join("helloworld.proto");
    let proto_streaming = proto_dir.join("streaming.proto");
    let descriptors = protox::compile([&proto_helloworld, &proto_streaming], [&proto_dir])?;
    tonic_prost_build::configure().compile_fds(descriptors)?;
    println!("cargo:rerun-if-changed={}", proto_helloworld.display());
    println!("cargo:rerun-if-changed={}", proto_streaming.display());
    println!("cargo:rerun-if-changed={}", proto_dir.display());
    Ok(())
}

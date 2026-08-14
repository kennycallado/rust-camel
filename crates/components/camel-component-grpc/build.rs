fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }
    // Read CARGO_MANIFEST_DIR at RUNTIME, not via env!() at build-script
    // compile time. With a shared target dir, the compiled build-script
    // binary is reused across worktrees of the same workspace version; a
    // compile-time bake would carry the FIRST worktree's absolute path and
    // break protoc when that worktree is later deleted. Cargo sets this var
    // fresh for every invocation, pointing at the package being built now.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("cargo sets CARGO_MANIFEST_DIR when running build scripts");
    let proto_dir = std::path::PathBuf::from(manifest_dir).join("tests");
    let proto_helloworld = proto_dir.join("helloworld.proto");
    let proto_streaming = proto_dir.join("streaming.proto");
    tonic_prost_build::configure()
        .compile_protos(&[proto_helloworld, proto_streaming], &[proto_dir])
        .unwrap();
}

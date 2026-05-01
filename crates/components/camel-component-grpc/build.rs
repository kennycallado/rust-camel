fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }
    let proto = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
    let proto_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    tonic_prost_build::configure()
        .compile_protos(&[proto], &[proto_dir])
        .unwrap();
}

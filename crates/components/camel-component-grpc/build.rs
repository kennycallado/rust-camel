fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }
    let proto_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let proto_helloworld = proto_dir.join("helloworld.proto");
    let proto_streaming = proto_dir.join("streaming.proto");
    tonic_prost_build::configure()
        .compile_protos(&[proto_helloworld, proto_streaming], &[proto_dir])
        .unwrap();
}

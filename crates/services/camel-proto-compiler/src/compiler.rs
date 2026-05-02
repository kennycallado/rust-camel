use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use prost_reflect::DescriptorPool;
use tracing::debug;

use crate::ProtoCompileError;

static COMPILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn compile_proto<P, I>(proto_path: P, includes: I) -> Result<DescriptorPool, ProtoCompileError>
where
    P: AsRef<Path>,
    I: IntoIterator,
    I::Item: AsRef<Path>,
{
    let proto_path = proto_path.as_ref();
    if !proto_path.exists() {
        return Err(ProtoCompileError::ProtoNotFound(proto_path.to_path_buf()));
    }

    let include_paths = includes
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .collect::<Vec<PathBuf>>();

    let protoc = std::env::var_os("PROTOC")
        .map(PathBuf::from)
        .unwrap_or(protoc_bin_vendored::protoc_bin_path()?);

    let descriptor_file = std::env::temp_dir().join(format!(
        "camel-proto-{}.desc",
        COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));

    let mut cmd = Command::new(&protoc);
    cmd.arg(format!(
        "--descriptor_set_out={}",
        descriptor_file.display()
    ))
    .arg("--include_imports")
    .arg(proto_path);

    for include in &include_paths {
        cmd.arg("-I").arg(include);
    }

    if let Some(parent) = proto_path.parent() {
        cmd.arg("-I").arg(parent);
    }

    debug!(protoc = %protoc.display(), proto = %proto_path.display(), "compiling proto");
    let output = cmd.output()?;

    if !output.status.success() {
        return Err(ProtoCompileError::ProtocFailed {
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    let bytes = std::fs::read(&descriptor_file)?;
    let _ = std::fs::remove_file(&descriptor_file);

    DescriptorPool::decode(bytes.as_slice())
        .map_err(|e| ProtoCompileError::DescriptorDecode(e.to_string()))
}

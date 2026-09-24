use std::path::{Path, PathBuf};
use std::process::Command;

use prost_reflect::DescriptorPool;
use tracing::debug;

use crate::ProtoCompileError;

/// Resolves the protoc binary, preferring the `PROTOC` environment
/// variable over the vendored resolver.
pub(crate) fn resolve_protoc() -> Result<PathBuf, ProtoCompileError> {
    resolve_protoc_with(|| {
        protoc_bin_vendored::protoc_bin_path().map_err(|e| ProtoCompileError::ProtocUnavailable {
            detail: e.to_string(),
        })
    })
}

/// Injection seam for [`resolve_protoc`]. Reads `PROTOC` first and
/// returns it without calling `vendored`. Otherwise runs `vendored`
/// inside `catch_unwind`: its `Ok` value and its `Err` pass through
/// verbatim; a panic is contained and mapped to
/// [`ProtoCompileError::ProtocUnavailable`] using the panic message.
pub(crate) fn resolve_protoc_with(
    vendored: impl FnOnce() -> Result<PathBuf, ProtoCompileError>,
) -> Result<PathBuf, ProtoCompileError> {
    if let Some(value) = std::env::var_os("PROTOC") {
        return Ok(PathBuf::from(value));
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(vendored)) {
        Ok(Ok(path)) => Ok(path),
        Ok(Err(e)) => Err(e),
        Err(payload) => Err(ProtoCompileError::ProtocUnavailable {
            detail: panic_payload_message(payload),
        }),
    }
}

/// Extracts a human-readable message from a caught panic payload.
fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "unknown panic payload".to_string()
    }
}

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

    let protoc = resolve_protoc()?;

    let temp_file = tempfile::Builder::new().suffix(".desc").tempfile()?;
    let descriptor_file = temp_file.path().to_path_buf();

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
    // temp_file is dropped here, auto-cleaning the descriptor file

    DescriptorPool::decode(bytes.as_slice())
        .map_err(|e| ProtoCompileError::DescriptorDecode(format!("{}: {e}", proto_path.display())))
}

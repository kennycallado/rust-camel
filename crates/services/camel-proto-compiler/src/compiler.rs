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

/// Stored panic hook signature, matching `std::panic::take_hook()`.
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

/// Serializes `SilencePanicHook` lifetimes so concurrent guards cannot
/// interleave take/set/restore and permanently leak the no-op hook.
static HOOK_SWAP: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Silences the process panic hook while alive and restores the
/// previous hook on drop.
///
/// Held only across the `catch_unwind` window of [`resolve_protoc_with`],
/// so a contained panic cannot double-report as `thread panicked` stderr
/// noise before the typed [`ProtoCompileError::ProtocUnavailable`]
/// surfaces. Concurrent guard lifetimes are serialized by [`HOOK_SWAP`],
/// so their take/set/restore swaps cannot interleave and the silent hook
/// can never be captured or restored by a second guard.
struct SilencePanicHook {
    // Held for the guard's whole lifetime; Drop::drop restores the
    // previous hook BEFORE this guard releases, so no other guard can
    // observe or capture the silent hook.
    _swap_lock: Option<std::sync::MutexGuard<'static, ()>>,
    previous: Option<PanicHook>,
}

impl SilencePanicHook {
    fn new() -> Self {
        let swap_lock = HOOK_SWAP
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        Self {
            _swap_lock: Some(swap_lock),
            previous: Some(previous),
        }
    }
}

impl Drop for SilencePanicHook {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::panic::set_hook(previous);
        }
    }
}

/// Injection seam for [`resolve_protoc`]. Reads `PROTOC` first and
/// returns it without calling `vendored`. Otherwise runs `vendored`
/// inside `catch_unwind`: its `Ok` value and its `Err` pass through
/// verbatim; a panic is contained and mapped to
/// [`ProtoCompileError::ProtocUnavailable`] using the panic message.
///
/// The `catch_unwind` window is hook-silent: a [`SilencePanicHook`] guard
/// is installed after the `PROTOC` early return and dropped when the
/// `match` completes, so the contained panic cannot double-report as
/// `thread panicked` stderr noise while the typed error still carries the
/// original payload message. Overlapping calls cannot interleave their
/// hook swaps; [`HOOK_SWAP`] serializes the guards' whole lifetimes.
pub(crate) fn resolve_protoc_with(
    vendored: impl FnOnce() -> Result<PathBuf, ProtoCompileError>,
) -> Result<PathBuf, ProtoCompileError> {
    if let Some(value) = std::env::var_os("PROTOC") {
        return Ok(PathBuf::from(value));
    }

    let _hook = SilencePanicHook::new();
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

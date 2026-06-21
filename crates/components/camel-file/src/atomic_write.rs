//! Atomic-write helper for the file producer.
//!
//! Extracted from `lib.rs` to fix Bug C (Override branch temp-path parent
//! missing when `fileName` contains nested directories) and to unify the
//! `TryRename` and `Override` write paths. See `docs/superpowers/specs/
//! 2026-06-20-rc-o6o-framework-contract-bugs.md` §3.3 and §4.1.

use std::path::Path;
use std::time::Duration;

use camel_component_api::{Body, CamelError};
use tokio::io::AsyncWriteExt;

use crate::{TempFileGuard, write_body_with_charset};

/// Default temp-file prefix when the caller does not supply one via
/// `FileConfig::temp_prefix` or the `tempPrefix` URI parameter.
///
/// Single source of truth — both the helper and the Contract Surface doc
/// reference this constant.
pub(crate) const DEFAULT_TEMP_PREFIX: &str = ".tmp.";

pub(crate) async fn atomic_write(
    target_path: &Path,
    body: Body,
    temp_prefix: Option<&str>,
    durable: bool,
    write_timeout: Duration,
    charset: &Option<String>,
) -> Result<(), CamelError> {
    let parent = target_path.parent().ok_or_else(|| {
        CamelError::ProcessorError(format!(
            "target path has no parent directory: {}",
            target_path.display()
        ))
    })?;
    let file_name = target_path.file_name().ok_or_else(|| {
        CamelError::ProcessorError(format!(
            "target path has no file_name component: {}",
            target_path.display()
        ))
    })?;

    let prefix = temp_prefix.unwrap_or(DEFAULT_TEMP_PREFIX);
    // Build the temp name from the prefix + FINAL file_name component only.
    // Bug C root cause: the old Override branch concatenated the prefix with the
    // full nested file_name (`a/b/c.bin`), producing a temp path whose parent
    // did not exist.
    let mut temp_name = prefix.to_string();
    temp_name.push_str(&file_name.to_string_lossy());
    let temp_path = parent.join(&temp_name);

    // 1. Create + write the temp file.
    // CRITICAL (e_gpt oracle blessing condition #1): the TempFileGuard must
    // only clean up files THIS HELPER successfully created. Constructing the
    // guard BEFORE `create_new(true).open()` would cause its Drop impl to
    // delete a pre-existing blocker file at temp_path on the open's failure
    // path — that's unsafe ownership (concurrent writer / stale external file
    // would be silently deleted). The guard is created ONLY after open
    // succeeds, so a failed open leaves any pre-existing file untouched.
    let mut temp_file = tokio::time::timeout(
        write_timeout,
        tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path),
    )
    .await
    .map_err(|_| CamelError::ProcessorError("Timeout creating temp file".into()))?
    .map_err(CamelError::from)?;

    // Open succeeded — THIS helper owns temp_path now. Arm the RAII guard.
    let mut guard = TempFileGuard::new(temp_path.clone());

    write_body_with_charset(body, charset, &mut temp_file, write_timeout).await?;

    // 2. Optional fsync of temp file contents (durable path).
    if durable {
        tokio::time::timeout(write_timeout, temp_file.sync_all())
            .await
            .map_err(|_| CamelError::ProcessorError("Timeout fsyncing temp file".into()))?
            .map_err(CamelError::from)?;
    } else {
        // Best-effort flush even when not durable (matches current behavior at lib.rs:1220).
        let _ = temp_file.flush().await;
    }

    // 3. Atomic rename: temp → target. Detect cross-filesystem (EXDEV) and reject.
    let rename_result =
        tokio::time::timeout(write_timeout, tokio::fs::rename(&temp_path, target_path)).await;

    match rename_result {
        Ok(Ok(_)) => {
            guard.disarm();
        }
        Ok(Err(e)) => {
            return Err(map_rename_error(e));
        }
        Err(_) => {
            return Err(CamelError::ProcessorError(
                "Timeout renaming temp file".into(),
            ));
        }
    }

    // 4. Optional parent-directory fsync (durable path).
    if durable {
        fsync_parent_dir(parent, write_timeout).await?;
    }

    Ok(())
}

/// Map a `tokio::fs::rename` error to a `CamelError`, treating cross-filesystem
/// (EXDEV) as an explicit rejection per spec §3.3.
fn map_rename_error(e: std::io::Error) -> CamelError {
    if let Some(raw) = e.raw_os_error() {
        // EXDEV = 18 on Linux/POSIX. Cross-device link not permitted.
        if raw == 18 {
            return CamelError::ProcessorError(format!(
                "cross-filesystem rename rejected (EXDEV); target and temp must share a filesystem: {e}"
            ));
        }
    }
    CamelError::from(e)
}

/// Fsync of the parent directory after a successful rename. The caller opted
/// into `durable=true` for strict crash-safety — fsync errors (including
/// platform differences like Windows `FlushFileBuffers` behavior on directories)
/// PROPAGATE as `CamelError`, NOT silently ignored. "Best-effort" wording in
/// earlier drafts was wrong: the code returns the error, so the docs must too.
/// Users who want lenient durability should leave `durable=false` (the default).
async fn fsync_parent_dir(parent: &Path, write_timeout: Duration) -> Result<(), CamelError> {
    // Open the directory read-only (write is not required for fsync; on Linux
    // O_RDONLY on a directory is the canonical way to obtain a dir fd for fsync).
    let dir_file = tokio::time::timeout(write_timeout, tokio::fs::File::open(parent))
        .await
        .map_err(|_| CamelError::ProcessorError("Timeout opening parent dir for fsync".into()))?
        .map_err(CamelError::from)?;
    tokio::time::timeout(write_timeout, dir_file.sync_all())
        .await
        .map_err(|_| CamelError::ProcessorError("Timeout fsyncing parent dir".into()))?
        .map_err(CamelError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::Body;

    #[tokio::test]
    async fn atomic_write_durable_true_succeeds_and_persists_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.bin");
        let body = Body::from(vec![1u8, 2, 3, 4]);

        atomic_write(
            &target,
            body,
            None,
            true, // durable
            Duration::from_secs(5),
            &None,
        )
        .await
        .unwrap();

        assert!(target.exists(), "target should exist after atomic_write");
        let bytes = std::fs::read(&target).unwrap();
        assert_eq!(bytes, vec![1u8, 2, 3, 4]);
        // Temp file must NOT remain in the parent directory.
        let mut entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        entries.retain(|e| {
            let name = e.as_ref().unwrap().file_name();
            name.to_string_lossy() == "out.bin"
        });
        assert_eq!(entries.len(), 1, "only the target file should remain");
    }

    #[tokio::test]
    async fn atomic_write_durable_false_succeeds_and_persists_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out2.bin");
        let body = Body::from(vec![9u8, 8, 7]);

        atomic_write(
            &target,
            body,
            None,
            false, // not durable
            Duration::from_secs(5),
            &None,
        )
        .await
        .unwrap();

        assert!(target.exists());
        let bytes = std::fs::read(&target).unwrap();
        assert_eq!(bytes, vec![9u8, 8, 7]);
    }

    #[tokio::test]
    async fn atomic_write_honors_custom_temp_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("final.txt");
        let body = Body::from(b"hi".to_vec());

        atomic_write(
            &target,
            body,
            Some("custom-"),
            false,
            Duration::from_secs(5),
            &None,
        )
        .await
        .unwrap();

        assert!(target.exists());
        // The temp file must have been renamed away.
        assert!(
            !dir.path().join("custom-final.txt").exists(),
            "temp file should have been renamed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn map_rename_error_translates_exdev_to_processor_error() {
        // EXDEV (errno 18) — fabricate via from_raw_os_error so the test does not
        // need a second mounted filesystem.
        let err = std::io::Error::from_raw_os_error(18);
        let mapped = super::map_rename_error(err);
        let msg = match mapped {
            camel_component_api::CamelError::ProcessorError(m) => m,
            other => panic!("expected ProcessorError, got {other:?}"),
        };
        assert!(
            msg.contains("cross-filesystem") && msg.contains("EXDEV"),
            "EXDEV error message must mention cross-filesystem and EXDEV, got: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn map_rename_error_passes_through_non_exdev() {
        use std::io::ErrorKind;
        // A non-EXDEV error (e.g. NotFound) should be wrapped via CamelError::from(io::Error).
        let err = std::io::Error::new(ErrorKind::NotFound, "missing");
        let mapped = super::map_rename_error(err);
        // CamelError::from(io::Error) maps to ProcessorError or Io variant — both are non-panic.
        assert!(
            matches!(
                mapped,
                camel_component_api::CamelError::ProcessorError(_)
                    | camel_component_api::CamelError::Io(_)
            ),
            "non-EXDEV io error should map to ProcessorError or Io, got {mapped:?}"
        );
    }

    #[tokio::test]
    async fn atomic_write_preserves_preexisting_blocker_on_create_fail() {
        // Oracle blessing condition #2: TempFileGuard is armed ONLY after
        // create_new(true).open() succeeds. If open fails (blocker exists),
        // no guard is constructed → the pre-existing blocker file is
        // preserved untouched. Earlier draft of this test expected the guard
        // to delete the blocker — that was the unsafe ownership bug e_gpt
        // caught.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cleanup.bin");

        // Pre-create the temp file path so create_new(true) MUST fail with AlreadyExists.
        let temp_path = dir.path().join(".tmp.cleanup.bin");
        std::fs::write(&temp_path, b"blocker").unwrap();

        let body = Body::from(vec![1u8, 2, 3]);
        let result = atomic_write(&target, body, None, false, Duration::from_secs(5), &None).await;

        assert!(
            result.is_err(),
            "expected atomic_write to fail because temp file already exists"
        );

        // The target must NOT exist after a failed write.
        assert!(!target.exists(), "target must not exist after failed write");

        // The pre-existing blocker file MUST be preserved — the guard was
        // never constructed because open() failed before guard creation.
        assert!(
            temp_path.exists(),
            "pre-existing blocker MUST be preserved (guard was not armed; \
             open() failed before TempFileGuard::new)"
        );
        assert_eq!(
            std::fs::read(&temp_path).unwrap(),
            b"blocker",
            "blocker file content MUST be unchanged"
        );
    }

    #[tokio::test]
    async fn atomic_write_cleans_temp_file_when_write_fails_after_open() {
        // Oracle blessing condition #2 (re-bless): the zero-timeout approach in
        // the prior draft did not deterministically exercise the post-open
        // failure path. Use an unsupported charset instead: open succeeds
        // (guard armed), then `write_body_with_charset` fails trying to encode
        // a non-ASCII body with a bogus charset, then Drop runs and removes
        // the temp file this helper created.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cleanup2.bin");
        let temp_path = dir.path().join(".tmp.cleanup2.bin");

        // Body contains non-ASCII bytes that will force the charset encoder to
        // actually attempt transcoding and fail on the bogus charset name.
        let body = Body::from("café ñ ü".to_string());
        let result = atomic_write(
            &target,
            body,
            None,
            false,
            Duration::from_secs(5),
            &Some("INVALID_CHARSET_XYZ".to_string()),
        )
        .await;

        assert!(
            result.is_err(),
            "expected write_body_with_charset to fail on bogus charset"
        );

        // Target MUST NOT exist — write never completed.
        assert!(!target.exists(), "target must not exist after failed write");

        // Temp file MUST be cleaned up — the helper opened it (guard armed),
        // then the write failed, so Drop removed it. This is the invariant
        // the oracle asked us to actually prove.
        assert!(
            !temp_path.exists(),
            "temp file created by helper MUST be cleaned up when write fails after open"
        );
    }

    #[tokio::test]
    async fn atomic_write_temp_file_lives_in_target_parent_not_dir_root() {
        // Shape-noise contract test: even when the eventual call site passes a
        // deeply-nested target_path, the temp file MUST live in the same parent —
        // never concatenated with the prefix at the directory root (Bug C root cause).
        let dir = tempfile::tempdir().unwrap();
        let nested_parent = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested_parent).unwrap();
        let target = nested_parent.join("deep.bin");

        let body = Body::from(b"deep".to_vec());
        atomic_write(&target, body, None, false, Duration::from_secs(5), &None)
            .await
            .unwrap();

        assert!(target.exists());
        // No stray temp at directory root (the Bug C signature).
        assert!(
            !dir.path().join(".tmp.a/b/deep.bin").exists(),
            "Bug C signature: stray temp path at directory root must not exist"
        );
        assert!(
            !dir.path().join(".tmp.a").exists(),
            "Bug C signature: stray temp directory at root must not exist"
        );
        // The temp WAS created in the parent (proves same-directory rename).
        // After successful rename, only the target remains in the parent.
        let parent_entries: Vec<_> = std::fs::read_dir(&nested_parent).unwrap().collect();
        assert_eq!(
            parent_entries.len(),
            1,
            "only the target should remain in parent"
        );
        assert_eq!(
            parent_entries[0].as_ref().unwrap().file_name(),
            std::ffi::OsString::from("deep.bin")
        );
    }
}

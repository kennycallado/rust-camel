# camel-file

File system component for rust-camel. Polls directories for new or changed files (consumer)
and writes exchange bodies to disk (producer).

## File Producer: Atomic Write Contract Surface

The producer's `Override` and `TryRename` write strategies route through a single
private helper: `crate::atomic_write::atomic_write`. The contract surface below governs
what the producer accepts, what it rejects, and what behavior is forbidden to be silent.

Spec reference: `docs/superpowers/specs/2026-06-20-rc-o6o-framework-contract-bugs.md` §3.3, §4.1.

### Accepted names / values

- **`fileName` URI param** and **`CamelFileName` exchange header**: arbitrary non-empty
  strings. Nested paths (`a/b/c.bin`) are accepted; the producer creates parent directories
  via `create_dir_all(target_path.parent())` before writing. Path-traversal protection is
  enforced by `validate_path_is_within_base` (`lib.rs:927`).
- **`fileExist` URI param**: `Override` (default), `Append`, `Fail`, `Ignore`, `TryRename`.
  Unknown values raise `CamelError::InvalidUri` at config time.
- **`tempPrefix` URI param**: a plain filename prefix (no path separators, no absolute paths,
  no null bytes). Validated by `is_valid_temp_prefix` (`lib.rs:919`). Required when
  `fileExist=TryRename`.
- **`durable` URI param**: boolean, default `false`. When `true`, the producer fsyncs the
  temp file and the parent directory after the atomic rename, in the order
  (temp → rename → parent dir). Crash-safe but slower. Errors propagate (caller opted into strict durability); users who want lenient behavior leave `durable=false`.

### Rejected names / values (strict, per ADR-0016)

- **`fileName` containing `..`**: rejected by `validate_path_is_within_base`.
- **`tempPrefix` containing path separators (`/`, `\`) or null bytes**: rejected at config
  time (`lib.rs:640-646`).
- **`tempPrefix` that is an absolute path**: rejected at config time.
- **`fileExist=TryRename` without `tempPrefix`**: rejected at config time (`lib.rs:648`).
- **`durable=maybe` (or any non-boolean)**: rejected with `CamelError::InvalidUri` by
  `parse_bool_param`.
- **Cross-filesystem rename (EXDEV)**: if the OS rejects the rename with EXDEV (errno 18),
  the producer returns `CamelError::ProcessorError("cross-filesystem rename rejected ...")`.
  The producer does NOT silently fall back to a non-atomic copy.

### Forbidden silent behavior

- The temp file MUST live at `target_path.parent().join(temp_prefix + target_path.file_name())`.
  Concatenating the prefix with the full nested `fileName` (the Bug C root cause) is forbidden.
- On write or rename failure, the temp file MUST be removed by the `TempFileGuard` RAII guard.
  A stray temp file after a failed write is a bug.
- `durable=true` MUST fsync in the order: temp file → rename → parent directory. Fsyncing the
  parent directory BEFORE the rename does NOT persist the new name and is forbidden.

## Language

**FileConfig**: URI-deserialized configuration for `file:` endpoints. Holds directory path,
polling delays, write strategy, temp-file prefix, durable flag, charset, and recursive-scan
options. Validated at construction (`lib.rs:613-694`); invalid configurations surface as
`CamelError::Config` or `CamelError::InvalidUri` before any exchange is processed.
_Avoid_: file options, file settings (use FileConfig when referring to the parsed struct).

**FileProducer**: Tower `Service<Exchange>` that writes the exchange body to disk under
`directory` + `fileName` (resolved from the `CamelFileName` header or `fileName` URI param).
Uses the `atomic_write` helper for `Override` and `TryRename` strategies. The `Fail` strategy uses `OpenOptions::create_new(true)` directly (the OS-level create is already atomic — no temp file or rename needed for `Fail`).
_Avoid_: file writer, file sink.

**atomic_write (pub(crate))**: The private helper at `src/atomic_write.rs` that performs
temp-file-then-rename atomically. Not exported outside the crate (YAGNI — one consumer).
If a second component needs it later, extract into a shared crate then.
_Avoid_: file writer utility, fs helper (too generic).

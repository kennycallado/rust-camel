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
  RELATIVE strings. Nested paths (`a/b/c.bin`) are accepted; the producer creates parent
  directories via `create_dir_all(target_path.parent())` before writing. Confinement is
  enforced in two layers: `validate_relative_filename` (lexical pre-check: rejects
  absolute paths, `..` components, NUL bytes — runs BEFORE any filesystem touch, since an
  absolute value would otherwise discard the base on `Path::join`) and
  `validate_path_is_within_base` (canonicalize-based base containment). Both in `src/lib.rs`.
- **`fileExist` URI param**: `Override` (default), `Append`, `Fail`, `Ignore`, `TryRename`.
  Unknown values raise `CamelError::InvalidUri` at config time. On Unix, the `Fail`,
  `Append`, and done-file opens use `O_NOFOLLOW` (`open_options_no_follow`, `src/lib.rs`):
  a SYMLINK LEAF fails the open, closing the check/open race for the final path
  component. Ancestor-directory replacement remains a residual TOCTOU surface
  (confinement is re-verified on the canonicalized path; `O_NOFOLLOW` cannot protect
  intermediate components).
- **`tempPrefix` URI param**: a plain filename prefix (no path separators, no absolute paths,
  no null bytes). Validated by `is_valid_temp_prefix` (`src/lib.rs`). Required when
  `fileExist=TryRename`. The generated temp name carries an additional unpredictable
  64-bit random infix (`prefix + hex + "." + file_name`), so a local attacker cannot
  pre-create the predicted path to force write failures.
- **`durable` URI param**: boolean, default `false`. When `true`, the producer fsyncs the
  temp file and the parent directory after the atomic rename, in the order
  (temp → rename → parent dir). Crash-safe but slower. Errors propagate (caller opted into strict durability); users who want lenient behavior leave `durable=false`.
- **`doneFileName` URI param**: done-marker filename pattern with `${file:name}`
  substitution. The substituted name passes through the SAME confinement as the body
  write (`validate_relative_filename` + `validate_path_is_within_base`) and is written
  with `O_NOFOLLOW` — a header-controlled `CamelFileName` cannot smuggle an absolute
  path or `..` into the done-file side path.
- **`maxWriteBytes` URI param**: u64, default `0` (unlimited). When > 0, the producer
  rejects bodies larger than the cap (text bodies pre-checked; stream bodies copied
  through a `take(max+1)` and error when the copied count exceeds the cap).

### Rejected names / values (strict, per ADR-0016)

- **`fileName` that is absolute, contains `..`, or contains NUL**: rejected by
  `validate_relative_filename` (lexical pre-check) — the error message names the
  parameter (`fileName` / `doneFileName`).
- **`tempPrefix` containing path separators (`/`, `\`) or null bytes**: rejected at config
  time by `FileConfig::validate` (`src/lib.rs`).
- **`tempPrefix` that is an absolute path**: rejected at config time.
- **`fileExist=TryRename` without `tempPrefix`**: rejected at config time by
  `FileConfig::validate` (`src/lib.rs`).
- **`durable=maybe` (or any non-boolean)**: rejected with `CamelError::InvalidUri` by
  `parse_bool_param`.
- **Symlink leafs on producer opens (Unix)**: `Fail`, `Append`, and done-file opens carry
  `O_NOFOLLOW`; a symlink FINAL component fails the open with `ELOOP`. Intermediate
  components are not symlink-protected (residual TOCTOU; see Accepted names / values).
- **Body larger than `maxWriteBytes`** (when > 0): rejected with
  `CamelError::ProcessorError` naming the limit.
- **Cross-filesystem rename (EXDEV)**: if the OS rejects the rename with EXDEV (errno 18),
  the producer returns `CamelError::ProcessorError("cross-filesystem rename rejected ...")`.
  The producer does NOT silently fall back to a non-atomic copy.

### Forbidden silent behavior

- The temp file MUST live at `target_path.parent().join(temp_prefix + random_infix + "." + target_path.file_name())`.
  Concatenating the prefix with the full nested `fileName` (the Bug C root cause) is forbidden.
  Dropping the random infix (making the temp name predictable again) is forbidden.
- On write or rename failure, the temp file MUST be removed by the `TempFileGuard` RAII guard.
  A stray temp file after a failed write is a bug.
- `durable=true` MUST fsync in the order: temp file → rename → parent directory. Fsyncing the
  parent directory BEFORE the rename does NOT persist the new name and is forbidden.
- The done-file write MUST NOT skip `validate_relative_filename` /
  `validate_path_is_within_base` — the substituted `${file:name}` derives from the
  header-influenced `file_name`, so it is exchange-data-influenced and needs the full
  confinement of the body write.

## Language

**FileConfig**: URI-deserialized configuration for `file:` endpoints. Holds directory path,
polling delays, write strategy, temp-file prefix, durable flag, charset, and recursive-scan
options. `FileConfig::validate` (`src/lib.rs`) validates it at construction. Invalid
configurations surface as `CamelError::Config` or `CamelError::InvalidUri` before any
exchange is processed.
_Avoid_: file options, file settings (use FileConfig when referring to the parsed struct).

**FileProducer**: Tower `Service<Exchange>` that writes the exchange body to disk under
`directory` + `fileName` (resolved from the `CamelFileName` header or `fileName` URI param).
Uses the `atomic_write` helper for `Override` and `TryRename` strategies. The `Fail` strategy uses `OpenOptions::create_new(true)` directly (the OS-level create is already atomic — no temp file or rename needed for `Fail`).
_Avoid_: file writer, file sink.

**atomic_write (pub(crate))**: The private helper at `src/atomic_write.rs` that performs
temp-file-then-rename atomically. Not exported outside the crate (YAGNI — one consumer).
If a second component needs it later, extract into a shared crate then.
_Avoid_: file writer utility, fs helper (too generic).

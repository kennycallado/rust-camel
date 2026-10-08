# Tasks: protox-compiler

Single phase (no `## Phase N` headings). Bd: `rc-15md7`. Spec:
`openspec/changes/protox-compiler/specs/data-formats/spec.md`. Design:
`openspec/changes/protox-compiler/design.md`.

## Execution contract (applies to EVERY task)

- Work only in the worktree `/home/shared/rust-camel-worktrees/347-protox`.
  Never run cargo in `/home/kenny/dev/rust-camel`. NEVER use `/tmp`; scratch and
  logs go to `/home/shared/tmp/347/` (create it if missing). Tests use
  `TMPDIR=/home/shared/tmp`.
- Before ANY cargo command run `df -h /home/shared | tail -1`. If used >= 78%,
  STOP and report `disk-guard: <value>`; do not build.
- Every cargo command runs through this wrapper (replace `<SUB>`, `<ARGS>` and `<LOG>`):

  ```text
  systemd-run --user --scope --collect --unit=fleet-protox \
    -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- \
    env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 \
    timeout 1500 cargo <SUB> <ARGS> > /home/shared/tmp/347/<LOG>.log 2>&1; rc=$?
  systemctl --user stop fleet-protox.scope
  tail -40 /home/shared/tmp/347/<LOG>.log
  exit $rc   # the pass/fail of the command is cargo's status, not tail's
  ```

  For `build`, `check`, `test` and `clippy` write `-j4` immediately after the
  subcommand word (`cargo test -j4 -p camel-proto-compiler ...`). Never append
  `-j4` after `--`, and never to `fmt`, `tree`, or `xtask` commands.
- Commit nothing; the conductor commits after review. Do not touch files outside the
  task's `Files` list. No `unwrap()` in non-test code (`cargo xtask lint-unwrap`).
- Before reporting: `cargo fmt --all` then `cargo clippy -p camel-proto-compiler --all-targets -- -D warnings`
  (through the wrapper) must pass for the crates the task touched.
- If a Tests entry is impossible or ambiguous, STOP and report `test-design-gap: <what>`.

## Conductor steps (not worker tasks)

- Before Task 4.2: bd `rc-x2rlm` (build.rs migration to protox, discovered-from `rc-15md7`) and bd `rc-sed5l` (editions support, blocked upstream) are filed; ADR-0084 Consequences cites both ids, plus `rc-me2ii` and `rc-grvqh`.
- The sealed-artifact temporary-directory gap is bd `rc-joask` (cite it in ADR-0084).

## camel-proto-compiler

### Task 1.1: Add the `protox` dependency

Spec: design.md "Approach" (workspace pin). Keep `protoc-bin-vendored` for now;
Task 1.3 removes it.

**Files:**
- `Cargo.toml` (modified)
- `crates/services/camel-proto-compiler/Cargo.toml` (modified)
- `Cargo.lock` (modified, by cargo)

**Steps:**
1. In root `Cargo.toml` `[workspace.dependencies]`, add the line `protox = "=0.9.1"` directly below the `prost-reflect = { version = "0.16", features = ["serde"] }` line.
2. In `crates/services/camel-proto-compiler/Cargo.toml` `[dependencies]`, add `protox = { workspace = true }`.
3. Run `cargo check -p camel-proto-compiler` (wrapper, log `t1.1`). It resolves `protox` 0.9.1 and updates `Cargo.lock`.
4. Run `git diff Cargo.lock` and confirm: added `[[package]]` blocks for the protox family (`protox`, `protox-parse`, `miette`, `miette-derive`, `logos*`, `beef`, `unicode-width`, and similar); dependency-list lines added inside existing package blocks as a consequence of protox feature activation (for example `prost-reflect` gaining `miette`/`logos`) and inside `camel-proto-compiler`; NO change to the `version` or `checksum` of any existing package. Otherwise STOP and report `lock-churn: <package>`.

**Tests:**
- `n/a (dependency task)`: verified by acceptance commands.

**Acceptance:**
- `cargo check -p camel-proto-compiler` exits 0 (log `t1.1`).
- `rg -n 'protox' Cargo.toml crates/services/camel-proto-compiler/Cargo.toml` shows exactly the two added lines.
- `git diff Cargo.lock` removes no `[[package]]` block and changes no existing `version` or `checksum` line.

- [x] 1.1

### Task 1.2: Nesting scanner module

Spec: "Malformed proto or descriptor-set input never terminates the process"
(scanner rules in design.md Approach step 3).

**Files:**
- `crates/services/camel-proto-compiler/src/nesting.rs` (new)
- `crates/services/camel-proto-compiler/src/lib.rs` (modified: add `mod nesting;` only)

**Steps:**
1. Create `nesting.rs` with exactly these items (write the unit tests first, see Tests; then add the implementation):

   ```rust
   //! Bracket-nesting scanner used before protox parses any text.

   /// Maximum bracket nesting depth accepted in `.proto` sources and in
   /// descriptor-set option text.
   pub(crate) const MAX_NESTING_DEPTH: usize = 64;

   /// Which text dialect is scanned.
   #[derive(Clone, Copy, PartialEq, Eq, Debug)]
   pub(crate) enum ScanMode {
       /// `.proto` source: `//` and `/* */` comments, quoted strings.
       ProtoSource,
       /// Descriptor option text (text format): as `ProtoSource`, plus `#`
       /// starts a line comment.
       OptionText,
   }

   /// Scans `text`. Openers are `{ < [`, closers are `} > ]`; the depth never
   /// goes below 0. Returns `Err(depth)` with the first depth above `limit`.
   pub(crate) fn scan_nesting(text: &[u8], limit: usize, mode: ScanMode) -> Result<(), usize> {
       let mut depth: usize = 0;
       let mut i = 0;
       while i < text.len() {
           let c = text[i];
           let next = text.get(i + 1).copied();
           if c == b'/' && next == Some(b'/') || (c == b'#' && mode == ScanMode::OptionText) {
               while i < text.len() && text[i] != b'\n' {
                   i += 1;
               }
               continue;
           }
           if c == b'/' && next == Some(b'*') {
               i += 2;
               while i < text.len() && !(text[i] == b'*' && text.get(i + 1) == Some(&b'/')) {
                   i += 1;
               }
               i += 2;
               continue;
           }
           if c == b'"' || c == b'\'' {
               i += 1;
               while i < text.len() {
                   match text[i] {
                       b'\\' if text.get(i + 1).is_some_and(|n| *n != b'\n') => i += 2,
                       b'\n' => break,
                       q if q == c => {
                           i += 1;
                           break;
                       }
                       _ => i += 1,
                   }
               }
               continue;
           }
           match c {
               b'{' | b'<' | b'[' => {
                   depth += 1;
                   if depth > limit {
                       return Err(depth);
                   }
               }
               b'}' | b'>' | b']' => depth = depth.saturating_sub(1),
               _ => {}
           }
           i += 1;
       }
       Ok(())
   }
   ```
2. In `lib.rs` add `mod nesting;` after `mod compiler;`. Nothing uses the module yet, so add `#![allow(dead_code)]` at the top of `nesting.rs` (temporary).

**Tests:** (unit tests in `#[cfg(test)] mod tests` inside `nesting.rs`; helper `fn rep(s: &str, n: usize) -> Vec<u8>` returns `s.repeat(n).into_bytes()`)
- `ok_at_limit`: `rep("{", 64) + rep("}", 64)` with limit 64, `ProtoSource` → `Ok(())`.
- `err_one_over_limit`: `rep("{", 65)` → `Err(65)`.
- `angle_and_square_count`: `rep("<", 65)` → `Err(65)`; `rep("[", 65)` → `Err(65)`.
- `line_comment_ignored`: `b"// "` + `rep("{", 100)` + `b"\n"` → `Ok(())`.
- `block_comment_ignored`: `b"/* "` + `rep("{", 100)` + `b" */"` → `Ok(())`; unterminated `b"/* "` + `rep("{", 100)` → `Ok(())`.
- `braces_in_string_ignored`: `b"\""` + `rep("{", 100)` + `b"\""` → `Ok(())`; same with single quotes → `Ok(())`.
- `string_ends_at_newline`: `b"x = \"abc\n"` + `rep("{", 100)` → `Err(65)`.
- `escaped_quote_does_not_end_string`: `br#"x = "\"" "#` + `rep("{", 100)` → `Err(65)`; and `br#"x = "\"{{{{"#` + `b"\n"` → `Ok(())` (the brace run is inside the string).
- `closers_never_go_negative`: `rep("}", 100) + rep("{", 65)` → `Err(65)`.
- `hash_comment_only_in_option_text`: input `rep("f < # >\n", 100)`: `OptionText` → `Err(65)`; `ProtoSource` → `Ok(())`.
- `typical_proto_passes`: `b"map<string, string> m = 1 [deprecated = true]; message A { message B { } }"` → `Ok(())`.
- Command: `cargo test -p camel-proto-compiler --lib nesting` (wrapper, log `t1.2`). Expected before the implementation: compile failure or test failure; after: pass.

**Acceptance:**
- `cargo test -p camel-proto-compiler --lib nesting` passes with 11 tests.
- `cargo clippy -p camel-proto-compiler --all-targets -- -D warnings` exits 0.
- `cargo fmt --all -- --check` exits 0.

- [x] 1.2

### Task 1.3: New error type and in-process source compilation; remove the protoc path

Spec: "Protobuf runtime compilation is hermetic", "Malformed proto or
descriptor-set input never terminates the process" (compile side), "Editions
source is rejected with a remedy". Design Approach steps 1, 3, 4.

**Files:**
- `crates/services/camel-proto-compiler/Cargo.toml` (modified)
- `crates/services/camel-proto-compiler/src/lib.rs` (modified)
- `crates/services/camel-proto-compiler/src/compiler.rs` (modified: rewritten)
- `crates/services/camel-proto-compiler/src/nesting.rs` (modified: narrow the temporary dead-code allowance, see step 7)

**Steps:**
1. `Cargo.toml`: remove the `protoc-bin-vendored` line; move `tempfile = { workspace = true }` from `[dependencies]` to a new `[dev-dependencies]` table (also add `serde_json = { workspace = true }` there). `[lints] workspace = true` stays.
2. `lib.rs`: replace the `ProtoCompileError` enum with this exact definition (the old `ProtocUnavailable` and `ProtocFailed` variants are deleted):

   ```rust
   #[derive(Debug, thiserror::Error)]
   #[non_exhaustive]
   pub enum ProtoCompileError {
       #[error("proto file not found: {0}")]
       ProtoNotFound(PathBuf),
       #[error("I/O error: {0}")]
       Io(#[from] std::io::Error),
       #[error("failed to compile {}: {detail}", path.display())]
       Compile { path: PathBuf, detail: String },
       #[error("failed to decode descriptor pool: {0}")]
       DescriptorDecode(String),
   }
   ```
   Replace the crate doc comment (lines 1-9) so it states: compilation is in-process with protox; a precompiled descriptor set (`.binpb`, `.pb`, `.desc`, `.protoset`) is accepted wherever a `.proto` path is; editions are unsupported; no external compiler is ever used. Do NOT write the token `PROTOC` or `protoc-bin-vendored` or `tempfile` in non-test code (Task 2.1 scans for them; lowercase `protoc` is allowed only inside the editions remedy string).
3. `lib.rs` tests module: delete `PROTOC_COMPILE_LOCK`, `ProtocEnvGuard`, `write_fake_protoc`, and the tests `test_descriptor_file_cleaned_up`, `protoc_env_short_circuits_vendored_resolver`, `protoc_env_empty_string_is_honored_as_set`, `vendored_panic_contained_as_protoc_unavailable`, `vendored_panic_does_not_invoke_panic_hook`, `vendored_err_passes_through_seam_verbatim`, `resolve_protoc_vendored_present_returns_file`, `protoc_env_marker_script_serves_compilation`, `protoc_env_broken_override_fails_without_fallback`, and the `use crate::compiler::{resolve_protoc, resolve_protoc_with}` import. In the kept tests (`compile_proto_success`, `compile_proto_missing_file_returns_error`, `compile_proto_invalid_syntax_returns_error`, `cache_hit_does_not_duplicate_entries`, `cache_does_not_grow_beyond_max`, `test_concurrent_compiles_do_not_clobber`, `cache_invalidation_on_content_change`) remove every `let _guard = PROTOC_COMPILE_LOCK.lock().unwrap();` line. In `compile_proto_invalid_syntax_returns_error` assert `matches!(&err, ProtoCompileError::Compile { detail, .. } if detail.contains(":2:"))`.
4. `compiler.rs`: delete everything above `pub fn compile_proto` (the `Command` import, `resolve_protoc`, `PanicHook`, `HOOK_SWAP`, `SilencePanicHook`, `resolve_protoc_with`, `panic_payload_message`). Rewrite the file with these items:

   ```rust
   use std::path::{Path, PathBuf};

   use prost_reflect::DescriptorPool;
   use protox::file::{
       ChainFileResolver, File, FileResolver, GoogleFileResolver, IncludeFileResolver,
   };
   use tracing::debug;

   use crate::ProtoCompileError;
   use crate::nesting::{MAX_NESTING_DEPTH, ScanMode, scan_nesting};

   /// Remedy appended when a source uses protobuf editions.
   const EDITIONS_REMEDY: &str = "protobuf editions are not supported; rewrite the schema with syntax = \"proto3\" or \"proto2\"";

   /// Error raised by the nesting guard. `Debug` equals `Display` so the
   /// protox `Debug` form of the error stays readable.
   struct NestingExceeded { name: String, depth: usize }
   impl std::fmt::Display for NestingExceeded {
       fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
           write!(f, "{}: nesting depth {} exceeds the limit of {}", self.name, self.depth, MAX_NESTING_DEPTH)
       }
   }
   impl std::fmt::Debug for NestingExceeded {
       fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
           std::fmt::Display::fmt(self, f)
       }
   }
   impl std::error::Error for NestingExceeded {}

   /// Include-directory resolver that scans file text before protox parses it.
   struct NestingGuardedInclude { dir: PathBuf, inner: IncludeFileResolver }
   impl NestingGuardedInclude {
       fn new(dir: PathBuf) -> Self { Self { inner: IncludeFileResolver::new(dir.clone()), dir } }
   }
   impl FileResolver for NestingGuardedInclude {
       fn resolve_path(&self, path: &Path) -> Option<String> { self.inner.resolve_path(path) }
       fn open_file(&self, name: &str) -> Result<File, protox::Error> {
           let candidate = self.dir.join(name);
           if let Ok(meta) = std::fs::metadata(&candidate)
               && meta.is_file()
               && meta.len() <= i32::MAX as u64
               && let Ok(bytes) = std::fs::read(&candidate)
               && let Err(depth) = scan_nesting(&bytes, MAX_NESTING_DEPTH, ScanMode::ProtoSource)
           {
               return Err(protox::Error::new(NestingExceeded { name: name.to_owned(), depth }));
           }
           self.inner.open_file(name)
       }
   }
   ```
   (If the `let`-chains do not compile on the workspace edition, rewrite as nested `if let`; behavior must stay identical: any read failure falls through to `self.inner.open_file(name)` unchanged.)
5. Continue `compiler.rs` with panic containment helpers and the compile pipeline:

   ```rust
   fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
       if let Some(m) = payload.downcast_ref::<String>() {
           m.clone()
       } else if let Some(m) = payload.downcast_ref::<&str>() {
           (*m).to_string()
       } else {
           "unknown panic payload".to_string()
       }
   }

   /// Runs `f`; a panic becomes `ProtoCompileError::Compile` with detail
   /// `internal compiler panic: <message>`. The global panic hook is untouched.
   pub(crate) fn contained_compile(
       path: &Path,
       f: impl FnOnce() -> Result<DescriptorPool, ProtoCompileError>,
   ) -> Result<DescriptorPool, ProtoCompileError> {
       std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|payload| {
           Err(ProtoCompileError::Compile {
               path: path.to_path_buf(),
               detail: format!("internal compiler panic: {}", panic_message(payload)),
           })
       })
   }

   /// Maps a protox error to `Compile`. `detail` is the protox `Debug` form
   /// (`file:line:col: message`); the editions remedy is appended when the
   /// detail contains `found 'edition'`.
   fn compile_error(path: &Path, e: &protox::Error) -> ProtoCompileError {
       let mut detail = format!("{e:?}");
       if detail.contains("found 'edition'") {
           detail.push_str(". ");
           detail.push_str(EDITIONS_REMEDY);
       }
       ProtoCompileError::Compile { path: path.to_path_buf(), detail }
   }

   fn compile_source(proto_path: &Path, includes: &[PathBuf]) -> Result<DescriptorPool, ProtoCompileError> {
       let parent = match proto_path.parent() {
           Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
           _ => PathBuf::from("."),
       };
       let mut resolver = ChainFileResolver::new();
       for dir in includes.iter().cloned().chain(std::iter::once(parent)) {
           resolver.add(NestingGuardedInclude::new(dir));
       }
       resolver.add(GoogleFileResolver::new());
       contained_compile(proto_path, || {
           let mut compiler = protox::Compiler::with_file_resolver(resolver);
           compiler.include_imports(true).include_source_info(false);
           compiler.open_file(proto_path).map_err(|e| compile_error(proto_path, &e))?;
           let bytes = compiler.encode_file_descriptor_set();
           DescriptorPool::decode(bytes.as_slice()).map_err(|e| {
               ProtoCompileError::DescriptorDecode(format!("{}: {e}", proto_path.display()))
           })
       })
   }

   pub fn compile_proto<P, I>(proto_path: P, includes: I) -> Result<DescriptorPool, ProtoCompileError>
   where P: AsRef<Path>, I: IntoIterator, I::Item: AsRef<Path>,
   {
       let proto_path = proto_path.as_ref();
       if !proto_path.exists() {
           return Err(ProtoCompileError::ProtoNotFound(proto_path.to_path_buf()));
       }
       let include_paths = includes.into_iter().map(|p| p.as_ref().to_path_buf()).collect::<Vec<PathBuf>>();
       debug!(proto = %proto_path.display(), "compiling proto");
       compile_source(proto_path, &include_paths)
   }
   ```
   Keep the `#[doc]` on `compile_proto` describing both inputs once Task 1.4 lands (for now: source only). The pool is built ONLY from `encode_file_descriptor_set()`; never call `Compiler::descriptor_pool()`.
6. Add a `#[cfg(test)] mod tests` in `compiler.rs` for the panic seam (see Tests).
7. In `nesting.rs` replace the file-level `#![allow(dead_code)]` with a single `#[allow(dead_code)]` on the `OptionText` variant of `ScanMode` (temporary; production code does not use it until Task 1.4). Everything else in `nesting.rs` is used by Task 1.3's resolver and must not need an allowance.

**Tests:** (in-crate unit tests; the integration files come in Tasks 2.x)
- `contained_compile_maps_panic` (`compiler.rs` tests): arrange none; act `contained_compile(Path::new("x.proto"), || panic!("boom"))`; assert result is `Err(ProtoCompileError::Compile { detail, .. })` with `detail.starts_with("internal compiler panic:")` and `detail.contains("boom")`, and the test itself returns (process alive).
- `contained_compile_passes_ok_and_err_through`: `contained_compile(p, || Ok(DescriptorPool::new()))` is `Ok`; `contained_compile(p, || Err(ProtoCompileError::DescriptorDecode("e".into())))` is `Err(DescriptorDecode(s))` with `s == "e"`.
- `edition_detail_gets_remedy`: write a file `ed.proto` containing `edition = "2023";\npackage ed;\nmessage E { string a = 1; }\n` in a `tempfile::tempdir()`; `compile_proto` → `Err(Compile { detail, .. })`; assert `detail.contains("editions")` and `detail.contains("proto3")`.
- `compile_proto_success`, `compile_proto_invalid_syntax_returns_error`, cache tests, concurrency test: the retained lib.rs tests listed in step 3 must pass.
- Command: `cargo test -p camel-proto-compiler --lib` (wrapper, log `t1.3`).

**Acceptance:**
- `cargo test -p camel-proto-compiler --lib` passes (retained lib tests + 3 new + 11 nesting).
- `rg -n 'Command|tempfile|temp_dir|PROTOC|protoc-bin-vendored|std::process' crates/services/camel-proto-compiler/src` returns hits ONLY inside `#[cfg(test)]` modules (`tempfile` in tests is fine) and the lowercase-only remedy string.
- `cargo tree -p camel-proto-compiler -e normal | rg 'protoc-bin-vendored'` prints nothing (empty stdout, `rg` exit status 1 is the pass condition).
- `rg -n 'ProtocUnavailable|ProtocFailed' crates --glob '*.rs'` returns no hits.
- `cargo clippy -p camel-proto-compiler --all-targets -- -D warnings` and `cargo xtask lint-unwrap` exit 0.

- [x] 1.3

### Task 1.4: Descriptor-set input with option-text guard

Spec: "Precompiled descriptor set input"; "Malformed ... input" (descriptor side); "Editions are unsupported" (descriptor side).
Design Approach step 2.

**Files:**
- `crates/services/camel-proto-compiler/src/compiler.rs` (modified)
- `crates/services/camel-proto-compiler/src/nesting.rs` (modified: remove the temporary `#[allow(dead_code)]` on `OptionText`)

**Steps:**
1. Add imports `prost::Message` and `prost_reflect::prost_types::{DescriptorProto, EnumDescriptorProto, FileDescriptorSet, UninterpretedOption}`. (Tests additionally import `prost_reflect::prost_types::{FileDescriptorProto, FileOptions, uninterpreted_option::NamePart}`.)
2. Add `pub(crate) fn contained_decode(path: &Path, f: impl FnOnce() -> Result<DescriptorPool, ProtoCompileError>) -> Result<DescriptorPool, ProtoCompileError>`: same as `contained_compile` but a panic maps to `ProtoCompileError::DescriptorDecode(format!("internal decoder panic: {} ({})", panic_message(payload), path.display()))`.
3. Add `fn is_descriptor_set(path: &Path) -> bool`: true when `path.extension().and_then(|e| e.to_str())` equals, ASCII case-insensitively, one of `binpb`, `pb`, `desc`, `protoset`.
4. Add the option-text checker. Each helper takes a slice of `UninterpretedOption`, scans each `aggregate_value` (when `Some`) with `scan_nesting(s.as_bytes(), MAX_NESTING_DEPTH, ScanMode::OptionText)` and returns `Err(depth)` on the first violation:

   ```rust
   fn check_uninterpreted(opts: &[UninterpretedOption]) -> Result<(), usize> {
       for o in opts {
           if let Some(text) = &o.aggregate_value {
               scan_nesting(text.as_bytes(), MAX_NESTING_DEPTH, ScanMode::OptionText)?;
           }
       }
       Ok(())
   }
   fn check_enum(e: &EnumDescriptorProto) -> Result<(), usize> {
       if let Some(o) = &e.options {
           check_uninterpreted(&o.uninterpreted_option)?;
       }
       for v in &e.value {
           if let Some(o) = &v.options {
               check_uninterpreted(&o.uninterpreted_option)?;
           }
       }
       Ok(())
   }
   fn check_message(m: &DescriptorProto) -> Result<(), usize> {
       if let Some(o) = &m.options {
           check_uninterpreted(&o.uninterpreted_option)?;
       }
       for f in m.field.iter().chain(m.extension.iter()) {
           if let Some(o) = &f.options {
               check_uninterpreted(&o.uninterpreted_option)?;
           }
       }
       for d in &m.oneof_decl {
           if let Some(o) = &d.options {
               check_uninterpreted(&o.uninterpreted_option)?;
           }
       }
       for r in &m.extension_range {
           if let Some(o) = &r.options {
               check_uninterpreted(&o.uninterpreted_option)?;
           }
       }
       for e in &m.enum_type {
           check_enum(e)?;
       }
       for n in &m.nested_type {
           check_message(n)?;
       }
       Ok(())
   }
   fn check_set_options(set: &FileDescriptorSet) -> Result<(), usize> {
       for f in &set.file {
           if let Some(o) = &f.options {
               check_uninterpreted(&o.uninterpreted_option)?;
           }
           for m in &f.message_type {
               check_message(m)?;
           }
           for e in &f.enum_type {
               check_enum(e)?;
           }
           for x in &f.extension {
               if let Some(o) = &x.options {
                   check_uninterpreted(&o.uninterpreted_option)?;
               }
           }
           for s in &f.service {
               if let Some(o) = &s.options {
                   check_uninterpreted(&o.uninterpreted_option)?;
               }
               for m in &s.method {
                   if let Some(o) = &m.options {
                       check_uninterpreted(&o.uninterpreted_option)?;
                   }
               }
           }
       }
       Ok(())
   }
   ```
4b. Add `fn has_editions(set: &FileDescriptorSet) -> bool { set.file.iter().any(|f| f.syntax.as_deref() == Some("editions")) }`.
5. Add `fn load_descriptor_set(path: &Path) -> Result<DescriptorPool, ProtoCompileError>`:
   `let bytes = std::fs::read(path)?;` then `contained_decode(path, closure)` where the closure body runs three steps in order: (a) `FileDescriptorSet::decode(bytes.as_slice())` mapping error to `DescriptorDecode(format!("{}: {e}", path.display()))`; (a2) if `has_editions(&set)` return `DescriptorDecode(format!("{}: protobuf editions descriptor sets are not supported; rewrite the schema with syntax = \"proto3\" or \"proto2\"", path.display()))`; (b) `check_set_options(&set)` mapping `Err(depth)` to `DescriptorDecode(format!("{}: option text nesting depth {depth} exceeds the limit of {MAX_NESTING_DEPTH}", path.display()))`; (c) `DescriptorPool::decode(bytes.as_slice())` (the ORIGINAL bytes, not `from_file_descriptor_set`) mapping error to `DescriptorDecode(format!("{}: {e}", path.display()))`.
6. In `compile_proto`, after the `ProtoNotFound` check and before collecting includes, add `if is_descriptor_set(proto_path) { debug!(descriptor_set = %proto_path.display(), "loading precompiled descriptor set"); return load_descriptor_set(proto_path); }`. Includes are ignored for this branch. Update the `compile_proto` doc comment to describe both inputs and that includes are ignored for descriptor sets.

**Tests:** (unit tests in the `compiler.rs` `#[cfg(test)] mod tests`; use `tempfile::tempdir()`)
- `contained_decode_maps_panic`: `contained_decode(Path::new("x.binpb"), || panic!("boom"))` → `Err(DescriptorDecode(s))` with `s.starts_with("internal decoder panic:")`, `s.contains("boom")`, `s.contains("x.binpb")`.
- `is_descriptor_set_extensions`: `is_descriptor_set` is true for `a.binpb`, `a.PB`, `a.Desc`, `a.protoset`, false for `a.proto`, `a`, `a.binpbx`.
- `descriptor_set_loads_via_compile_proto`: path `<CARGO_MANIFEST_DIR>/tests/helloworld.desc` (existing fixture) → `Ok`, pool has message `helloworld.HelloRequest`.
- `handbuilt_editions_descriptor_set_is_typed_error`: a hand-built `FileDescriptorSet` with one `FileDescriptorProto { name: Some("e.proto".into()), syntax: Some("editions".into()), ..Default::default() }` encoded to `e.binpb` → `Err(DescriptorDecode(s))` with `s.contains("editions")` (independent of protoc).
- `corrupt_descriptor_set_is_typed_error`: write bytes `[0xff, 0xff, 0xff, 0xff, 0xff]` to `bad.binpb` → `Err(DescriptorDecode(s))` and `s.contains("bad.binpb")`.
- `hostile_option_text_is_typed_error`: build a `FileDescriptorSet` with one `FileDescriptorProto { name: Some("h.proto".into()), options: Some(FileOptions { uninterpreted_option: vec![UninterpretedOption { name: vec![NamePart { name_part: "x".into(), is_extension: true }], aggregate_value: Some("f < ".repeat(10_000)), ..Default::default() }], ..Default::default() }), ..Default::default() }`, encode with `prost::Message::encode_to_vec`, write to `h.binpb`; run `compile_proto` on a thread with `std::thread::Builder::new().stack_size(2 * 1024 * 1024)` (move a cloned `PathBuf` into the `'static` closure); assert `Err(DescriptorDecode(s))` with `s.contains("option text nesting depth")`. A second case with `aggregate_value = "f < # >\n".repeat(10_000)` has the same outcome.
- Command: `cargo test -p camel-proto-compiler --lib` (wrapper, log `t1.4`).

**Acceptance:**
- `cargo test -p camel-proto-compiler --lib` passes (all earlier tests plus the 6 above).
- `cargo clippy -p camel-proto-compiler --all-targets -- -D warnings`, `cargo fmt --all -- --check` and `cargo xtask lint-unwrap` exit 0.
- `compile_proto` on a `.binpb` never reads any include directory (code inspection: the branch returns before the include list is built).

- [x] 1.4

## camel-proto-compiler integration tests

### Task 2.1: Hermetic child-process tests and no-spawn-surface scan

Spec scenarios: "Compilation works with no protoc, no PATH and no temp directory";
"An environment override cannot bring an external compiler back"; "The runtime
crate has no executable-spawning surface".

**Files:**
- `crates/services/camel-proto-compiler/tests/hermetic.rs` (new)

**Steps:**
1. Create `tests/hermetic.rs` with a helper `fn manifest() -> PathBuf` returning `PathBuf::from(env!("CARGO_MANIFEST_DIR"))` (compile-time macro; the child process runs with a cleared environment, so never read `CARGO_MANIFEST_DIR` at runtime) and fixture helpers: local `tests/helloworld.proto`; gRPC `../../components/camel-component-grpc/tests/helloworld.proto` and `.../streaming.proto`; `../../dataformats/camel-dataformat-protobuf/tests/fixtures/recursive.proto`; kitchen sink `tests/fixtures/ks/main/kitchen.proto` with include `tests/fixtures/ks/lib`.
2. Child body test `hermetic_child_body`: if env `CAMEL_PROTO_HERMETIC_CHILD` is not `1`, return immediately (passes as a no-op). If env `CAMEL_PROTO_HERMETIC_RELATIVE` is set, run `compile_proto(<that relative path>, std::iter::empty::<&Path>())`, assert pool has `helloworld.HelloRequest`, and return. Otherwise compile every fixture in step 1 and assert: helloworld has `helloworld.HelloRequest`, `helloworld.HelloReply`, service `helloworld.Greeter`; streaming has services `streaming.StreamService` and messages `streaming.ListRequest`, `streaming.EchoResponse`; recursive has message `test.Node`; kitchen sink has messages `kitchen.v1.Order`, `kitchen.v1.Order.Line`, `common.Address`, `legacy.Legacy` and service `kitchen.v1.OrderService`. Also compile `tests/helloworld.proto` through the relative path `tests/helloworld.proto` (the child inherits the crate root as cwd) and assert the pool has `helloworld.Greeter`. Also assert `std::env::var_os("PROTOC").is_none()` unless env `CAMEL_PROTO_HERMETIC_EXPECT_PROTOC` is set (that case sets a hostile value on purpose).
3. Helper `fn run_child(envs: &[(&str, &str)], cwd: Option<&Path>) -> std::process::Output`: `std::process::Command::new(std::env::current_exe().unwrap())` with args `["--exact", "hermetic_child_body", "--nocapture", "--test-threads=1"]`, `env_clear()`, then `.env("CAMEL_PROTO_HERMETIC_CHILD", "1")`, `.env("TMPDIR", "/home/shared/tmp/347/does-not-exist")`, `.env("PATH", "")`, then the given `envs`, and `current_dir(cwd)` when given. (`unwrap` is fine in tests.)
4. Parent tests (each asserts `output.status.success()` and prints stderr on failure):
   - `compiles_with_no_protoc_no_path_no_tmpdir`: `run_child(&[], None)`.
   - `empty_parent_relative_path_compiles`: `run_child(&[("CAMEL_PROTO_HERMETIC_RELATIVE", "helloworld.proto")], Some(&manifest().join("tests")))`.
   - `env_override_cannot_bring_back_external_compiler`: `tempfile::tempdir()` as `dir`; write executable shell script `dir/protoc` containing `#!/bin/sh\nprintf invoked >> "${0%/*}/marker"\nexit 1\n` (mode 0o755; the script calls no external utility because `PATH` is restricted); first run the script once directly from the parent (`Command::new(&script).status()`), assert `dir/marker` now exists (control: the marker mechanism works), then delete `dir/marker`; `run_child(&[("PATH", dir_str), ("PROTOC", script_str), ("CAMEL_PROTO_HERMETIC_EXPECT_PROTOC", "1")], None)` (later `.env` entries override earlier ones, so `PATH` becomes `dir_str`); assert child success AND `!dir.join("marker").exists()`.
5. Declare `static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());` and hold its guard (`let _g = SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());`) in each parent test across the script write, the control run and every `run_child` call, so a freshly written script is never executed while a sibling test forks (ETXTBSY). No sleeps.
6. Scan test `no_external_spawn_surface_in_runtime_sources`: for every `*.rs` file in `src/`, take the text before the first line equal to `#[cfg(test)]` (whole file if none) and assert it contains none of the case-sensitive tokens `Command`, `tempfile`, `temp_dir`, `PROTOC`, `protoc-bin-vendored`, `std::process`. Then read `Cargo.toml`, take the text from `[dependencies]` to the next line starting with `[`, assert it contains neither `protoc-bin-vendored` nor `tempfile`. (The test source file itself lives in `tests/`, which is not scanned.)

**Tests:** (executable spec — the 5 tests above)
- `compiles_with_no_protoc_no_path_no_tmpdir`, `empty_parent_relative_path_compiles`, `env_override_cannot_bring_back_external_compiler`, `no_external_spawn_surface_in_runtime_sources`, plus the no-op `hermetic_child_body`.
- Command: `cargo test -p camel-proto-compiler --test hermetic` (wrapper, log `t2.1`). Expected: pass (implementation exists from 1.3). To prove the scan test bites, temporarily add the text `// Command` in `src/compiler.rs` above the tests module, confirm the scan test FAILS, then remove it.

**Acceptance:**
- `cargo test -p camel-proto-compiler --test hermetic` passes, 5 tests.
- `git diff --stat` shows `src/compiler.rs` unchanged by this task.
- clippy `--all-targets -D warnings` and `cargo fmt --all -- --check` exit 0.

- [x] 2.1

### Task 2.2: Parity, packed-encoding and repo-fixture tests

Spec scenarios: "Packed encoding matches protoc"; hermetic scenario fixture set;
the golden descriptor sets were produced once by protoc 31.1
(`tests/fixtures/golden/*.binpb`, committed by the conductor).

**Files:**
- `crates/services/camel-proto-compiler/tests/parity.rs` (new)

**Steps:**
1. Create `tests/parity.rs`. Helpers: `fn manifest()`, `fn compile(path, includes)` wrapping `camel_proto_compiler::compile_proto`, and `fn encode(pool, message_name, json) -> Vec<u8>` that builds a `DynamicMessage` with `DynamicMessage::deserialize(desc, &mut serde_json::Deserializer::from_str(json))` and `encode_to_vec()` (`prost_reflect::DynamicMessage`, `prost::Message`; both crates are dependencies, `serde_json` is a dev-dependency).
2. `fn normalized(pool) -> BTreeMap<String, FileDescriptorProto>`: from `pool.file_descriptor_protos()`, skip the file named `google/protobuf/descriptor.proto` (protox serves an older copy than protoc 31.1), set `source_code_info = None`, and for every service method whose `options == Some(MethodOptions::default())` set `options = None` (protoc emits an empty options message, protox omits it).
3. Write the tests below.

**Tests:**
- `kitchen_sink_matches_protoc_golden`: pool A = `compile_proto(tests/fixtures/ks/main/kitchen.proto, [tests/fixtures/ks/lib])`; pool B = `compile_proto(tests/fixtures/golden/kitchen.binpb, [])`; assert `normalized(A) == normalized(B)` (same file names, equal protos).
- `kitchen_sink_json_to_wire_matches_golden`: with JSON `{"id":"o1","note":"n","status":"ACTIVE","lines":[{"sku":"s","qty":2}],"weights":[1.5,2.5],"tags":{"7":"x"},"card":"4111","created":"2024-01-02T03:04:05Z","ttl":"3s","priority":"HIGH"}` encode `kitchen.v1.Order` with pool A and pool B; assert byte-equal; assert the bytes contain the two-byte sequence `[0xa1, 0x01]` at least twice (field 20 `weights` unpacked, wire type 1) and do not contain `[0xa2, 0x01]` (packed tag).
- `proto3_packed_false_encodes_unpacked`: compile `tests/fixtures/packed/p3.proto`; JSON `{"w":[1.0,2.0],"d":[3.0]}` for `p3.P`; assert bytes equal `[0x09, 0,0,0,0,0,0,0xf0,0x3f, 0x09, 0,0,0,0,0,0,0,0x40, 0x12, 0x08, 0,0,0,0,0,0,0x08,0x40]`.
- `proto2_packed_true_encodes_packed`: compile `tests/fixtures/packed/p2.proto`; JSON `{"values":[1,2],"plain":[3,4]}` for `p2.P`; assert bytes equal `[0x0a, 0x02, 0x01, 0x02, 0x10, 0x03, 0x10, 0x04]`.
- `repo_fixtures_compile`: helloworld (local `tests/helloworld.proto`, and the gRPC crate copy via `../../components/camel-component-grpc/tests/helloworld.proto`), streaming (`streaming.StreamService` with three methods), recursive (`test.Node`): each compiles and exposes the names listed in Task 2.1 step 2.
- `repo_fixture_helloworld_json_roundtrip`: compile gRPC `helloworld.proto`; encode `helloworld.HelloRequest` from `{"name":"Camel"}`; assert bytes equal `[0x0a, 0x05, b'C', b'a', b'm', b'e', b'l']`.
- `precompiled_helloworld_equals_compiled`: `normalized(compile(tests/helloworld.proto))` equals `normalized(compile(tests/fixtures/golden/helloworld.binpb))`.
- Command: `cargo test -p camel-proto-compiler --test parity` (wrapper, log `t2.2`).

**Acceptance:**
- `cargo test -p camel-proto-compiler --test parity` passes, 7 tests.
- If `normalized` equality fails for a reason other than the two allowances in step 2, STOP and report `parity-gap: <diff>`; do not widen the allowances.
- clippy `--all-targets -D warnings` and `cargo fmt --all -- --check` exit 0.

- [x] 2.2

### Task 2.3: Hostile-input tests (nesting, imports, descriptor sets)

Spec scenarios: "Deep nesting is a typed error"; "Angle-bracket option nesting is a typed error";
"An unterminated string cannot hide deep nesting"; "An escaped quote cannot hide deep nesting";
"Deep nesting in an imported file is a typed error"; "A hostile descriptor set fails with a typed error".

**Files:**
- `crates/services/camel-proto-compiler/tests/hostile.rs` (new)

**Steps:**
1. Create `tests/hostile.rs`. Helper `fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T` runs `f` on a thread built with `std::thread::Builder::new().stack_size(2 * 1024 * 1024)` and joins it (a panic or abort in `f` fails or kills the test). Helper `fn nested_messages(depth: usize) -> String` returns `syntax = "proto3";\n` followed by `depth` nested message openers (level i is the text `message M<i> { `, for i from 0 to depth-1), then `string s = 1;`, then `depth` closing braces `}`. Helper `fn write(dir: &Path, name: &str, text: &str) -> PathBuf`.
2. Each test uses `tempfile::tempdir()` and calls `camel_proto_compiler::compile_proto(path, [dir])` inside `on_small_stack`; assertion helper `assert_compile_err(r)` checks `matches!(r, Err(ProtoCompileError::Compile { .. }))`.

**Tests:**
- `message_nesting_100_is_typed_error` and `message_nesting_10000_is_typed_error`: `nested_messages(100)` and `nested_messages(10_000)` → `Compile`.
- `angle_bracket_option_nesting_is_typed_error`: file text `"syntax = \"proto3\";\noption (r) = { f ".to_string() + &"< f ".repeat(10_000) + &"> ".repeat(10_000) + "};\n"` (brace depth 1, angle depth 10 000) → `Compile` (the guard rejects it before parsing; assert `detail.contains("nesting depth")`).
- `square_bracket_nesting_is_typed_error`: same shape with `[` and `]` repeated 10 000 → `Compile` with `detail.contains("nesting depth")`.
- `unterminated_string_cannot_hide_nesting`: text `syntax = "proto3";\noption x = "abc\n` + `"{".repeat(10_000)` + `"\n"` → `Compile` (assert `detail.contains("nesting depth")`).
- `escaped_quote_cannot_hide_nesting`: text `syntax = "proto3";\noption x = "\"" ` + `"{".repeat(10_000)` + `"\n"` → `Compile` with `detail.contains("nesting depth")`.
- `deep_import_is_typed_error_and_names_the_import`: `top.proto` = `syntax = "proto3";\nimport "deep.proto";\nmessage T { string a = 1; }\n`; `deep.proto` = `nested_messages(10_000)`; `compile_proto(top, [dir])` → `Compile` whose `detail` contains `deep.proto` and `nesting depth`.
- `deep_nesting_inside_comment_is_accepted`: file with `// ` + 10 000 `{` on one comment line plus a valid `message Ok { string a = 1; }` → `Ok`, pool has `Ok`.
- `hostile_descriptor_set_is_typed_error`: build `h.binpb` as in Task 1.4's `hostile_option_text_is_typed_error` for the 3 aggregate values `"f < ".repeat(10_000)`, `"f { ".repeat(10_000)`, `"f < # >\n".repeat(10_000)`; each → `Err(ProtoCompileError::DescriptorDecode(s))` with `s.contains("option text nesting depth")`.
- Command: `cargo test -p camel-proto-compiler --test hostile` (wrapper, log `t2.3`). Expected: pass (guard exists since Tasks 1.2-1.4).

**Acceptance:**
- `cargo test -p camel-proto-compiler --test hostile` passes, 9 tests, with no stack overflow and no `panicked at` line from protox in `/home/shared/tmp/347/t2.3.log`.
- clippy `--all-targets -D warnings` and `cargo fmt --all -- --check` exit 0.

- [x] 2.3

### Task 2.4: Error-position, editions and descriptor-set file tests

Spec scenarios: "A syntax error carries line and column"; "Editions source fails with guidance";
"A descriptor set loads"; "An editions descriptor set fails with a typed error";
"A corrupt descriptor set fails with a typed error".

**Files:**
- `crates/services/camel-proto-compiler/tests/errors_and_descriptor_sets.rs` (new)

**Steps:**
1. Create the test file with `fn manifest()` and `fn fixture(rel) -> PathBuf` rooted at `tests/fixtures`. Error fixtures are `err/*.proto`, copied from the e_opus harness; the include dir for each is its own parent directory.

**Tests:**
- `syntax_error_has_line_and_column`: `compile_proto(fixture("err/syntax.proto"), std::iter::empty::<&Path>())` → `Err(Compile { detail, .. })`; assert `detail.contains("syntax.proto:2:29")`.
- `duplicate_field_number_has_position`: `err/dup_number.proto` → `Compile`, `detail.contains("dup_number.proto:2:38")` and `detail.contains("already used")`.
- `missing_import_is_compile_error`: `err/missing_import.proto` → `Compile`, `detail.contains("nope/missing.proto")`.
- `undefined_type_is_compile_error`: `err/undefined_type.proto` → `Compile`, `detail.contains("undefined_type.proto:2:")`.
- `proto3_default_and_enum_zero_are_rejected`: `err/proto3_default.proto` and `err/proto3_enum_zero.proto` each → `Compile`.
- `compile_error_display_names_the_path`: the `syntax.proto` error's `to_string()` starts with `failed to compile ` and contains `syntax.proto`.
- `editions_source_error_names_remedy`: `fixture("ks/main/ed2023.proto")` → `Compile`, `detail.contains("editions")` and `detail.contains("proto3")`.
- `editions_descriptor_set_is_typed_error`: `fixture("golden/ed2023.binpb")` (its file has `syntax = "editions"`) → `Err(DescriptorDecode(s))`, `s.contains("editions")` and `s.contains("ed2023.binpb")`.
- `precompiled_kitchen_descriptor_set_loads_without_includes`: `fixture("golden/kitchen.binpb")` → `Ok`; pool has `kitchen.v1.Order` and service `kitchen.v1.OrderService`.
- `descriptor_set_extension_is_case_insensitive`: copy `golden/helloworld.binpb` to `HELLO.BINPB` and `hello.PB` and `hello.desc` and `hello.protoset` in a tempdir; each loads and has `helloworld.HelloRequest`.
- `descriptor_set_works_through_proto_cache`: `ProtoCache::new()`; `get_or_compile(fixture("golden/helloworld.binpb"), std::iter::empty::<&Path>())` twice; `cache.len() == 1`; pool has `helloworld.Greeter`.
- `corrupt_descriptor_set_names_the_path`: tempdir file `bad.binpb` with bytes `[0xff; 5]` → `Err(DescriptorDecode(s))`, `s.contains("bad.binpb")`.
- `missing_descriptor_set_is_not_found`: path `.../nope.binpb` → `Err(ProtoNotFound(_))`.
- `descriptor_set_without_imports_is_decode_error`: take `golden/kitchen.binpb`, decode to `FileDescriptorSet`, keep only the file named `kitchen.proto`, re-encode into tempdir `partial.binpb` → `Err(DescriptorDecode(_))`.
- Command: `cargo test -p camel-proto-compiler --test errors_and_descriptor_sets` (wrapper, log `t2.4`).

**Acceptance:**
- `cargo test -p camel-proto-compiler --test errors_and_descriptor_sets` passes, 14 tests.
- If an exact position string differs from protox output, report `position-drift: <actual>`; do not loosen below `file:line` unless the conductor approves.
- clippy `--all-targets -D warnings` and `cargo fmt --all -- --check` exit 0.

- [x] 2.4

## Downstream and workspace

### Task 3.1: Downstream crates, README sweep of test-side PROTOC use, golden deptree

Spec: proposal acceptance (downstream tests green) and design (`default-deptree.txt`).

**Files:**
- `crates/camel-cli/tests/fixtures/default-deptree.txt` (modified, regenerated)
- `crates/services/camel-proto-compiler/tests/helloworld.desc` (kept; used by Task 1.4)

**Steps:**
1. Run, through the wrapper with log names `t3.1a`..`t3.1d`: `cargo test -p camel-dataformat-protobuf --no-fail-fast`; `cargo test -p camel-component-grpc --no-fail-fast`; `cargo test -p camel-dsl --features protobuf --no-fail-fast`; `cargo test -p camel-cli --test compile_asset_test --no-fail-fast`. Record pass/fail counts. Tests that need Docker or native bridges and are already `#[ignore]`d stay ignored.
2. Regenerate the golden: `CARGO_TERM_COLOR=never cargo tree -p camel-cli -e features,no-dev --prefix none --locked | sed -E 's| \(/[^)]*\)||g; s| \(\*\)||g; s| \[\*\]||g' | LC_ALL=C sort -u > crates/camel-cli/tests/fixtures/default-deptree.txt` (run from the worktree root; wrapper only for cargo; this command does not compile).
3. Run `git diff crates/camel-cli/tests/fixtures/default-deptree.txt | rg '^[+-][^+-]'` and confirm every changed line names a package in the closure of `protox`: compute the allowed set with `cargo tree -p protox -e normal,build --prefix none --format '{p}'` (wrapper, log `t3.1f`; take the package names) plus feature lines of packages that are already in the golden (for example `prost-reflect feature "miette"`, `feature "text-format"`). A removed `protoc-bin-vendored*` line is NOT expected (build scripts still use it). Any package outside that set => STOP and report `deptree-creep: <line>`.
4. Run `cargo test -p camel-cli --test feature_profiles` (wrapper, log `t3.1e`).

**Tests:**
- `default_closure_matches_golden` (existing, `feature_profiles.rs`): after step 2 → passes. Before step 2 it fails (the new crates are missing from the fixture).
- Downstream suites from step 1 pass unchanged.

**Acceptance:**
- Step 1 suites: zero failures (counts recorded in the report).
- `cargo test -p camel-cli --test feature_profiles` exits 0.
- `git diff --stat` shows `default-deptree.txt` changed and no other file.
- `rg -n 'set_var\("PROTOC"' crates --glob '*.rs' --glob '!**/build.rs'` returns no hits.

- [x] 3.1

## Docs and decision record

### Task 4.1: Fix docs, crate README and comments

Spec: proposal "What Changes" docs bullet; mission item 4.

**Files:**
- `docs/src/data-formats/protobuf.md` (modified)
- `docs/src/components/grpc.md` (modified)
- `crates/services/camel-proto-compiler/README.md` (modified)

**Steps:**
1. `docs/src/data-formats/protobuf.md`: in line 3 delete the sentence "A `protoc` binary must exist at runtime." and say the compiler is built in (pure Rust, no external tool). Replace the whole `## Protoc resolution` section (the `PROTOC` list, the false "embedded in standard builds" claim and the `export PROTOC` block) with a `## Compilation` section: compilation is in-process, no `protoc`, no `PROTOC`, no temporary files, works on any platform and in a `FROM scratch` image. Add `## Precompiled descriptor sets`: any path that accepts a `.proto` also accepts a `FileDescriptorSet` file with extension `.binpb`, `.pb`, `.desc` or `.protoset` (chosen by extension, includes ignored, imports must be inside the set), with the producing commands `protoc --include_imports --descriptor_set_out=schema.binpb schema.proto` and `buf build -o schema.binpb`. Add `## Limits`: protobuf editions (sources and descriptor sets) are rejected with a message that says to rewrite the schema as `proto3` or `proto2` (prost-reflect 0.16 cannot load editions); bracket nesting deeper than 64 is rejected; errors report `file:line:column`. Keep each sentence short (ASD-STE100 style; load the `ste-writing` skill and apply it to the new prose).
2. `docs/src/components/grpc.md`: line 60 table row: `protoFile` description becomes "Path to the `.proto` file or to a precompiled descriptor set (`.binpb`, `.pb`, `.desc`, `.protoset`) for runtime descriptor resolution". Add one sentence to the intro or the options section: no `protoc` is needed at runtime.
3. `crates/services/camel-proto-compiler/README.md`: rewrite Overview/Features/Known limitation: in-process protox compilation; descriptor-set input; typed `ProtoCompileError` (`ProtoNotFound`, `Io`, `Compile`, `DescriptorDecode`, `#[non_exhaustive]`); cache bullets unchanged; delete the `ProtocUnavailable` paragraph and the `std::env::temp_dir()` / `rc-gr8k` paragraph (no temporary files exist any more); add a `Known limitations` list: editions sources, nesting limit 64, `ProtoCache` does not invalidate when an imported file changes (`rc-me2ii`). Add a `Migration` note: `PROTOC` is ignored; `ProtocUnavailable`/`ProtocFailed` are gone.
4. Run `rg -n '\bPROTOC\b|\bprotoc\b|vendored' docs/src crates/services/camel-proto-compiler crates/dataformats crates/components/camel-component-grpc/README.md crates/components/camel-component-grpc/CONTEXT.md examples --glob '!**/build.rs' --glob '!**/Cargo.toml'` and fix every statement that says `protoc` is needed or embedded at runtime. Allowed remaining mentions: the `protoc --include_imports --descriptor_set_out` producer command, the Migration note, and `docs/src/services/bridge.md` mentions about unrelated bridge protocols (leave untouched if they concern the bridge).

**Tests:**
- `docs-build`: `mdbook` is not required; the check is textual: `rg -n 'embedded in standard builds|must exist at runtime|Set PROTOC|Protoc resolution' docs crates --glob '*.md'` returns no hits.
- `lint-context-citations` stays green: `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- The `rg` command in Tests returns no hits.
- `cargo xtask lint-context-citations` and `cargo xtask lint-single-source` exit 0.
- New prose passes a manual STE check: no sentence over 25 words in the added text.

- [x] 4.1

### Task 4.2: ADR-0084 and CONTEXT-MAP entry

Spec: mission item 6.

**Files:**
- `docs/adr/0084-hermetic-proto-compilation.md` (new)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Create `docs/adr/0084-hermetic-proto-compilation.md` in English, ASD-STE100 style, short (under 70 lines), same header format as `docs/adr/0083-artifact-signing-envelope.md` (`# ADR-0084: ...`, `- Status: Accepted (decided 2026-10-06; bd rc-15md7)`, `- Source: bd rc-15md7; openspec change protox-compiler`, `- Amends: ADR-0075 intent (self-contained artifact)`). Sections: **Context** (the baked `CARGO_MANIFEST_DIR` path; Docker images 0.51 to 0.56 cannot run `protobuf:` or a `grpc://` route with `protoFile=`; `FROM scratch` has no `/tmp`; docs claimed "embedded"); **Decision** (compile `.proto` in-process with `protox` 0.9.1 pinned `=0.9.1`; pool built from the encoded descriptor set, never `Compiler::descriptor_pool()`; nesting guard 64 and `catch_unwind`; descriptor-set input by extension; `PROTOC` ignored; `ProtoCompileError` breaking change; owner premise: no built-in runtime feature needs an executable outside the `camel` binary and build scripts need only the Rust toolchain, with `exec`, `containers` and the Java bridges as explicit opt-in exceptions); **Rejected options** (B embed and extract protoc: needs a writable executable directory, +9 to 14 MB per target, 8 targets only; C docs-only: keeps the defect; A with a `PROTOC` override: generic variable hides dev/prod divergence); **Consequences** (editions are unsupported, in sources and in descriptor sets, until prost-reflect supports them: bd `rc-sed5l` tracks upstream; error texts follow protox; build-time `build.rs` migration is bd `rc-x2rlm`; `rc-me2ii` and the sealed-artifact temp directory bd `rc-joask` stay open; `camel-bridge` downloads `rc-grvqh` need their own decision).
2. `CONTEXT-MAP.md`: add a bullet for `[0084](./docs/adr/0084-hermetic-proto-compilation.md)` directly after the `[0083]` bullet (line ~115), one sentence, same style, naming `(camel-proto-compiler)`.

**Tests:**
- `adr-listed`: `rg -n '0084' CONTEXT-MAP.md docs/adr/0084-hermetic-proto-compilation.md` shows both files.
- `lint-context-citations`: `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- `docs/adr/0084-hermetic-proto-compilation.md` exists, `wc -l` under 70, contains the strings `protox`, `FROM scratch`, `Rejected options`.
- `cargo xtask lint-context-citations` exits 0.
- `rg -c 'rc-sed5l|rc-x2rlm|rc-joask|rc-me2ii|rc-grvqh' docs/adr/0084-hermetic-proto-compilation.md` matches, and each of the five ids appears at least once (check with five separate `rg -q` calls, all exit 0).

- [x] 4.2


## Post-landing-gate hardening (e_gpt review 2026-10-08)

Scope: resolve REJECT findings C1 (descriptor-set public-import cycle recursion),
C2 (deep import chains), I1 (scan/parse TOCTOU + non-regular files), I2 (byte
budgets), I3 (Windows compile of hermetic.rs), minors (parity regen header,
docs line:column qualifier). After these, no input path can abort the process.

### Task 5.1: Compiler hardening — read-once resolver, import budget, byte caps, cycle preflight

Spec: "Malformed proto or descriptor-set input never terminates the process"
(strengthened: import-graph bound, cycle rejection, byte budget, file-kind
rejection, validated-buffer identity). Design updates parallel this task.

**Files:**
- `crates/services/camel-proto-compiler/src/compiler.rs` (modified)
- `crates/services/camel-proto-compiler/src/lib.rs` (modified: capped `hash_proto_content`)
- `crates/services/camel-proto-compiler/src/nesting.rs` (modified: none expected; limits live in compiler.rs)

**Steps:**
1. Add constants and a shared budget near the top of `compiler.rs`:

   ```rust
   /// Maximum size of one schema input (source file or descriptor set).
   pub(crate) const MAX_SCHEMA_BYTES: usize = 16 * 1024 * 1024;
   /// Maximum number of distinct include-resolved schema files per compile.
   pub(crate) const MAX_IMPORT_FILES: usize = 256;
   /// Maximum cumulative schema bytes opened per compile.
   pub(crate) const MAX_IMPORT_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

   /// Per-compile import budget shared by every include resolver.
   #[derive(Default)]
   struct ImportBudget {
       files: std::sync::atomic::AtomicUsize,
       bytes: std::sync::atomic::AtomicU64,
   }

   impl ImportBudget {
       /// Records one opened file of `len` bytes; errors when a limit is hit.
       fn record(&self, name: &str, len: usize) -> Result<(), protox::Error> {
           let files = self.files.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
           let bytes = self.bytes.fetch_add(len as u64, std::sync::atomic::Ordering::SeqCst) + len as u64;
           if files > MAX_IMPORT_FILES || bytes > MAX_IMPORT_TOTAL_BYTES {
               return Err(protox::Error::new(ImportBudgetExceeded { name: name.to_owned() }));
           }
           Ok(())
       }
   }
   ```

2. Add the new typed wrapper errors next to `NestingExceeded` (same
   Display-equals-Debug pattern so protox `Debug` relays the text):

   ```rust
   struct ImportBudgetExceeded { name: String }
   // Display: "{name}: import graph exceeds the limit of {MAX_IMPORT_FILES} files or {MAX_IMPORT_TOTAL_BYTES} total bytes"
   struct UnsupportedFileKind { name: String }
   // Display: "{name}: not a regular file"
   struct SchemaTooLarge { name: String, len: u64 }
   // Display: "{name}: schema input of {len} bytes exceeds the limit of {MAX_SCHEMA_BYTES} bytes"
   struct ShadowedInput { name: String, expected: PathBuf, found: PathBuf }
   // Display: "path '{expected}' is shadowed by '{found}' in the include paths" (name in Debug too)
   ```

3. Rewrite `NestingGuardedInclude` so the SAME opened file descriptor is
   fstat-ed, read, scanned and parsed (no second read, no `inner` field; drop
   the `IncludeFileResolver` import):

   ```rust
   struct NestingGuardedInclude {
       dir: PathBuf,
       budget: std::sync::Arc<ImportBudget>,
       top: std::sync::Arc<std::sync::Mutex<Option<(String, PathBuf)>>>,
   }

   impl NestingGuardedInclude {
       fn new(dir: PathBuf, budget: std::sync::Arc<ImportBudget>,
              top: std::sync::Arc<std::sync::Mutex<Option<(String, PathBuf)>>>) -> Self {
       Self { dir, budget, top }
       }
   }

   impl FileResolver for NestingGuardedInclude {
       fn resolve_path(&self, path: &Path) -> Option<String> {
           let name = protox::file::IncludeFileResolver::new(self.dir.clone()).resolve_path(path)?;
           // Record the first resolving include for the shadow check in open_file.
           let mut guard = self.top.lock().unwrap_or_else(|p| p.into_inner());
           if guard.is_none() {
               *guard = Some((name.clone(), path.to_path_buf()));
           }
           Some(name)
       }

       fn open_file(&self, name: &str) -> Result<File, protox::Error> {
           use std::io::Read;
           let candidate = self.dir.join(name);
           let handle = match std::fs::File::open(&candidate) {
               Ok(h) => h,
               Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                   return Err(protox::Error::file_not_found(name));
               }
               Err(err) => return Err(protox::Error::new(OpenInclude { name: name.to_owned(), err })),
           };
           // Metadata of the open descriptor: no swap race between stat and read.
           let meta = handle.metadata().map_err(|err| protox::Error::new(OpenInclude { name: name.to_owned(), err }))?;
           if !meta.is_file() {
               return Err(protox::Error::new(UnsupportedFileKind { name: name.to_owned() }));
           }
           if meta.len() > MAX_SCHEMA_BYTES as u64 {
               return Err(protox::Error::new(SchemaTooLarge { name: name.to_owned(), len: meta.len() }));
           }
           let mut text = String::new();
           let mut limited = handle.take(MAX_SCHEMA_BYTES as u64 + 1);
           limited.read_to_string(&mut text).map_err(|err| protox::Error::new(OpenInclude { name: name.to_owned(), err }))?;
           if text.len() > MAX_SCHEMA_BYTES {
               return Err(protox::Error::new(SchemaTooLarge { name: name.to_owned(), len: text.len() as u64 }));
           }
           self.budget.record(name, text.len())?;
           if let Err(depth) = scan_nesting(text.as_bytes(), MAX_NESTING_DEPTH, ScanMode::ProtoSource) {
               return Err(protox::Error::new(NestingExceeded { name: name.to_owned(), depth }));
           }
           // Shadow replication (protox checks file.path(); File::from_source has none):
           if let Some((rec_name, rec_path)) = self.top.lock().unwrap_or_else(|p| p.into_inner()).clone()
               && rec_name == name
               && Path::new(&rec_path) != candidate.as_path()
           {
               return Err(protox::Error::new(ShadowedInput {
                   name: name.to_owned(),
                   expected: rec_path,
                   found: candidate,
               }));
           }
           // Parse the SAME validated buffer.
           File::from_source(name, &text)
       }
   }
   ```

   With `struct OpenInclude { name: String, err: std::io::Error }` (Display
   `"{name}: {err}"`). If let-chains fail on the shadow `if`, rewrite as a
   nested `if` — behavior identical. `File::from_source` keeps
   `source_code_info` behavior identical to `File::open` (both call
   `protox_parse::parse`), so import error positions are unchanged.

4. In `compile_source`, create one budget and one shadow record per compile and
   share them: `let budget = Arc::new(ImportBudget::default()); let top = Arc::new(Mutex::new(None));`
   then `resolver.add(NestingGuardedInclude::new(dir, Arc::clone(&budget), Arc::clone(&top)));`
   for each include dir. `GoogleFileResolver` stays unwrapped (embedded WKTs
   do not consume budget).

5. Add the descriptor-set import-cycle preflight (iterative, no recursion) in
   `compiler.rs` and call it in `load_descriptor_set` after the editions check
   and before `check_set_options`:

   ```rust
   /// Rejects cyclic dependency graphs before pool decoding. prost-reflect's
   /// public-dependency traversal recurses without a bound on cycles.
   fn check_import_cycles(set: &FileDescriptorSet) -> Result<(), String> {
       use std::collections::HashMap;
       let deps: HashMap<&str, Vec<&str>> = set.file.iter()
           .map(|f| (f.name.as_deref().unwrap_or(""), f.dependency.iter().map(String::as_str).collect()))
           .collect();
       // 0 = unvisited, 1 = on stack, 2 = done
       let mut state: HashMap<&str, u8> = deps.keys().map(|k| (*k, 0u8)).collect();
       for start in deps.keys() {
           if state[start] != &0 { continue; }
           let mut stack: Vec<(&str, usize)> = vec![(*start, 0)];
           let mut path: Vec<&str> = Vec::new();
           while let Some((node, idx)) = stack.pop() {
               if idx == 0 {
                   state.insert(node, 1);
                   path.push(node);
               }
               let node_deps = match deps.get(node) { Some(d) => d, None => { path.pop(); state.insert(node, 2); continue; } };
               if let Some(dep) = node_deps.get(idx) {
                   stack.push((node, idx + 1));
                   match state.get(*dep).copied().unwrap_or(0) {
                       0 => stack.push((dep, 0)),
                       1 => {
                           let mut cycle = path.clone();
                           cycle.push(dep);
                           return Err(cycle.join(" -> "));
                       }
                       _ => {}
                   }
               } else {
                   state.insert(node, 2);
                   path.pop();
               }
           }
       }
       Ok(())
   }
   ```
   (Verify this algorithm compiles and terminates on: self-import, two-file
   cycle, diamond, chain. Fix the stack bookkeeping if the pop/push order is
   wrong — the required property is: detect any back-edge to an on-stack node,
   never recurse.) Map failure to `DescriptorDecode(format!("{}: cyclic import graph: {cycle}", path.display()))`.

6. Cap `load_descriptor_set` reads: replace `std::fs::read(path)?` with the
   same open/fstat/is_file/len/take/read pattern; non-regular →
   `DescriptorDecode(format!("{}: not a regular file", ..))`, oversized →
   `DescriptorDecode(format!("{}: schema input of {len} bytes exceeds the limit of {MAX_SCHEMA_BYTES} bytes", ..))`,
   io errors keep the `Io` mapping (`?`).

7. In `lib.rs`, cap `hash_proto_content` identically (open, fd metadata,
   is_file, len check, `take(MAX_SCHEMA_BYTES + 1)`, read, re-check length);
   size/kind failures return
   `ProtoCompileError::Compile { path, detail }` with the same texts. This
   protects `ProtoCache` entry points.

8. Unit tests in `compiler.rs` tests module:
   - `resolver_reads_and_parses_same_buffer`: write `ok.proto` (`syntax = "proto3"; message Ok { string a = 1; }`) in a tempdir; build the wrapper with fresh Arcs; `open_file("ok.proto")` → `Ok(file)`; assert `file.source() == Some(<text>)` and `file.file_descriptor_proto().name == Some("ok.proto")`.
   - `resolver_rejects_directory_include`: `std::fs::create_dir(dir.join("x.proto"))`; `open_file("x.proto")` → Err whose Display contains "not a regular file".
   - `resolver_shadow_check_replicates_protox`: tempdir with `a/dup.proto` (message A) and `b/dup.proto` (message B), different content; wrapper for `a` and wrapper for `b` share `top`; call `b.resolve_path(Path::new("<tmp>/b/dup.proto"))` → Some("dup.proto"); then `a.open_file("dup.proto")` → Err Display contains "shadowed by" and the `a` path.
   - `import_budget_records_and_rejects`: fresh budget; `record` Ok 256 times; 257th → Err Display contains "import graph exceeds".
   - `cycle_check_detects_self_and_pair`: hand-built sets (x→x public; a→b, b→a) → Err containing "cyclic import graph" and the names; diamond+chain set → Ok.
   - `oversized_include_is_rejected_before_read`: in a tempdir create a file, `set_len(2 << 30)` (sparse; do NOT write 2 GiB), `open_file` → Err Display contains "exceeds the limit".

**Acceptance:**
- `cargo test -j4 -p camel-proto-compiler --lib` passes (all earlier + 6 new).
- `rg -n 'inner.open_file|IncludeFileResolver::new\(self.dir' src/compiler.rs` shows `IncludeFileResolver` used only inside `resolve_path` delegation (no second read/parse).
- `cargo clippy -j4 -p camel-proto-compiler --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `cargo xtask lint-unwrap` exit 0.

- [x] 5.1

### Task 5.2: Hostile-input regression tests, Windows portability, minors

Spec: same requirement; adds the scenarios the gate demanded.

**Files:**
- `crates/services/camel-proto-compiler/tests/hostile.rs` (modified)
- `crates/services/camel-proto-compiler/tests/hermetic.rs` (modified)
- `crates/services/camel-proto-compiler/tests/parity.rs` (modified: header comment only)
- `docs/src/data-formats/protobuf.md` (modified: one sentence)

**Steps:**
1. `hostile.rs` new tests (use the existing `on_small_stack`, `write` helpers):
   - `import_chain_over_limit_is_typed_error`: 300-file chain in a tempdir (`f0.proto` imports `f1.proto`, ..., `f298.proto` imports `f299.proto`, last file has `message End { string a = 1; }`; every file starts `syntax = "proto3";`); `compile_proto(f0, [dir])` on a 2 MiB thread → `Err(Compile { detail, .. })`, `detail.contains("import graph exceeds")`; no abort (test completes).
   - `import_chain_under_limit_compiles`: 200-file chain ending in `message End`; → Ok; pool has the final message (fully qualified name `End` or package if given — use package `chain;` in every file and assert `chain.End`).
   - `descriptor_self_import_cycle_is_typed_error`: `FileDescriptorSet` with one file `name: Some("x.proto")`, `syntax: Some("proto3")`, `dependency: vec!["x.proto"]`, `public_dependency: vec![0]`; encode to `x.binpb`; compile on 2 MiB thread → `Err(DescriptorDecode(s))`, `s.contains("cyclic import graph")`.
   - `two_file_public_import_cycle_is_typed_error`: `a.proto` deps `b.proto`, `b.proto` deps `a.proto`, both `public_dependency: vec![0]` → `DescriptorDecode` containing "cyclic import graph" and both names.
   - `oversized_source_is_typed_error`: tempdir file `big.proto`, `set_len(2 << 30)` (sparse, no data write); `compile_proto` → `Err(Compile { detail, .. })` or `Io`, assert the Display contains "exceeds the limit"; also `ProtoCache::new().get_or_compile(big, [])` → Err with "exceeds the limit" (hash path capped).
   - `oversized_descriptor_set_is_typed_error`: `big.binpb` with `set_len(2 << 30)` → `Err(DescriptorDecode(s))`, `s.contains("exceeds the limit")`.
2. `hermetic.rs`: move `SPAWN_LOCK`, the marker-script writing helper, and `env_override_cannot_bring_back_external_compiler` under `#[cfg(unix)]` (the `use std::os::unix::fs::PermissionsExt` import moves inside the cfg'd test or helper). All other tests stay portable (they must still compile on Windows: no Unix-only APIs outside the cfg).
3. `parity.rs`: replace the header comment (top of file) with regeneration commands for the goldens:

   ```text
   // Goldens in tests/fixtures/golden were produced ONCE by protoc 31.1
   // (protoc-bin-vendored 3.2.0):
   //   PROTOC=~/.cargo/registry/src/*/protoc-bin-vendored-linux-x86_64-3.2.0/bin/protoc
   //   $PROTOC --include_imports --descriptor_set_out=golden/kitchen.binpb -I ks/lib -I ks/main ks/main/kitchen.proto
   //   $PROTOC --include_imports --descriptor_set_out=golden/ed2023.binpb -I ks/main ks/main/ed2023.proto
   //   $PROTOC --include_imports --descriptor_set_out=golden/helloworld.binpb -I .. ../helloworld.proto
   // Regenerate only when a fixture changes; never regenerate from protox output.
   ```

4. `docs/src/data-formats/protobuf.md` `## Limits`: qualify the position claim — "Syntax and semantic errors in `.proto` sources report the position as `file:line:column`. I/O and descriptor-decode errors do not carry a position."

**Tests:** the six new hostile tests + the existing suite.

**Acceptance:**
- `cargo test -j4 -p camel-proto-compiler` (all binaries) passes; hostile.rs has the 6 new tests; no abort anywhere in the log.
- `cargo check -j4 -p camel-proto-compiler --all-targets` (same tree) exits 0 and `rg -n 'cfg\(unix\)' tests/hermetic.rs` brackets the script machinery.
- clippy `-D warnings`, `cargo fmt --all -- --check`, `cargo xtask lint-test-sleep`, `cargo xtask lint-unbounded-wait` exit 0.

- [x] 5.2

### Task 5.3: Gate re-run (conductor)

Re-run after 5.1+5.2: fmt --check --all; clippy set for touched crates (workspace set + camel-proto-compiler); lint-unwrap, lint-test-sleep, lint-unbounded-wait, lint-log-levels, lint-context-citations; `cargo test -p camel-proto-compiler`, `-p camel-dataformat-protobuf`, `-p camel-component-grpc`, `-p camel-dsl --features protobuf`, `-p camel-cli --test compile_asset_test --test feature_profiles`; `cargo xtask changelog --check --from main --to HEAD`. cargo audit unchanged (no dependency change) — recorded as skipped-with-reason.

- [x] 5.3


### Task 5.4: Review fixes — nonblocking opens, Windows directory handles, test extraction

r_gpt review of 5.1/5.2 (round 2): findings 1 (FIFO blocking open), 2
(Windows directory open), 4 (1040-line compiler.rs).

**Files:**
- `crates/services/camel-proto-compiler/src/compiler.rs` (modified)
- `crates/services/camel-proto-compiler/src/compiler_tests.rs` (new: the unit tests moved out of compiler.rs)

**Steps:**
1. Add a shared opener used by BOTH `NestingGuardedInclude::open_file` and `read_schema_bytes` (descriptor set + hash paths):

   ```rust
   /// Opens a schema input for validation. On Unix the open is
   /// nonblocking so a writerless FIFO cannot hang before the
   /// regular-file check; for regular files the flag is a no-op.
   fn open_schema_input(path: &Path) -> std::io::Result<std::fs::File> {
       std::fs::OpenOptions::new().read(true).custom_flags(SCHEMA_OPEN_FLAGS).open(path)
   }
   ```

   With platform constants:

   ```rust
   #[cfg(unix)]
   /// O_NONBLOCK: 0o4000 on every supported Unix (Linux, macOS, BSDs, Solaris).
   const SCHEMA_OPEN_FLAGS: i32 = 0o4000;
   #[cfg(windows)]
   /// FILE_FLAG_BACKUP_SEMANTICS: lets the handle open a directory so the
   /// descriptor metadata (not the OS error) decides the rejection.
   const SCHEMA_OPEN_FLAGS: u32 = 0x0200_0000;
   #[cfg(not(any(unix, windows)))]
   const SCHEMA_OPEN_FLAGS: u32 = 0;
   ```

   Wire `use std::os::unix::fs::OpenOptionsExt` / `use std::os::windows::fs::OpenOptionsExt`
   behind the matching cfg (an empty `custom_flags` call is not available on
   other platforms — cfg the whole call there and fall back to `File::open`).
   Replace every `std::fs::File::open(&candidate)` / `File::open(path)` in the
   schema paths with `open_schema_input`.
2. Move the entire `#[cfg(test)] mod tests` of `compiler.rs` into a new sibling
   file `src/compiler_tests.rs`, mounted from `compiler.rs` with
   `#[cfg(test)] #[path = "compiler_tests.rs"] mod tests;` (keeps private-item
   access through `super::`). No test-content changes in this step.
3. New tests (in `compiler_tests.rs`, cfg-gated where needed):
   - `fifo_include_is_typed_error_without_hanging` (`#[cfg(unix)]`): tempdir;
     create a FIFO with `std::process::Command::new("mkfifo").arg(path)`; a
     proto importing that name; `compile_proto` → typed `Compile` error whose
     detail contains "not a regular file". The test returning at all proves
     the open did not block.
   - `fifo_descriptor_set_is_typed_error` (`#[cfg(unix)]`): the FIFO path with
     a `.binpb` extension passed to `compile_proto` → `DescriptorDecode`
     containing "not a regular file".
   - `directory_source_is_typed_error` (portable): a `.proto`-suffixed
     directory as the top-level input → typed error (Compile or DescriptorDecode
     path text containing "not a regular file"); asserts the Windows
     backup-semantics opener yields the metadata rejection, not an OS error.
4. Confirm `cargo test -j4 -p camel-proto-compiler` all green (expect 84) and
   `wc -l src/compiler.rs` drops by the moved test block.

**Acceptance:**
- All crate tests pass incl. the 3 new ones; no hang (fleet timeout not hit).
- `rg -n 'std::fs::File::open' src/compiler.rs src/lib.rs` shows no remaining direct open in schema paths.
- clippy `-D warnings`, fmt --check, lint-unwrap, lint-test-sleep exit 0.
- `src/compiler_tests.rs` exists; compiler.rs non-test code is unchanged apart from the opener swap.

- [x] 5.4


## Landing-gate round 2 (e_gpt delta-verify 2026-10-08)

Remaining findings: CRITICAL acyclic descriptor-graph recursion (prost-reflect
`resolve_public_dependencies` re-walks already-seen public deps without
memoization: stack depth = longest chain, call count = exponential on layered
DAGs; the 16 MiB cap does not bound it), IMPORTANT symlink policy implicit,
MINOR public docs for the new limits. Ledger:

### Task 6.1: Descriptor-graph chain and expansion bounds before pool decode

Spec: "Malformed proto or descriptor-set input never terminates the process"
(descriptor-graph bounds; subsumes the cyclic check which stays).

**Files:**
- `crates/services/camel-proto-compiler/src/compiler.rs` (modified)
- `crates/services/camel-proto-compiler/src/compiler_tests.rs` (modified)

**Steps:**
1. Add constants next to the other limits:

   ```rust
   /// Maximum longest-chain length in a descriptor-set dependency graph.
   /// prost-reflect resolves public imports with unbounded recursion whose
   /// stack depth equals this chain length.
   pub(crate) const MAX_DESCRIPTOR_CHAIN: usize = 256;
   /// Worst-case number of public-dependency resolution steps tolerated in a
   /// descriptor set. Diamond-rich graphs expand exponentially because
   /// already-seen dependencies are re-walked; this bound rejects them
   /// before pool building.
   pub(crate) const MAX_DESCRIPTOR_EXPANSION: u64 = 100_000;
   ```

2. Add an iterative (Kahn topological) preflight — NO recursion in the
   preflight itself — and call it in `load_descriptor_set` right after
   `check_import_cycles`:

   ```rust
   /// Bounds the descriptor dependency graph before pool building:
   /// the longest dependency chain (prost-reflect public-import recursion
   /// depth) and the worst-case expansion of the un-memoized public-import
   /// walk (exponential on diamond-rich DAGs). Cycles are rejected first by
   /// `check_import_cycles`, so the topological walk terminates.
   fn check_descriptor_graph(set: &FileDescriptorSet) -> Result<(), String> {
       use std::collections::HashMap;
       let names: HashMap<&str, usize> = set.file.iter().enumerate()
           .filter_map(|(i, f)| f.name.as_deref().map(|n| (n, i)))
           .collect();
       let n = set.file.len();
       // dependency[i][k] = Some(index) for the k-th dependency of file i
       // (public_dependency positions index into this ORIGINAL list).
       let deps_resolved: Vec<Vec<Option<usize>>> = set.file.iter().map(|f| {
           f.dependency.iter().map(|d| names.get(d.as_str()).copied()).collect()
       }).collect();
       let all_deps: Vec<Vec<usize>> = deps_resolved.iter()
           .map(|v| v.iter().copied().flatten().collect()).collect();
       let public_deps: Vec<Vec<usize>> = deps_resolved.iter().enumerate().map(|(i, v)| {
           set.file[i].public_dependency.iter()
               .filter_map(|&pi| v.get(pi as usize).and_then(|o| *o)).collect()
       }).collect();
       // Kahn topological order (in-degree over all_deps).
       let mut indegree = vec![0usize; n];
       for out in &all_deps { for &d in out { indegree[d] += 1; } }
       let mut queue: std::collections::VecDeque<usize> =
           (0..n).filter(|&i| indegree[i] == 0).collect();
       let mut order: Vec<usize> = Vec::with_capacity(n);
       while let Some(i) = queue.pop_front() {
           order.push(i);
           for &d in &all_deps[i] {
               indegree[d] -= 1;
               if indegree[d] == 0 { queue.push_back(d); }
           }
       }
       if order.len() != n {
           return Err("cyclic dependency graph".to_string());
       }
       // Longest chain and worst-case expansion, in topological order,
       // saturating so exponential values cannot overflow.
       let cap = MAX_DESCRIPTOR_EXPANSION;
       let mut chain = vec![1usize; n];
       let mut expand = vec![1u64; n];
       let mut total: u64 = 0;
       for &i in &order {
           for &d in &all_deps[i] { chain[i] = chain[i].max(chain[d] + 1); }
           for &d in &public_deps[i] {
               expand[i] = expand[i].saturating_add(expand[d]);
           }
           if chain[i] > MAX_DESCRIPTOR_CHAIN {
               return Err(format!("descriptor import chain exceeds the limit of {MAX_DESCRIPTOR_CHAIN} files"));
           }
           total = total.saturating_add(expand[i]);
           if total > cap {
               return Err(format!("descriptor public-import graph expands beyond the resolution budget of {cap} steps"));
           }
       }
       Ok(())
   }
   ```

   Map failure to
   `DescriptorDecode(format!("{}: {msg}", path.display()))` (same shape as
   the cycle error). Verify the public_dependency index mapping against
   `prost_types` (positions index the `dependency` vec) and the prost-reflect
   walk (`resolve.rs:422-430`: recursion only on already-seen deps, so
   `expand` is the exact worst-case call bound).

3. Unit tests in `compiler_tests.rs`:
   - `graph_chain_over_bound_rejected`: 300-file public-dep chain (f_i deps f_{i-1}, public_dependency [0]) → Err contains "import chain exceeds".
   - `graph_layered_dag_rejected_by_expansion`: 7 layers × 10 files; each file public-deps all 10 files of the previous layer; chain length 8 ≤ 256 but expansion ≥ 10^6 → Err contains "resolution budget".
   - `graph_leaf_first_revisit_rejected`: root with dependencies f_999..f_500 (500 deps), each f_i (i ≥ 1) public-deps f_{i-1} (chain 1001) → Err (chain message).
   - `graph_small_diamond_ok`: root → a, b; a, b → leaf (all public); leaf has a message; root has a field of the leaf type → Ok.
4. Integration tests in `tests/hostile.rs` (2 MiB thread, typed-error asserts,
   no abort):
   - `acyclic_layered_descriptor_set_is_typed_error`: encode the layered-DAG set to `layered.binpb`; `compile_proto` → `Err(DescriptorDecode(s))`, `s.contains("resolution budget")`.
   - `acyclic_chain_descriptor_set_is_typed_error`: the 300-chain set → `DescriptorDecode`, `s.contains("import chain exceeds")`.
   - `acyclic_descriptor_graph_under_bounds_loads`: the small-diamond set → Ok; `pool.get_message_by_name` finds the leaf message through the public imports.

**Acceptance:**
- `cargo test -j4 -p camel-proto-compiler` passes (84 + 7 new = 91).
- clippy `-D warnings`, fmt --check, lint-unwrap exit 0.

- [x] 6.1

### Task 6.2: Document symlink policy and the schema limits

e_gpt: the follow-symlinks behavior must be an explicit documented policy, not
an implicit leftover. Policy chosen (protoc parity): symlinks ARE followed;
safety comes from the single-read validated buffer (kind, cap, scan and parse
all operate on the one opened descriptor), so a link target can only ever be
validated content or a typed rejection.

**Files:**
- `docs/src/data-formats/protobuf.md` (modified)
- `crates/services/camel-proto-compiler/README.md` (modified)
- `openspec/changes/protox-compiler/design.md` (modified)

**Steps:**
1. `design.md` Approach step 3 (after the read-once sentences): add "Symbolic
   links are followed, matching protoc. The link target must resolve to a
   regular file through the open descriptor; every other kind, or a target
   above the byte cap, is a typed error. This is an explicit policy decision
   (ADR-0084 context), not an oversight: rejecting links would break
   protoc-compatible schema layouts, and the single-read validation makes
   following safe."
2. `docs/src/data-formats/protobuf.md` `## Limits`: extend to name every
   bound: nesting depth 64; import graph 256 files / 64 MiB cumulative; any
   single schema input 16 MiB; descriptor sets additionally bounded by import
   chain length 256 and public-import resolution budget 100 000 steps;
   symbolic links are followed (protoc behavior) and their target must be a
   regular file.
3. `README.md` `## Migration`: add one line that these behavioral limits are
   part of the breaking change (the `!` marker on the hardening commit is
   justified by them; the error enum already changed in the earlier breaking
   commit).

**Tests:**
- `rg -n 'symbolic link|16 MiB|resolution budget' docs/src/data-formats/protobuf.md crates/services/camel-proto-compiler/README.md` matches.
- `cargo xtask lint-context-citations`, `lint-single-source` exit 0.

**Acceptance:**
- All three files updated; STE style (short sentences); gates green.

- [x] 6.2

### Task 6.3: Gate re-run and park update (conductor)

Re-run: crate tests, fmt, clippy crate, changelog check; update
inbox/protox-parked.json with round-2 status and the `!` justification note.

- [x] 6.3

## Landing-gate round 3: traversal entry points and source snapshots

The previous graph budget was not an upper bound. Each direct dependency,
including private imports and repeated occurrences, starts a public-import
walk. Source compilation also builds internal pools before final decoding.

### Task 7.1: Charge every descriptor traversal entry point

**Files:**
- `crates/services/camel-proto-compiler/src/compiler.rs` (modified)
- `crates/services/camel-proto-compiler/src/compiler_tests.rs` (modified)
- `crates/services/camel-proto-compiler/tests/hostile.rs` (modified)

**Steps:**
1. Keep iterative cycle and chain guards. Compute E(v) = 1 + sum E(d)
   over every public dependency occurrence using saturating arithmetic.
2. Replace sum E(v) with W = file_count + sum over EVERY file and EVERY
   direct dependency occurrence d of E(d). Do not deduplicate entry points.
   Missing dependencies remain decoder errors; preserve original public
   dependency indexing. Reject W above 100000 before pool construction.
3. Return or expose W internally so source lifetime accounting can reuse it.
   This is an upper bound, not an exact call count; correct misleading comments.

**Tests (write first):**
- `private_import_roots_exceed_work_budget`: arrange 150 public-chain files
  and 1000 private roots, each importing the 150 chain names leaf-first;
  act on graph checker; assert resolution-budget rejection in both file orders.
- `private_import_roots_descriptor_set_is_typed_error`: encode the same set
  to a test-owned file; act compile_proto on a 2 MiB thread; assert
  DescriptorDecode containing resolution budget. Never decode this adversarial
  set before the guard exists.
- `leaf_first_descriptor_set_is_typed_error`: encode the existing 1000-file
  leaf-first helper pattern; act compile_proto; assert typed chain-limit error.
- Command: scoped bounded `cargo test -j4 -p camel-proto-compiler`.
  Expected: budget helper regression fails before implementation, all pass after.

**Acceptance:** corrected budget formula, new regressions pass; crate clippy
all-targets with -D warnings and fmt check pass. No unchecked pools in tests.

- [x] 7.1

### Task 7.2: Preflight source imports before protox builds internal pools

**Files:**
- `crates/services/camel-proto-compiler/src/compiler.rs` (modified)
- `crates/services/camel-proto-compiler/src/compiler_tests.rs` (modified)
- `crates/services/camel-proto-compiler/tests/hostile.rs` (modified)

**Steps:**
1. Inside contained_compile, iteratively preload the reachable root/import
   closure using the existing safe resolver (explicit includes, parent,
   Google fallback). Parse via File::from_source only; do not construct a pool.
   Preserve visiting/done states, declaration order, source text, custom
   options, root path mapping and shadow checks. Count filesystem reads with
   existing byte/file budgets; include embedded WKTs in graph validation.
2. Record missing imports in the immutable snapshot instead of reopening them.
   A snapshot FileResolver returns cloned cached Files or file_not_found;
   there is no filesystem or Google fallback after preflight. Compiler can
   then produce its canonical missing-import diagnostics from source locations.
3. Check cycles, chain and W from 7.1 before Compiler construction. Source
   lifetime bound = saturating (file_count + 1) * W, limit 100000. This covers
   at most N internal file additions plus final pool decode, even if each
   rebuild resolved the full prefix. Reject with Compile and path/detail.
4. Construct Compiler::with_file_resolver(snapshot), preserve existing flags,
   encoded-descriptor pool decoding and catch_unwind. Never reopen source
   files between graph validation and pool building.

**Tests (write first):**
- `layered_public_import_source_graph_is_typed_error`: arrange 70 width-two
  public-import layers plus a private root importing all layers leaf-first
  (141 files, below source caps); compile on 2 MiB thread; assert Compile
  resolution-budget rejection before any internal pool construction.
- `source_lifetime_budget_is_typed_error`: arrange 10 width-two layers plus
  root; final graph W below 100000 but (N+1)W above; assert typed budget error.
- `source_snapshot_survives_file_replacement`: preload valid sources, replace
  or remove original paths, compile cached snapshot; assert original messages
  resolve and replacement contents do not. Use an internal seam, no races.
- Preserve existing 200-file private-chain, kitchen-sink custom-option parity,
  WKT, missing-import location, shadow and relative/empty-parent regressions.
- Command: scoped bounded `cargo test -j4 -p camel-proto-compiler`.
  Expected: graph helper/lifetime rejection regression red first; malicious
  source graph must never be sent into unguarded Compiler during red testing.

**Acceptance:** all pool entry points have an immutable guarded graph; tests,
crate clippy all-targets -D warnings, fmt, lint-unwrap/test-sleep pass.

- [x] 7.2

### Task 7.3: Round-three contract, review and verified park

**Files:**
- `docs/src/data-formats/protobuf.md` (modified)
- `crates/services/camel-proto-compiler/README.md` (modified)
- `openspec/changes/protox-compiler/design.md` (modified)
- `openspec/changes/protox-compiler/specs/data-formats/spec.md` (modified)
- `openspec/changes/protox-compiler/.review.json` (modified after review)
- `.opencode/fleet/inbox/protox-parked.json` (fleet handoff at absolute root path)

**Steps:**
1. Qualify 256-file/64-MiB limits as source-only; descriptor sets may contain
   more files within byte, chain and work bounds. Document source lifetime
   budget, snapshot architecture, and multiplicity-aware W; remove exact-count
   and unconditional no-abort assertions not supported by reviewed evidence.
2. Reviewer verifies both important findings and both minors; resolve all
   real findings before completion. Keep actual verdicts in .review.json.
3. Run affected crate tests, downstream protobuf suites, fmt, clippy and
   relevant lints using disk guards, containment and captured exit statuses.
4. Commit locally after status/diff/log inspection. Update parked JSON with
   new head, actual review verdict and round-three gates; keep editions,
   sealed-artifact and deptree notes. Do not merge, push or close bd.

**Tests:** source and descriptor regressions from 7.1/7.2; openspec validate
change returns valid; affected tests and gates exit zero. Review approval is
required separately from test success.

- [x] 7.3

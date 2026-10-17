# Tasks: build-protox

## Workspace protobuf generation

### Task 1.1: Replace the six vendored compiler build paths

**Files:**
- `Cargo.toml`, `Cargo.lock`.
- `crates/components/camel-component-grpc/{Cargo.toml,build.rs}`.
- `crates/components/camel-validator/{Cargo.toml,build.rs}`.
- `crates/components/camel-xslt/{Cargo.toml,build.rs}`.
- `crates/components/camel-jms/{Cargo.toml,build.rs}`.
- `crates/components/camel-cxf/{Cargo.toml,build.rs}`.
- `crates/camel-bench/{Cargo.toml,build.rs}`.

**Steps:**
1. Confirm the successful baseline build and seven saved Rust files under target/rc-x2rlm-evidence/before. Do not alter or regenerate that baseline after edits.
2. Replace each protoc-bin-vendored build dependency with protox.workspace = true. Remove the workspace protoc-bin-vendored declaration. Keep protox pinned at =0.9.1.
3. In each build.rs, compile the same proto files/include roots using protox::compile and pass descriptors to tonic_prost_build::configure().compile_fds. Return errors through main's Result. Preserve gRPC's execution-time CARGO_MANIFEST_DIR lookup and camel-bench's canonical schema location.
4. Explicitly print cargo:rerun-if-changed for every proto input and include directory. Preserve existing watches. Remove PROTOC mutation and obsolete vendored-compiler comments.
5. Refresh the lockfile without upgrading unrelated packages. Format the changed Rust.

**Tests:** (temporary verification functions stored only under target/rc-x2rlm-evidence)
- `verify_generated_rust_parity`: arrange baseline snapshots grouped by package and filename; build the six modified library targets with --message-format=json and use each build-script-executed out_dir to enumerate generated Rust; assert exactly six packages, exactly seven files, identical per-package filename sets and identical bytes for every file.
- `verify_no_protoc_invocation`: arrange an executable script which writes PROTOC_SENTINEL_MARKER then exits 97; set PROTOC to it and prepend a directory containing the same script named protoc to PATH; force all six build scripts to rerun and build affected all-targets; assert exit 0, all six build-script output files have a fresh mtime, and no marker exists.
- `verify_manifest_and_lock_removal`: parse workspace/all six manifests plus Cargo.lock; assert the six build dependencies use workspace protox, the workspace pin remains =0.9.1, and no protoc-bin-vendored package/dependency remains.

**Acceptance:** All three checks pass. No generated Rust API/byte change and no unrelated lockfile changes. Independent review has no Critical or Important findings before proceeding.

- [x] 1.1

## Documentation and validation

### Task 1.2: Verify schema tracking and finish the migration record

**Files:**
- `docs/adr/0084-hermetic-proto-compilation.md`.
- Evidence under ignored `target/rc-x2rlm-evidence/`; durable results in bd rc-x2rlm.

**Steps:**
1. Amend the ADR-0084 consequence paragraph to record completion of rc-x2rlm: all six build scripts use protox and Cargo.lock has no vendored protoc. Preserve the other independent follow-up issues.
2. Verify each build output's explicit input/include watches. For each distinct proto source, advance its mtime without changing bytes, rebuild affected library targets, and verify each owning script's output mtime advances; restore source timestamps afterward.
3. Run focused library tests and clippy --all-targets for all six packages; run cargo fmt --check --all and all applicable xtask lints, schema check, and cargo audit. Record outcomes and investigate failures before completion. No Cargo command runs in main.
4. Obtain independent review of the final diff, archive/sync the OpenSpec change, record evidence in bd and commit locally. Never push.

**Tests:**
- `verify_rebuild_watches`: arrange a successful migrated build and record each owning build output mtime; verify output text includes every proto/include path; touch each distinct input and rebuild; assert every affected owner's output mtime is newer and original source bytes remain unchanged, restoring timestamps afterward.
- `focused_library_tests`: execute cargo test --lib selecting camel-component-grpc, camel-component-validator, camel-xslt, camel-component-jms, camel-component-cxf and camel-bench; assert exit 0 and inspect ignored tests separately.
- `focused_clippy`: same package selection, cargo clippy --all-targets -- -D warnings; assert exit 0.

**Acceptance:** Schema watches are verified; focused tests/clippy, formatting and applicable gates pass or an independently verified pre-existing blocker is recorded as a linked issue. All Critical/Important review findings are resolved. ADR and bd reflect actual results, OpenSpec is archived and changes are committed locally.

- [x] 1.2

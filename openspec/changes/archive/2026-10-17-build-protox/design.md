# Design: build-protox

## Approach

Complete ADR-0084's build-time follow-up, bd rc-x2rlm. Each of the six existing build scripts will call workspace-pinned protox::compile with its existing source/include paths, then pass the FileDescriptorSet directly to tonic_prost_build::configure().compile_fds. Keep tonic/prost generation defaults and output directory behavior. Add protox.workspace = true to each build-dependency section and remove protoc-bin-vendored everywhere in workspace manifests and Cargo.lock. Avoid a shared helper crate: these scripts remain small and their source locations differ.

The gRPC build script must continue obtaining CARGO_MANIFEST_DIR at script execution time, so Cargo cache reuse between worktrees cannot bake in a deleted worktree path. camel-bench continues reading the canonical bridges/xml proto. Preserve existing explicit rerun-if-changed declarations; emit explicit proto/include watches where needed because compile_fds does not consume file paths itself. Do not add custom runtime compiler guards to trusted repository-owned proto sources.

## Affected crates

- camel-component-grpc: gRPC test server/client stubs.
- camel-component-validator and camel-xslt: XML bridge stubs.
- camel-component-jms and camel-component-cxf: bridge stubs.
- camel-bench: canonical XML bridge benchmark stubs.
- Workspace Cargo.toml and Cargo.lock: remove the vendored compiler family without unrelated dependency refreshes.

## Architecture boundaries

CONTEXT-MAP.md classifies these adapters under Components and the runtime compiler under Services. This change affects build-time code generation only: no changes to Runtime, DSL, language evaluation, wire contracts or public generated types. ADR-0084 already chooses protox; update its consequence paragraph to mark the build-time follow-up complete without creating another ADR.

## Verification

Before implementation, build the six library targets with the committed lockfile in the feature worktree. Save every generated Rust filename and bytes per package. After migration, regenerate all six targets and compare each package's complete generated Rust file set and bytes. Fail on missing, extra or different files; classify any difference before proceeding.

After removal, build the affected all-targets configurations with PROTOC pointing at an executable sentinel that records invocation and exits unsuccessfully. The build must succeed and the sentinel marker must remain absent. Check Cargo.lock for zero protoc-bin-vendored packages. Run focused library tests, formatting, clippy on affected targets, and applicable xtask gates. Keep logs and temporary snapshots under the ignored worktree target directory; record durable results in bd. No new permanent implementation-mirroring tests are needed for this build-only change.

## Alternatives considered

- Keep vendored protoc: retains the unsupported-platform build restriction.
- Depend on system protoc: requires another tool installation and host-dependent output.
- Commit generated Rust: duplicates schema-derived code and changes repository maintenance policy.
- Route build scripts through camel-proto-compiler: introduces unnecessary framework build dependencies and runtime guards; protox already provides descriptors directly.

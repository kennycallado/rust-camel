# Proposal: build-protox

## Why

ADR-0084 already uses protox for runtime descriptor compilation, but six build scripts still launch a platform-specific vendored protoc. This leaves the build dependent on the vendored target matrix. Complete bd rc-x2rlm using the workspace-pinned protox dependency. CONTEXT-MAP.md places the affected adapters in Components and the reusable runtime compiler in Services; this change concerns build tooling only.

## What Changes

- Generate descriptors in-process with protox and pass them to tonic-prost-build compile_fds in camel-component-grpc, camel-component-validator, camel-xslt, camel-component-jms, camel-component-cxf and camel-bench.
- Remove protoc-bin-vendored from those manifests, the workspace declaration and Cargo.lock.
- Preserve generated Rust, include paths, output locations and rebuild triggers. Update ADR-0084's build-time follow-up status.
- Exclude runtime compiler behavior, protobuf editions, Java bridge generation and unrelated dependency updates.

## Acceptance criteria

- All six code-generation outputs are byte-identical before and after migration.
- All affected targets compile without invoking protoc; focused tests, formatting, clippy and applicable repository lints pass.
- Cargo.lock has no protoc-bin-vendored package; no production build script sets PROTOC.

## Risk budget

Accept build-script implementation changes only. Generated-code or public API differences are outside scope and must be investigated rather than accepted silently. Reuse the pinned protox version and preserve gRPC's runtime CARGO_MANIFEST_DIR lookup. All Cargo builds run inside the feature worktree.

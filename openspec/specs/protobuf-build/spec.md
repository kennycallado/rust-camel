# protobuf-build Specification

## Purpose

Define portable, in-process protobuf code generation for workspace build scripts while preserving generated Rust and schema-change tracking.

## Requirements

### Requirement: In-process workspace protobuf code generation

The build scripts of camel-component-grpc, camel-component-validator, camel-xslt, camel-component-jms, camel-component-cxf and camel-bench SHALL compile repository-owned protobuf sources with the workspace-pinned protox and generate Rust with tonic-prost-build compile_fds. They SHALL NOT invoke protoc or set PROTOC. Workspace manifests and Cargo.lock SHALL NOT depend on protoc-bin-vendored or its platform packages.

#### Scenario: Build succeeds without a protoc invocation

- **GIVEN** all six packages and PROTOC pointing to an executable sentinel that writes an invocation marker and exits unsuccessfully
- **WHEN** their build scripts regenerate Rust and all targets compile
- **THEN** compilation succeeds and the invocation marker does not exist

### Requirement: Generated Rust and source tracking remain compatible

The migration SHALL preserve the complete generated Rust file set and bytes per affected package, the existing protobuf include roots, tonic/prost generation options and OUT_DIR behavior. The gRPC build script SHALL obtain CARGO_MANIFEST_DIR at script execution time. Each build script SHALL declare watches for its protobuf inputs and include root so Cargo regenerates output when those files change.

#### Scenario: Generated outputs match the protoc baseline

- **GIVEN** saved generated Rust from the six unmodified build scripts at commit 2ae4f107
- **WHEN** the migrated scripts generate Rust for the same sources and generator versions
- **THEN** each package has exactly the same generated filenames and byte contents

#### Scenario: Canonical XML schema and gRPC fixture paths stay valid

- **GIVEN** a linked feature worktree and the existing protobuf fixture locations
- **WHEN** gRPC and camel-bench execute their build scripts
- **THEN** gRPC resolves its fixture paths using the current manifest directory and camel-bench reads bridges/xml/src/main/proto/xml_bridge.proto

#### Scenario: Editing a tracked schema reruns generation

- **GIVEN** a successfully generated package
- **WHEN** a protobuf source or include directory changes
- **THEN** Cargo reruns its build script through the declared rerun-if-changed paths

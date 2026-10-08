## ADDED Requirements

### Requirement: Protobuf runtime compilation is hermetic

The protobuf data format and every other caller of
`camel_proto_compiler::compile_proto` SHALL compile `.proto` source inside the
process. Compilation SHALL NOT start a subprocess, SHALL NOT read the
`PROTOC` environment variable, SHALL NOT write a temporary file, and SHALL NOT
depend on any path baked in at build time. The result SHALL be a
`DescriptorPool` built by decoding the encoded `FileDescriptorSet` of the
compilation, so that the packed-encoding flag of every field matches `protoc`.
The `compile_proto` and `ProtoCache::get_or_compile` signatures SHALL NOT
change.

#### Scenario: Compilation works with no protoc, no PATH and no temp directory

- **GIVEN** a child process (re-exec of the test binary) with `PROTOC` unset,
  an empty `PATH`, and `TMPDIR` pointing to a directory that does not exist
- **WHEN** `compile_proto` runs on the repo `helloworld.proto`, `streaming.proto`
  and `recursive.proto` fixtures and on the kitchen-sink fixture (imports,
  well-known types, proto2 import, custom options, oneof, map, services)
- **THEN** every call succeeds and the pool exposes the expected messages and services

#### Scenario: An environment override cannot bring an external compiler back

- **GIVEN** `PROTOC` set to an executable script that writes a marker file, and
  `PATH` containing only the directory of a script named `protoc`
- **WHEN** `compile_proto` runs on the kitchen-sink fixture
- **THEN** compilation succeeds and no marker file exists

#### Scenario: Packed encoding matches protoc

- **GIVEN** a proto3 field `repeated double w = 1 [packed = false]` and a proto2
  field `repeated int32 v = 1 [packed = true]`
- **WHEN** a JSON message is encoded through the pool returned by `compile_proto`
- **THEN** the proto3 field encodes unpacked (`09 ..` repeated) and the proto2
  field encodes packed (`0a 02 01 02`)

#### Scenario: The runtime crate has no executable-spawning surface

- **GIVEN** the `camel-proto-compiler` non-test source (every file in `src/`,
  `#[cfg(test)]` code excluded) and the manifest `[dependencies]` table
- **WHEN** a test scans them for the case-sensitive tokens `Command`,
  `tempfile`, `temp_dir`, `PROTOC` and `protoc-bin-vendored`
- **THEN** none is found (lowercase `protoc` inside user-facing remedy text is allowed)

### Requirement: Precompiled descriptor set input

Wherever a `.proto` path is accepted by `compile_proto` (and therefore by the
protobuf data format, `ProtoCache`, the DSL `protobuf:` resolver and the gRPC
`protoFile` parameter), the system SHALL also accept a precompiled
`FileDescriptorSet`. The system SHALL select the descriptor-set path when the
file extension is `binpb`, `pb`, `desc` or `protoset` (ASCII case-insensitive)
and SHALL decode the file directly without compiling. The set SHALL contain all
imports. Include directories SHALL be ignored for this input. A file that does
not decode, or whose option text exceeds the nesting limit, SHALL fail with
`ProtoCompileError::DescriptorDecode` naming the path.

#### Scenario: A descriptor set loads

- **GIVEN** a `helloworld.binpb` file produced by `protoc --include_imports --descriptor_set_out`
- **WHEN** `compile_proto` and `ProtoCache::get_or_compile` run on it
- **THEN** the pool exposes `helloworld.HelloRequest` and `helloworld.Greeter`

#### Scenario: An editions descriptor set fails with a typed error

- **GIVEN** a descriptor set compiled by protoc from an `edition = "2023"` source (file `syntax` is `editions`)
- **WHEN** `compile_proto` runs on the `.binpb` file
- **THEN** the result is `DescriptorDecode` whose message names editions and the path

#### Scenario: A hostile descriptor set fails with a typed error

- **GIVEN** a `.binpb` whose `uninterpreted_option.aggregate_value` nests 10 000 levels of `{` or `<`, including a variant where each `>` is hidden after a `#` comment (`f < # >` newline, repeated)
- **WHEN** `compile_proto` runs on it
- **THEN** the result is `DescriptorDecode` and no panic or abort occurs

#### Scenario: A corrupt descriptor set fails with a typed error

- **GIVEN** a file `bad.binpb` containing bytes that are not a `FileDescriptorSet`
- **WHEN** `compile_proto` runs on it
- **THEN** the result is `DescriptorDecode` and the message contains the path

### Requirement: Malformed proto or descriptor-set input never terminates the process

The compiler SHALL convert every failure on `.proto` or descriptor-set input
into a typed `ProtoCompileError`. It SHALL reject any source file (the
top-level file and every imported file) whose bracket nesting depth (`{`, `<`
and `[` count as openers) exceeds 64, before parsing it, SHALL apply the same
limit to option text inside descriptor sets before building the pool, and
SHALL contain any panic raised by the protox compile or by the pool decode.
The compiler SHALL return a typed error for every rejected or failed input
instead of terminating the process.

For `.proto` source, the compiler SHALL preload the reachable root/import
closure through the guarded include resolvers into an immutable snapshot
before it constructs any internal pool. The snapshot SHALL cache each parsed
file and freeze each import that no resolver can name. After preflight the
compiler SHALL resolve imports from the snapshot only, and SHALL NOT read the
filesystem or the embedded well-known types again. Embedded well-known types
SHALL NOT count toward the source file count or the source byte budget.

Before any pool construction the compiler SHALL bound the dependency graph.
The source input budget SHALL be at most 256 distinct include-resolved schema
files and 64 MiB cumulative source bytes, with at most 16 MiB per single
input. Every descriptor graph, from source or from a descriptor set, SHALL
keep its longest import chain at or below 256 files and its
multiplicity-aware public-import traversal work `W` at or below 100 000,
where `W = N + sum E(d)` over every file and every direct import occurrence
`d` (private imports included, never deduplicated) and
`E(v) = 1 + sum E(d)` over every public import occurrence of `v`. A `.proto`
source graph SHALL also keep its lifetime `(N + 1) * W` at or below 100 000,
because protox builds its internal pool incrementally. The compiler SHALL
reject cyclic descriptor-set dependency graphs before pool decoding. `W` and
the lifetime bound are conservative bounds on traversal calls, not exact
counts of every scanning operation. Syntax and semantic
errors SHALL be reported as `ProtoCompileError::Compile { path, detail }`
where `detail` carries the `file:line:column: message` form with the same line
and column that `protoc` reports.

#### Scenario: Deep nesting is a typed error

- **GIVEN** `.proto` files with message nesting depth 100 and depth 10 000
- **WHEN** `compile_proto` runs on each from a thread with a 2 MiB stack
- **THEN** each returns `Compile` and no panic or abort occurs

#### Scenario: Angle-bracket option nesting is a typed error

- **GIVEN** a file with `option (r) = { f < f < ... > > };` nested 10 000 levels with angle brackets, so brace depth is 1
- **WHEN** `compile_proto` runs
- **THEN** it returns `Compile` and no panic or abort occurs

#### Scenario: An unterminated string cannot hide deep nesting

- **GIVEN** a file with an unterminated string literal on one line followed by 10 000 nested braces
- **WHEN** `compile_proto` runs
- **THEN** it returns `Compile` and no panic or abort occurs

#### Scenario: An escaped quote cannot hide deep nesting

- **GIVEN** a file with the line `x = "\"" {{{...` (an escaped quote, then 10 000 openers)
- **WHEN** `compile_proto` runs
- **THEN** it returns `Compile` and no panic or abort occurs

#### Scenario: A deep import chain is a typed error

- **GIVEN** 300 schema files, each containing only a syntax declaration and one import of the next
- **WHEN** `compile_proto` runs on the first file from a thread with a 2 MiB stack
- **THEN** the result is `Compile` whose detail names the import-graph limit and no panic or abort occurs

#### Scenario: A cyclic descriptor set is a typed error

- **GIVEN** a `.binpb` whose dependency graph contains a cycle (a file importing itself through `public_dependency`, or two files importing each other)
- **WHEN** `compile_proto` runs on it
- **THEN** the result is `DescriptorDecode` whose message names the cyclic import graph and no panic or abort occurs

#### Scenario: An acyclic descriptor graph with pathological shape is a typed error

- **GIVEN** a `.binpb` whose dependency graph is acyclic but whose longest import chain exceeds 256 files, or whose public-import traversal work `W` exceeds 100 000 (layered diamonds or a leaf-first chain with a shared tail)
- **WHEN** `compile_proto` runs on it
- **THEN** the result is `DescriptorDecode` naming the exceeded bound, and no panic, abort or unbounded work occurs

#### Scenario: A private-root descriptor graph is a typed error

- **GIVEN** a `.binpb` with 150 files on a public chain and 1000 private root
  files, each importing all 150 chain names leaf-first, so every private root
  starts a separate public-import walk
- **WHEN** `compile_proto` runs on it, in either file order
- **THEN** the result is `DescriptorDecode` naming the resolution budget, and
  the set is never decoded into a pool

#### Scenario: A layered public-import source graph is a typed error

- **GIVEN** 70 width-two public-import layers plus a private root that imports
  every layer file leaf-first (141 source files, below the 256-file and 64 MiB
  source caps)
- **WHEN** `compile_proto` runs on the root from a thread with a 2 MiB stack
- **THEN** the result is `Compile` naming the resolution budget, rejected
  before any internal pool is built, and no panic or abort occurs

#### Scenario: A source lifetime bound is a typed error

- **GIVEN** 10 width-two public-import layers plus a private root whose final
  graph `W` stays below 100 000 but whose lifetime `(N + 1) * W` exceeds it
- **WHEN** `compile_proto` runs on the root
- **THEN** the result is `Compile` naming the source lifetime limit, and no
  panic or abort occurs

#### Scenario: An oversized schema input is a typed error

- **GIVEN** a schema source or descriptor set larger than the schema byte limit (16 MiB), and a `ProtoCache` asked for the same path
- **WHEN** `compile_proto` or `get_or_compile` runs
- **THEN** each returns a typed error naming the size limit, without reading or hashing the full file

#### Scenario: A non-regular file in an include path is a typed error

- **GIVEN** an import or top-level path that resolves to a directory or device instead of a regular file
- **WHEN** `compile_proto` runs
- **THEN** the result is a typed error stating the input is not a regular file, and no unscanned content is parsed

#### Scenario: Deep nesting in an imported file is a typed error

- **GIVEN** a valid top-level file that imports a file nested 10 000 levels deep
- **WHEN** `compile_proto` runs
- **THEN** it returns `Compile` and names the imported file

#### Scenario: A syntax error carries line and column

- **GIVEN** a file whose line 2 has `message Broken { string x = ; }`
- **WHEN** `compile_proto` runs
- **THEN** the result is `Compile` and `detail` contains `:2:` and the file name

#### Scenario: A panic inside the compiler is contained

- **GIVEN** a compile call whose closure panics
- **WHEN** the containment wrapper runs
- **THEN** the result is `Compile` with `detail` starting `internal compiler panic:` and the process stays alive

#### Scenario: A panic on the descriptor-set path is contained

- **GIVEN** a descriptor-set load whose closure panics
- **WHEN** the containment wrapper runs
- **THEN** the result is `DescriptorDecode` whose text starts `internal decoder panic:` and the process stays alive

### Requirement: Editions are unsupported and rejected with a remedy

A `.proto` source that declares `edition = "..."` SHALL fail with
`ProtoCompileError::Compile` whose `detail` states that protobuf editions are
not supported and that the user SHALL rewrite the schema with
`syntax = "proto3"` or `syntax = "proto2"`. A descriptor set that contains a
file with `syntax = "editions"` SHALL fail with
`ProtoCompileError::DescriptorDecode` naming editions, before the pool decode.
The system SHALL NOT advise a precompiled set as a workaround for editions.

#### Scenario: Editions source fails with guidance

- **GIVEN** a `.proto` file starting `edition = "2023";`
- **WHEN** `compile_proto` runs
- **THEN** the result is `Compile` and `detail` contains `editions` and `proto3`

## REMOVED Requirements

### Requirement: Protobuf runtime protoc resolution

**Reason**: the runtime `protoc` path is removed (ADR-0084); compilation is in-process.

**Migration**: `PROTOC` is ignored. Editions are unsupported; rewrite the schema as `proto3` or `proto2`.

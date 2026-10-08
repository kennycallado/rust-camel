# Design: protox-compiler

## Approach

All runtime `.proto` compilation passes through one function,
`camel_proto_compiler::compile_proto` (callers: protobuf data format, DSL
`protobuf:` resolver, gRPC consumer and producer, through `ProtoCache`). This
change rewrites that function and nothing at the call sites.

`compile_proto(path, includes)` flow:

1. Missing path: `ProtoNotFound` (unchanged).
2. Extension `binpb`, `pb`, `desc` or `protoset` (ASCII case-insensitive):
   read the file (same 16 MiB cap and regular-file check as sources) and
   decode it into `prost_types::FileDescriptorSet` (prost applies a recursion
   limit). An iterative dependency-graph preflight rejects cyclic sets
   (self-imports and public-import cycles) with a typed error before the pool
   decode: prost-reflect 0.16.5 recurses without a bound on public-import
   cycles. That same preflight bounds the longest import chain (256 files) and
   the multiplicity-aware public-import traversal work `W = N + sum E(d)`
   (100 000), so an acyclic but diamond-rich or leaf-first set cannot expand
   exponentially. These are conservative bounds on traversal calls, not exact
   counts of every scanning operation. Run `scan_nesting` on every
   `uninterpreted_option.aggregate_value` string in the set, because
   `DescriptorPool::decode` parses that text recursively without a depth
   limit. Then call `DescriptorPool::decode(original_bytes)` (not
   `from_file_descriptor_set`, which drops custom option values). The decode
   runs inside the same `catch_unwind` as step 3. Failure maps to
   `DescriptorDecode`; a panic on this path maps to
   `DescriptorDecode("internal decoder panic: ..")`. Includes are ignored. The set must contain its
   imports (`--include_imports`); a set without them fails to decode.
3. Otherwise compile the source with `protox`:
   - Include list = user includes, then the parent directory of the proto
     (empty parent becomes `.`). This is the current protoc argument order.
   - Preflight loads the reachable root/import closure before any pool is
     built. File resolution uses a `ChainFileResolver` built by this crate:
     one `NestingGuardedInclude` resolver per include directory, then
     `GoogleFileResolver`. The guard runs before protox parses, so it covers
     the top-level file and every import. The wrapper opens the file once
     (nonblocking on Unix through `libc` `O_NONBLOCK`, so a writerless FIFO
     cannot hang the open), fstats the open descriptor, rejects non-regular
     files (directories, devices, FIFOs), reads at most `MAX_SCHEMA_BYTES`
     (16 MiB) from that descriptor, scans the buffer, and parses the SAME
     buffer through `File::from_source` — there is no second read
     (landing-gate fix, 2026-10-08: the earlier scan-then-reparse window
     admitted an abort). Each file is parsed once, in declaration preorder,
     and no pool is built. A missing file returns protox `file_not_found` so
     the chain tries the next include directory, exactly as before.
   - The preload result is an immutable `SourceSnapshot`: it caches every
     parsed `File`, keeps declaration order, and records every import that no
     resolver could name. A `SnapshotResolver` serves cached files to the
     later `Compiler` and returns `file_not_found` for unknown or
     recorded-missing names. There is no filesystem or Google fallback after
     preflight. Shadow semantics are replicated in the wrapper:
     `resolve_path` records the first resolving include, and `open_file`
     errors when the file comes from a different include directory than the
     one that resolved the user's path (equivalent to protox `check_shadow`,
     which needs `File::path` and cannot see a `from_source` file).
   - Symbolic links are followed, matching protoc. The link target must
     resolve to a regular file through the open descriptor; every other kind,
     or a target above the byte cap, is a typed error. This is an explicit
     policy decision (ADR-0084 context), not an oversight: rejecting links
     would break protoc-compatible schema layouts, and the single-read
     validation makes following safe.
   - The source-only `ImportBudget` (shared by every include resolver, 256
     distinct include-resolved files / 64 MiB cumulative) bounds source
     input. It does not bound traversal work on its own: repeated occurrences
     of the same dependency can expand exponentially while the distinct-file
     count stays small. Embedded well-known types come from the
     `GoogleFileResolver` and consume neither the file nor the byte budget.
   - Before any `Compiler` exists, `check_source_graph` validates the
     snapshot's `FileDescriptorSet`: iterative cycle rejection, the longest
     chain (256 files), the multiplicity-aware work bound `W = N + sum E(d)`
     (100 000), and the source lifetime `(N + 1) * W` (100 000), all with
     saturating arithmetic. `E(v) = 1 + sum E(d)` over every public
     dependency occurrence of `v`. The `W` sum runs over every direct
     dependency occurrence `d` of every file, private included, and never
     deduplicates. `W` and the lifetime bound are conservative bounds on
     traversal calls, not exact counts of every scanning operation.
   - `scan_nesting(text, 64)` is a single-pass scanner shared by sources and
     descriptor-set option text. Openers are `{ < [`, closers are `} > ]`;
     the depth never goes below 0. Angle brackets count because protox keeps
     option aggregate values as flat text that prost-reflect parses
     recursively when it builds the pool, and `<...>` nests like `{...}`.
     A line comment ends at newline; a block comment ends at the first
     `*/`; a string literal ends at its matching quote or at a newline
     (protox string rule), so an unterminated string cannot hide deep
     nesting. Inside a string a backslash consumes the next character
     (except a newline), so `"\""` does not end the string early. A second
     mode, used only for descriptor option text, also treats `#` as a line
     comment to newline (the prost-reflect text-format parser does); the
     `.proto` source mode does not.
   - `include_imports(true)`, `include_source_info(false)`.
   - One closure runs inside `catch_unwind(AssertUnwindSafe(..))` and
     covers preflight, graph validation,
     `Compiler::with_file_resolver(snapshot)` construction, `open_file`,
     `encode_file_descriptor_set` (the measured protox panic is in this
     call) and `DescriptorPool::decode`. A panic becomes `Compile { detail: "internal compiler panic: .." }`.
     The global panic hook is not touched.
   - The pool is `DescriptorPool::decode(compiler.encode_file_descriptor_set())`,
     built from the snapshot alone. Source files are never reopened between
     graph validation and this decode.
     `Compiler::descriptor_pool()` is never used: it reports a wrong
     `is_packed()` for proto3 `[packed=false]` and proto2 `[packed=true]`.
4. Error mapping: `protox::Error` becomes `Compile { path, detail }` with
   `detail = format!("{e:?}")` (`file:line:col: message`; the position
   matches protoc). A test pins this form. When the detail contains
   `found 'edition'`, the remedy is appended: protobuf editions are not
   supported; rewrite the schema with `syntax = "proto3"` or `"proto2"`.
   Editions descriptor sets are also unsupported: prost-reflect 0.16 accepts
   only `syntax` `proto2` and `proto3` (`UnknownSyntax`), so step 2 rejects
   any file with `syntax == "editions"` before the pool decode, with a
   `DescriptorDecode` message that names editions.

`ProtoCompileError` becomes:
`ProtoNotFound(PathBuf)`, `Io(io::Error)`, `Compile { path, detail }`,
`DescriptorDecode(String)`, marked `#[non_exhaustive]`. The old
`ProtocUnavailable` and `ProtocFailed` variants are deleted. No code
outside the crate matches on them (verified by grep).

Removed from the crate: `resolve_protoc*`, `SilencePanicHook`, `HOOK_SWAP`,
`std::process::Command`, the temporary descriptor file, the
`protoc-bin-vendored` and (runtime) `tempfile` dependencies. `tempfile`
stays as a dev-dependency. `PROTOC` is no longer read.

`ProtoCache` keeps its key (path, SHA-256 of the top-level file bytes,
include-path hash). Descriptor-set files hash their bytes like sources.
Import invalidation stays a separate bd (`rc-me2ii`).

Workspace: add `protox = "=0.9.1"` next to `prost-reflect`, and `libc` for
the nonblocking schema open. `protox` 0.9.1
depends on `prost` 0.14 and `prost-reflect` 0.16, which match the workspace.
The `camel-cli` golden `default-deptree.txt` is regenerated with the
documented command; its diff must contain only the `protox` family.

ADR-0084 (`docs/adr/`) records the decision. Docs: `docs/src/data-formats/protobuf.md`,
`docs/src/components/grpc.md`, crate README. Spec: `data-formats` loses the
"Protobuf runtime protoc resolution" requirement and gains the hermetic,
descriptor-set, malformed-input and editions-unsupported requirements.

## Affected crates

- `camel-proto-compiler`: rewrite of `compiler.rs`, error type, tests, README, Cargo.toml.
- `camel-dataformat-protobuf`, `camel-component-grpc`, `camel-dsl`: no code change; existing tests re-run.
- `camel-cli`: regenerated `tests/fixtures/default-deptree.txt`.
- Workspace `Cargo.toml` and `Cargo.lock`: `protox` added.

## Architecture boundaries

`camel-proto-compiler` is a Services-layer crate with no dependency on the
Runtime. It stays a leaf: new dependencies are `protox` and `libc` (the
latter for the nonblocking schema open on Unix), plus their transitive
crates. No data-plane or control-plane contract changes. The
hermetic-runtime rule is recorded in ADR-0084 and extends the intent of
ADR-0075 (self-contained executable artifact).

## Alternatives considered

- Embed `protoc` bytes and extract at run time (option B): needs a writable
  and executable directory, fails on `FROM scratch`, adds 9 to 14 MB per
  target, covers only 8 targets. Rejected.
- Docs-only (option C): keeps the defect, contradicts the owner premise.
  Rejected.
- Keep a `PROTOC` override beside protox: `PROTOC` is a generic variable.
  It would hide dev and production divergence. Rejected. The descriptor-set
  input covers the same need without an executable.
- `Compiler::descriptor_pool()`: wrong packed flag. Rejected.
- Depth pre-scan of the top-level file only: imports could still abort the
  process. Rejected in favour of the resolver wrapper.
- Counting only braces: `option x = { f < f < ... > > }` has brace depth 1
  and overflows the stack in the option text parser. Rejected; all three
  bracket kinds count.

## Known gap (out of scope, bd filed)

`camel compile` sealed artifacts write `protoFile` assets under
`std::env::temp_dir()` at boot (`camel-cli/src/compile/materialize.rs`), and
the gRPC config accepts absolute paths only inside that directory. A sealed
artifact on a `FROM scratch` image therefore still lacks a temporary
directory. This change fixes the `camel run`, Rust API and library paths.
A separate bd tracks the sealed-artifact path.

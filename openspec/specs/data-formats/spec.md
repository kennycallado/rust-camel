# data-formats Specification

## Purpose
Built-in data formats and the protobuf compiler. TAR, GZIP, and TAR.GZ marshal
and unmarshal exchange bodies under bounded configuration, and the protobuf
compiler runs in-process from `.proto` source or a precompiled descriptor set
and rejects malformed or editions-based input with typed errors.
## Requirements
### Requirement: TAR data format

The system SHALL provide a built-in `tar` data format that marshals one exchange body into one fixed-name `payload` regular-file entry and unmarshals the first regular-file entry entirely in memory.

#### Scenario: TAR byte round trip

- **GIVEN** an exchange with a non-empty byte body
- **WHEN** the body is marshaled as `tar` and then unmarshaled as `tar`
- **THEN** the resulting body bytes equal the original bytes

#### Scenario: TAR entry policy

- **GIVEN** a TAR archive containing zero, one, or multiple regular-file entries, with any number of directories, symlinks, hardlinks, or device entries
- **WHEN** `allow_multi_entry` is false
- **THEN** unmarshal succeeds only when exactly one regular-file entry exists, and otherwise fails with a no-regular-file or multi-entry error
- **WHEN** `allow_multi_entry` is true
- **THEN** unmarshal returns bytes from the first regular-file entry and emits a warning when more than one regular-file entry exists

#### Scenario: TAR non-regular entries

- **GIVEN** an archive containing directories, symlinks, hardlinks, or device entries before or beside a regular file
- **WHEN** it is unmarshaled
- **THEN** non-regular entries are not followed or returned and no filesystem path is accessed

### Requirement: GZIP and TAR.GZ data formats

The system SHALL provide built-in `gzip` and `tar.gz` formats. `gzip` SHALL compress and decompress arbitrary materialized body bytes. `tar.gz` SHALL use the same TAR entry and entry-selection semantics as `tar` with gzip compression around the TAR stream.

#### Scenario: Composable and combined output

- **GIVEN** identical input bytes
- **WHEN** the bytes are marshaled as `tar` then `gzip`
- **THEN** the result can be unmarshaled as `gzip` then `tar` to recover the input
- **WHEN** the bytes are marshaled as `tar.gz`
- **THEN** the result can be unmarshaled as `tar.gz` to recover the input
- **AND WHEN** `tar.gz` output is unmarshaled as `gzip` then `tar`
- **THEN** it recovers the input
- **AND WHEN** `tar` then `gzip` output is unmarshaled as `tar.gz`
- **THEN** it recovers the input

#### Scenario: GZIP decompression limit

- **GIVEN** compressed input whose total decompressed TAR/GZIP stream, including headers, padding, and skipped entries, exceeds `max_decompressed_size`
- **WHEN** it is unmarshaled as `gzip` or `tar.gz`
- **THEN** unmarshal fails before materializing bytes beyond the configured limit

### Requirement: Bounded format configuration

The system SHALL reject `Body::Empty`, unsupported stream bodies, input larger than `max_input_size`, invalid compression levels, and unknown configuration fields with the established data-format errors. Zero-length `Body::Bytes` and `Body::Text` remain materialized bodies and follow archive semantics.

#### Scenario: Stream body rejection

- **GIVEN** an exchange body represented by `Body::Stream`
- **WHEN** it is marshaled or unmarshaled by one of the new formats
- **THEN** the operation fails without consuming the stream

### Requirement: Data-format integration and scope

The system SHALL expose `tar`, `gzip`, and `tar.gz` through the built-in registry and documented format metadata. The system SHALL not add a TAR splitter or filesystem extraction in v1; entry-per-exchange splitting remains a separate capability.

#### Scenario: Registry resolution

- **GIVEN** a route requests `tar`, `gzip`, or `tar.gz`
- **WHEN** the built-in data-format factory resolves the name
- **THEN** it returns the corresponding data format implementation

#### Scenario: Malicious archive paths stay in memory

- **GIVEN** an archive contains `../escape`, absolute, or symlink entry paths
- **WHEN** it is unmarshaled by this change
- **THEN** no filesystem path is accessed and the entry is ignored unless it is a regular file selected by the in-memory rules; any future disk extraction applies the nearest-existing-ancestor and path/symlink confinement precedent from `rc-0ks57`

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

### Requirement: Form data format

The system SHALL provide a built-in `form` data format (`name() == "form"`)
that unmarshals an `application/x-www-form-urlencoded` body into a JSON
object and marshals a JSON object back into form text, under bounded
configuration (`max_fields`, default 1000, counting total non-empty pairs
with duplicates included; `max_bytes`, default 1 MiB, on input length).
The canon "Bounded format configuration" requirement's `Body::Empty`
rejection covers the archive formats only; the form format follows the
JSON format here (`Body::Empty` is a no-op).
Every parse or encode failure SHALL fail closed with a
`CamelError::TypeConversionFailed` whose message is prefixed `form` and
names the cap kind or the malformed escape with its byte offset. Unmarshal
SHALL accept `Body::Text` and `Body::Bytes`; `Body::Empty` SHALL be a no-op
returning `Body::Empty` (mirroring the JSON format); `Body::Stream` SHALL
fail closed and streams SHALL never be consumed. Pairs split on `&`; empty
segments (leading/trailing `&`, `&&`) are skipped and do not count toward
`max_fields`; each non-empty pair splits on the first `=`; a missing `=`
yields an empty value; an empty value is `""`; `+` decodes to space; `%XX`
escapes decode strictly (malformed escapes fail) and decoded bytes SHALL be
valid UTF-8. Duplicate keys SHALL become arrays with values in first-seen
(encounter) order — no lossy object overwrite. Object key iteration order
is unspecified; consumers compare by JSON value. Marshal: `Body::Empty` is
a no-op; only a top-level JSON object marshals — strings pass through,
number/bool/null stringify to their JSON lexical form, arrays repeat the
key per scalar element, nested objects or arrays SHALL be rejected, and
the empty object marshals to `""`. Marshal encoding SHALL follow the
WHATWG `application/x-www-form-urlencoded` serializer: ASCII alphanumerics
and `*-._` pass unescaped, space becomes `+`, every other byte becomes
uppercase `%XX` over its UTF-8 encoding.

#### Scenario: single pair unmarshals to a flat object

- **GIVEN** an exchange whose body is `Body::Text("name=camel")`
- **WHEN** the form data format unmarshals it
- **THEN** the body is a JSON object equal to `{"name": "camel"}`

#### Scenario: duplicate keys become arrays in encounter order

- **GIVEN** an exchange whose body is
  `Body::Text("tag=a&tag=b&tag=c")`
- **WHEN** the form data format unmarshals it
- **THEN** the body is a JSON object where `tag` equals the array
  `["a", "b", "c"]`

#### Scenario: plus and percent escapes decode

- **GIVEN** an exchange whose body is
  `Body::Text("a=one+two&b=one%20two&c=1%2B1")`
- **WHEN** the form data format unmarshals it
- **THEN** the JSON object equals `{"a": "one two", "b": "one two",
  "c": "1+1"}`

#### Scenario: missing equals yields an empty value

- **GIVEN** an exchange whose body is `Body::Text("flag&x=")`
- **WHEN** the form data format unmarshals it
- **THEN** the JSON object equals `{"flag": "", "x": ""}`

#### Scenario: empty key is allowed

- **GIVEN** an exchange whose body is `Body::Text("=v")`
- **WHEN** the form data format unmarshals it
- **THEN** the JSON object equals `{"": "v"}`

#### Scenario: empty input unmarshals to an empty object

- **GIVEN** an exchange whose body is `Body::Text("")`
- **WHEN** the form data format unmarshals it
- **THEN** the body is the JSON object `{}`

#### Scenario: empty segments are skipped

- **GIVEN** an exchange whose body is `Body::Text("a=1&&b=2")` or
  `Body::Text("&a=1&")`
- **WHEN** the form data format unmarshals it
- **THEN** the JSON object equals `{"a": "1", "b": "2"}` (respectively
  `{"a": "1"}`) and the empty segments do not count toward `max_fields`

#### Scenario: UTF-8 payload round trip

- **GIVEN** an exchange whose body is `Body::Text("nome=café")` encoded as
  UTF-8
- **WHEN** the form data format unmarshals it
- **THEN** the JSON object equals `{"nome": "café"}` and marshalling that
  object back yields `nome=caf%C3%A9`

#### Scenario: malformed percent escape fails closed

- **GIVEN** an exchange whose body is `Body::Text("a=100%2")` or
  `Body::Text("a=%zz")`
- **WHEN** the form data format unmarshals it
- **THEN** unmarshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form` and names the malformed escape

#### Scenario: invalid UTF-8 fails closed

- **GIVEN** an exchange whose body is `Body::Bytes(b"a=%FF")` or
  `Body::Bytes(b"a=%E9")` (lone continuation/leading byte)
- **WHEN** the form data format unmarshals it
- **THEN** unmarshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form`

#### Scenario: field cap fails closed and the exact cap passes

- **GIVEN** an exchange whose body contains more non-empty pairs than
  `max_fields`
- **WHEN** the form data format unmarshals it
- **THEN** unmarshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form` and names `max_fields`
- **GIVEN** an exchange whose body contains exactly `max_fields` non-empty
  pairs
- **WHEN** the form data format unmarshals it
- **THEN** unmarshal succeeds

#### Scenario: byte cap fails closed before parsing

- **GIVEN** an exchange whose body length exceeds `max_bytes`
- **WHEN** the form data format unmarshals it
- **THEN** unmarshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form` and names `max_bytes`, before any pair is
  parsed

#### Scenario: empty body passes through unchanged

- **GIVEN** an exchange whose body is `Body::Empty`
- **WHEN** the form data format unmarshals or marshals it
- **THEN** the body stays `Body::Empty` and no error is raised

#### Scenario: stream body fails closed

- **GIVEN** an exchange whose body is `Body::Stream`
- **WHEN** the form data format unmarshals or marshals it
- **THEN** the operation fails with `CamelError::TypeConversionFailed` and
  the stream is not consumed

#### Scenario: marshal flat object

- **GIVEN** an exchange whose body is the JSON object `{"a": "1", "b": "2"}`
- **WHEN** the form data format marshals it
- **THEN** the form text, re-parsed as pairs, equals the set
  `{a: "1", b: "2"}` joined by `&` (pair order unspecified)

#### Scenario: marshal array values repeat the key

- **GIVEN** an exchange whose body is the JSON object
  `{"tag": ["a", "b"]}`
- **WHEN** the form data format marshals it
- **THEN** the body is form text exactly `tag=a&tag=b`

#### Scenario: marshal scalar values stringify to their JSON lexical form

- **GIVEN** an exchange whose body is the JSON object
  `{"n": 1, "b": true, "z": null}`
- **WHEN** the form data format marshals it
- **THEN** the form text, re-parsed as a pair-set, equals
  `{n: "1", b: "true", z: "null"}`

#### Scenario: marshal encodes per the WHATWG serializer

- **GIVEN** an exchange whose body is the JSON object
  `{"q": "a b&c=d+é"}`
- **WHEN** the form data format marshals it
- **THEN** the body is form text exactly `q=a+b%26c%3Dd%2B%C3%A9`

#### Scenario: marshal rejects nested structures

- **GIVEN** an exchange whose body is the JSON object
  `{"a": {"b": "1"}}` or `{"a": [["1"]]}`
- **WHEN** the form data format marshals it
- **THEN** marshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form` and names the nested value

#### Scenario: marshal empty object yields empty text

- **GIVEN** an exchange whose body is the JSON object `{}`
- **WHEN** the form data format marshals it
- **THEN** the body is `Body::Text("")`

#### Scenario: marshal rejects non-object top-level values

- **GIVEN** an exchange whose body is a JSON array or scalar
- **WHEN** the form data format marshals it
- **THEN** marshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form`

### Requirement: XML data format marshal body acceptance

The built-in `xml` data format (`XmlDataFormat`, `name() == "xml"`) SHALL
marshal `Body::Json` to `Body::Text` containing XML, pass `Body::Text` and
`Body::Empty` through unchanged, and pass an already-serialized XML body
through unchanged: `Body::Xml` SHALL be accepted as-is, and `Body::Bytes`
SHALL be accepted only when the bytes are valid UTF-8, in which case the
bytes are passed through unchanged (the body shape stays `Body::Bytes`, with
no re-encoding and no conversion to `Body::Text`). `Body::Bytes` that is not
valid UTF-8 SHALL fail closed with a `CamelError::TypeConversionFailed` whose
message names `Body::Bytes`, states that the bytes are not valid UTF-8, and
reports the first invalid byte offset. `Body::Stream` SHALL fail closed
without being consumed. The step SHALL NOT parse or re-serialize a
`Body::Xml`/`Body::Bytes` payload, SHALL NOT validate XML well-formedness for
those bodies, and SHALL NOT sniff content types; the caller owns the
already-serialized wire form. The UTF-8-only rule matches the REST media
gate, which rejects non-UTF-8 XML charset declarations.

#### Scenario: Xml body passes through unchanged

- **GIVEN** an exchange whose body is
  `Body::Xml("<user><name>kenny</name></user>")`
- **WHEN** the XML data format marshals it
- **THEN** the resulting body is exactly the same `Body::Xml` value

#### Scenario: valid UTF-8 bytes pass through unchanged

- **GIVEN** an exchange whose body is `Body::Bytes` holding
  `<user><name>kenny</name></user>` encoded as UTF-8
- **WHEN** the XML data format marshals it
- **THEN** the resulting body is `Body::Bytes` with byte-identical content

#### Scenario: invalid UTF-8 bytes fail with a typed diagnostic

- **GIVEN** an exchange whose body is `Body::Bytes` containing the byte
  sequence `[0xFF, 0xFE]`
- **WHEN** the XML data format marshals it
- **THEN** marshalling fails with `CamelError::TypeConversionFailed` whose
  message contains `Body::Bytes`, `UTF-8`, and the first invalid byte offset
  (`0`)

#### Scenario: JSON body still marshals to XML text

- **GIVEN** an exchange whose body is `Body::Json({"user": {"name": "kenny"}})`
- **WHEN** the XML data format marshals it
- **THEN** the resulting body is `Body::Text("<user><name>kenny</name></user>")`

#### Scenario: stream body fails closed

- **GIVEN** an exchange whose body is `Body::Stream`
- **WHEN** the XML data format marshals it
- **THEN** marshalling fails with `CamelError::TypeConversionFailed` and the
  stream is never consumed


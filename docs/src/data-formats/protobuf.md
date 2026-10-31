# Protobuf

The protobuf data format converts between JSON and binary protobuf wire format. It uses `prost-reflect` for dynamic message descriptors that the format compiles at runtime, so it requires no compile-time code generation. The compiler is built in. It is pure Rust and needs no external tool. It ships as a separate crate, `camel-dataformat-protobuf`.

Marshal converts `Body::Json` to `Body::Bytes`. Unmarshal reverses the conversion and returns `Body::Json`. The round trip preserves field values through the JSON bridge.

## When to use protobuf

Choose protobuf when the contract is a gRPC service or when the schema must evolve without breaking older clients. Protobuf carries typed fields, forward and backward compatibility, and a compact binary encoding. Choose JSON instead when the consumer is a browser, a REST API, or any system that reads text. JSON is readable, universal, and cheaper to debug. See [Data Formats](index.md) for the full format catalog.

## Construction

`ProtobufDataFormat` takes a proto file path and a fully-qualified message name:

```rust,ignore
use camel_dataformat_protobuf::ProtobufDataFormat;

let df = ProtobufDataFormat::new("protos/helloworld.proto", "helloworld.HelloRequest")?;
```

The constructor compiles the proto file at runtime through `camel-proto-compiler`. Pass a shared `ProtoCache` to `new_with_cache` to reuse the compiled descriptor pool across formats. The cache key covers the root `.proto` and its transitive imports as resolved, so editing an imported file invalidates the cached descriptor.

## Compilation

`camel-proto-compiler` compiles `.proto` files in-process with `protox`, a pure Rust parser. Compilation needs no `protoc` binary and reads no `PROTOC` variable. It writes no temporary file. It works on every platform and in a `FROM scratch` image.

## Precompiled descriptor sets

Every place that accepts a `.proto` path also accepts a precompiled `FileDescriptorSet` file. The extension selects the input: `.binpb`, `.pb`, `.desc`, or `.protoset`. The compiler ignores include paths for this input. The set must contain its imports.

Produce a set with one of these commands:

```console
protoc --include_imports --descriptor_set_out=schema.binpb schema.proto
buf build -o schema.binpb
```

## Limits

Protobuf editions are unsupported. A source file with `edition = "2023"` or a descriptor set with `syntax = "editions"` fails with a typed error. The error says to rewrite the schema as `proto3` or `proto2`. `prost-reflect` 0.16 cannot load editions.

The compiler rejects bracket nesting deeper than 64. It rejects any single schema input larger than 16 MiB, source file or descriptor set alike.

Source imports carry two source-only bounds: at most 256 distinct include-resolved `.proto` files and at most 64 MiB of cumulative source bytes. Embedded well-known types count toward neither bound and do not count toward the source file count. A descriptor set has no separate file-count limit. Its size, import chain and traversal work bound it instead.

Every dependency graph, from source or from a descriptor set, must keep its longest import chain at or below 256 files and its public-import traversal work `W` at or below 100 000. Work is `W = N + sum E(d)`. `N` is the file count. The sum runs over every direct import occurrence `d` of every file, private imports included, and never deduplicates. `E(v) = 1 + sum E(d)` over every public import occurrence of `v`. A source graph must also keep its lifetime `(N + 1) * W` at or below 100 000, because protox builds its internal pool incrementally. These are conservative bounds on traversal calls, not exact counts of every scanning operation.

The compiler preloads the reachable source import closure into an immutable snapshot before it builds any pool. Missing imports stay frozen in that snapshot, and no filesystem or embedded well-known type is read again after validation.

The compiler follows symbolic links, as protoc does. The link target must be a regular file. Any other kind is a typed error.

Syntax and semantic errors in `.proto` sources report the position as `file:line:column`. I/O and descriptor-decode errors do not carry a position.

## Route usage

The protobuf format is not built-in. The YAML DSL resolves it through the `protobuf:<path>#<Message>` data format string:

```yaml
- marshal: "protobuf:protos/helloworld.proto#helloworld.HelloRequest"
```

The `camel-dsl` crate gates this format behind its non-default `protobuf` cargo feature. A route that names `protobuf:` fails at compile time when the feature is off. The proto path must be relative and cannot contain `..`.

## REST usage (raw binding)

A `rest:` operation cannot auto-lower protobuf from `consumes` and `produces`: a media type carries no `<path>#<Message>` selector. Declare `binding: raw` and add explicit `unmarshal:` and `marshal:` steps. The `json` binding rejects any non-JSON media type at route load, so it cannot serve protobuf.

```yaml
rest:
  - host: 0.0.0.0
    port: 8080
    path: /api
    operations:
      - method: POST
        path: /orders/{orderId}
        operation_id: createOrder
        binding: raw
        consumes: application/x-protobuf
        produces: application/x-protobuf
        steps:
          - unmarshal: "protobuf:protos/orders.proto#orders.CreateOrder"
          - to: direct:handleOrder
          - marshal: "protobuf:protos/orders.proto#orders.OrderReceipt"
```

The `raw` binding injects no automatic data-format steps. The explicit `unmarshal` decodes the streamed request, and the explicit `marshal` encodes the response.

Do not add a separate `stream_cache:` step. The DSL wraps every `Unmarshal` step in `StreamCacheService`, which buffers the request `Body::Stream` into bytes before the protobuf unmarshal decodes it. A `stream_cache: true` around the unmarshal is redundant.

The media negotiation gate stays first. An undeclared request `Content-Type` fails with `415`, and an `Accept` mismatch fails with `406`. Declare both `consumes` and `produces`. Path templates such as `{orderId}` keep working unchanged.

`request_schema` and `response.schema` are JSON Schema and are rejected in raw mode: the protobuf schema lives in the `.proto` file, not in the operation. A raw `POST` defaults to `201`; set `success_status` to override.

Raw lowering sets the response `Content-Type` header to the declared `produces` value after the user steps run, and it intentionally overrides an earlier user-set Content-Type. A `do_try`/`catch` error body can therefore carry `application/x-protobuf` even when its bytes are not protobuf, unless the catch branch replaces the body with valid protobuf bytes or a later step overrides the header.

The route must enable the `camel-dsl` `protobuf` cargo feature and register the format; a route that names `protobuf:` fails at compile time otherwise. See [Route usage](#route-usage) for the format string rules (relative path, no `..`) and [Precompiled descriptor sets](#precompiled-descriptor-sets) for descriptor inputs.

## Body type support

| Body type | Marshal | Unmarshal |
| --- | --- | --- |
| `Body::Json` | Encodes to protobuf bytes | Passes through |
| `Body::Text` | Parses as JSON, then encodes | Rejected |
| `Body::Bytes` | Validates and passes through | Decodes to JSON |
| `Body::Empty`, `Body::Stream`, `Body::Xml` | Rejected | Rejected |

## DoS protection

The format rejects payloads larger than 64 MiB by default. The cap prevents out-of-memory errors from oversized inputs. Raise or lower the limit with `with_max_decode_bytes`:

```rust,ignore
let df = ProtobufDataFormat::new("schema.proto", "my.Message")?
    .with_max_decode_bytes(128 * 1024 * 1024);
```

Prost enforces a recursion limit of 100 levels. Deeply nested payloads return `RecursionLimitReached` at depth 100.

**Reference**: [camel-dataformat-protobuf source](https://github.com/kennycallado/rust-camel/blob/main/crates/dataformats/camel-dataformat-protobuf/src/lib.rs)

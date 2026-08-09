# Protobuf

The protobuf data format converts between JSON and binary protobuf wire format. It uses `prost-reflect` for dynamic message descriptors that the format compiles at runtime. The format requires no compile-time code generation. It ships as a separate crate, `camel-dataformat-protobuf`.

Marshal converts `Body::Json` to `Body::Bytes`. Unmarshal reverses the conversion and returns `Body::Json`. The round trip preserves field values through the JSON bridge.

## When to use protobuf

Choose protobuf when the contract is a gRPC service or when the schema must evolve without breaking older clients. Protobuf carries typed fields, forward and backward compatibility, and a compact binary encoding. Choose JSON instead when the consumer is a browser, a REST API, or any system that reads text. JSON is readable, universal, and cheaper to debug. See [Data Formats](index.md) for the full format catalog.

## Construction

`ProtobufDataFormat` takes a proto file path and a fully-qualified message name:

```rust,ignore
use camel_dataformat_protobuf::ProtobufDataFormat;

let df = ProtobufDataFormat::new("protos/helloworld.proto", "helloworld.HelloRequest")?;
```

The constructor compiles the proto file at runtime through `camel-proto-compiler`. Pass a shared `ProtoCache` to `new_with_cache` to reuse the compiled descriptor pool across formats.

The protobuf format is not built-in. Register it before the route starts:

```rust,ignore
ctx.data_format_registry()
    .register("protobuf", std::sync::Arc::new(df));
```

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

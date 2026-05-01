# camel-dataformat-protobuf

> Protobuf DataFormat for rust-camel (JSON <-> protobuf wire format)

## Overview

`camel-dataformat-protobuf` provides a dynamic protobuf data format that marshals JSON to protobuf bytes and unmarshals protobuf bytes back to JSON.

## Features

- Marshal: JSON → protobuf bytes
- Unmarshal: protobuf bytes → JSON
- Integrates with `camel-proto-compiler` for dynamic proto resolution
- Implements `camel_api::DataFormat` trait
- Works with DSL `marshal: protobuf:path#Message` syntax

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-dataformat-protobuf = "*"
```

## URI Format

```
protobuf:path/to/file.proto#package.Message
```

## Usage

```rust
use camel_component_api::Body;
use camel_dataformat_protobuf::ProtobufDataFormat;
use serde_json::json;

let df = ProtobufDataFormat::new("api.proto", "myapp.UserRequest")?;

let body = Body::Json(json!({"name": "Alice"}));
let bytes = df.marshal(body)?;
let back = df.unmarshal(bytes)?;
```

## License

Apache-2.0

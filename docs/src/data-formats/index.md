# Data Formats

A data format converts a message body between a wire representation and a structured type. Each format implements the `DataFormat` trait from `camel-api`. The trait defines a `marshal` operation (body to wire) and an `unmarshal` operation (wire to body). Per [ADR-0030](../adr/0030-exchange-aware-dataformat-hooks.md), the trait also exposes Exchange-aware hooks for formats that read or write Exchange metadata.

The [Marshal and Unmarshal](../eip/marshal-unmarshal.md) EIP page covers route-level usage.

## Available formats

| Format | Crate | Body mapping |
| --- | --- | --- |
| `json` | built-in (`camel-processor`) | Text ↔ Json |
| `csv` | built-in (`camel-processor`) | Text ↔ Json |
| `xml` | built-in (`camel-processor`) | Text ↔ Json |
| `zip` | built-in (`camel-processor`) | Any body → zipped Bytes |
| `protobuf` | [camel-dataformat-protobuf](protobuf.md) | Json ↔ Bytes |

JSON, CSV, XML, and ZIP are registered by default. Protobuf ships as a separate crate.

**Reference**: [DataFormat trait](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-api/src/data_format.rs)

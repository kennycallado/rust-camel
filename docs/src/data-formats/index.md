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
| `tar` | built-in (`camel-processor`) | Any body → single-entry TAR Bytes |
| `gzip` | built-in (`camel-processor`) | Any body → gzip-compressed Bytes |
| `tar.gz` | built-in (`camel-processor`) | Any body → gzipped single-entry TAR Bytes |
| `protobuf` | [camel-dataformat-protobuf](protobuf.md) | Json ↔ Bytes |

JSON, CSV, XML, ZIP, `tar`, `gzip`, and `tar.gz` are registered by default. Protobuf ships as a separate crate.

## Archive formats

`tar` wraps the body bytes as one regular-file entry named `payload`. Unmarshal reads the archive in memory and returns the bytes of the first regular-file entry. Entries that are not regular files (directories, symlinks, hard links, device nodes) are ignored. Entry paths never touch the filesystem. By default, unmarshal fails when the archive holds no regular-file entry or more than one. Set `allow_multi_entry` to true to accept the first regular-file entry; unmarshal then logs a warning when more than one regular-file entry exists.

`gzip` compresses and decompresses materialized body bytes on its own. You can combine `tar` and `gzip`: marshal with `tar` and then with `gzip`, and unmarshal in the reverse order. `tar.gz` output also decodes through `gzip` and then `tar`, and output composed from `tar` and then `gzip` also decodes through `tar.gz`.

`tar.gz` applies the `tar` entry policy to a gzip-compressed TAR stream. Use it when you want the single-entry archive without handling the intermediate TAR bytes.

Each format caps its `marshal` input at `max_input_size` and caps the decoded `unmarshal` output at `max_decompressed_size` before the bytes are materialized. For `gzip` and `tar.gz`, the output cap covers the full decoded stream, including TAR headers, padding, and skipped entries. `compression_level` accepts 0–9. Invalid levels and unknown configuration fields fail closed. For `tar.gz`, the compressed archive gets a fixed TAR framing bound (entry header, payload padding, end-of-archive marker) on top of the raw-body input cap, so the effective raw-body cap stays `max_input_size`. `Body::Empty` and stream bodies are rejected. Zero-length byte and text bodies stay materialized bodies and follow archive semantics.

These formats do not split an archive into one exchange per entry and do not extract entries to disk. Entry-per-exchange splitting stays a splitter concern: `zip_splitter` keeps its current scope, and no TAR splitter exists.

**Reference**: [DataFormat trait](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-api/src/data_format.rs)

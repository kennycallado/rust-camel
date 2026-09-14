# TAR Splitter

The TAR Splitter decomposes a multi-entry TAR archive into one exchange per regular-file entry. Each entry flows through the route as its own message. The TAR.GZ variant applies the same walk to a GZIP-compressed TAR stream and emits identical metadata and bodies for equivalent archives.

## Configuration

YAML selects the format through the streaming split step:

```yaml
- id: tar-split-route
  from: file:data/inbox
  steps:
    - split:
        streaming: true
        stream:
          format: tar
        aggregation: original
        steps:
          - to: log:tar-entry?showBody=true
```

Set `format: tar.gz` for a compressed archive. Other `stream` keys (`max_record_bytes`, `batch_size`, `chunk_size`) keep their streaming-split meaning; the format name accepts only `tar` and `tar.gz` for archives, and an unknown name fails at parse time.

On the API side, a route builder pushes a `DeclarativeStreamSplit` step with `StreamSplitFormat::Tar` or `StreamSplitFormat::TarGz` in the `StreamSplitConfig`. Callers that need custom bounds construct a `TarSplitConfig` and use the `tar_splitter(config)` or `tar_gz_splitter(config)` expressions directly.

## Emitted metadata

The splitter walks the archive in header order and emits regular files sequentially. Each exchange carries the entry body plus headers:

| Header | Meaning |
| --- | --- |
| `CAMEL_TAR_ENTRY_NAME` | File name of the entry, without directory components |
| `CAMEL_TAR_ENTRY_PATH` | Full entry path inside the archive |
| `CAMEL_TAR_ENTRY_INDEX` | Zero-based index of the emitted entry |
| `CAMEL_TAR_ENTRY_SIZE` | Decoded entry size in bytes |
| `CAMEL_TAR_ENTRY_IS_DIRECTORY` | Always `false`; directories are never emitted |

The index counts emitted regular-file entries, not raw archive headers. A directory or link at header position one does not advance the index of the file that follows it. Parent `Content-Length` and `Content-Type` headers are removed from each fragment because they describe the archive, not the entry. Duplicate entry names follow the shared archive `DuplicatePolicy` semantics: deterministic collision-free indexed names by default (`AllowWithIndex`) — an indexed name never reuses a name another entry already emitted — or reject when configured with `Reject`. The ZIP splitter's duplicate handling is unchanged historical behavior: its reader collapses duplicate names, so the policy branches there are dormant.

## Materialization and caps

The splitter materializes the full archive bytes before the walk starts. TAR.GZ input is first bounded by the compressed-input cap and then decoded in memory; the decode is bounded by `max_total_decoded_size` plus a dual-bounded TAR framing allowance — one header block and padding per allowed entry, capped at an absolute 64 MiB ceiling so an absurd entry cap can never make the decode unbounded, plus a 64 KiB constant base that covers GNU longname and PAX extension blocks and the end-of-archive blocks. Framing therefore never counts against the payload cap, and long-name archives are not falsely rejected. Every cap is always on; the values below are the bounded defaults, and each can be set explicitly through `TarSplitConfig`:

| Cap | Default | Scope |
| --- | --- | --- |
| `max_entries` | 10,000 | Emitted regular-file entries |
| `max_per_entry_size` | 512 MiB | Decoded size of one entry |
| `max_total_decoded_size` | 1 GiB | Aggregate decoded payload bytes across entries (TAR framing excluded) |
| `max_compressed_size` | 1 GiB | TAR.GZ compressed input, checked before decode |
| `max_path_length` | 4,096 | Validated entry path length |

A cap violation fails the split with a bounded error before the offending bytes are retained. Unknown configuration fields are rejected, so a deserialized config is always fully bounded.

## Skipped entry kinds

Directories, symlinks, hard links, device entries, GNU contiguous (`b'7'`), and old-GNU sparse (`'S'`) entries do not produce fragments — only plain regular-file entries are emitted, matching tar-rs `is_file()` semantics. The splitter reads header typeflags only, never follows link targets, and never touches the filesystem. An archive with no regular-file entry fails by default; set `allow_empty_archive` to accept zero fragments.

## Path confinement

Every entry name is validated before use: absolute paths, `..` traversal, backslashes, drive prefixes, and over-length names are rejected, with TAR-prefixed error text. Beyond validation, entry paths stay in-memory bytes — the splitter never joins an entry path onto a filesystem path, so no archive can place or follow a symlink on disk. This is the same escape class that rc-0ks57 closed for the file producer through nearest-existing-ancestor confinement of writes to the canonical base; the TAR splitter reaches that boundary earlier, by rejection at validation time plus zero filesystem access, so there is no write path to confine.

## Single-member GZIP

`tar.gz` accepts exactly one GZIP member. A concatenated multi-member stream fails with an explicit unsupported-multi-member error instead of silently decoding only the first member. The standalone `gzip` data format keeps its first-member unmarshal behavior; the rejection applies to TAR.GZ stream splitting only.

## Aggregation

`AggregationStrategy::Original` returns the original exchange after the split scope closes. The split is not a round-trip: the route result body is the original archive, not a reassembly of fragment outputs, and no archive is re-encoded. Use `collect_all` when the route needs the fragment outputs as a list.

The TAR Splitter and the [ZIP Splitter](zip-splitter.md) each parse their own archive format; there is no generic archive-splitting abstraction. Both differ from the [Streaming Splitter](streaming-splitter.md), which splits incremental byte streams such as NDJSON, log lines, or raw chunks. Use the TAR Splitter when the input is a TAR or TAR.GZ archive and you need per-entry metadata.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the splitter compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The per-entry sub-pipeline compiles into child steps on the same route channel. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

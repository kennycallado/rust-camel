# Design: tarsplitter

## Approach

Add a TAR-specific splitter beside `zip_splitter`, sharing only security and cap helpers where that avoids semantic drift. TAR is read sequentially, so emitted regular-file fragments preserve archive order. Each fragment is materialized with a bounded `entry.take(limit + 1)` read; aggregate decoded bytes and compressed input are capped. TAR.GZ wraps the TAR reader in the existing bounded GZIP path and remains single-member in v1.

Each fragment carries TAR name/path/index/size metadata. Index counts emitted regular files only. Fragment headers remove parent `Content-Length` and `Content-Type`; non-regular entries are skipped, and link targets are never followed. Entry names are validated for length, absolute paths, and traversal before any future extraction surface. V1 does not write paths to disk; the spec names the nearest-existing-ancestor confinement precedent from rc-0ks57 for any future extractor.

Use the existing `StreamSplitFormat` route compiler and add `Tar`/`TarGz` variants. Configuration follows the fail-closed bounded-format pattern: explicit caps, `deny_unknown_fields`, and the shared archive duplicate-name policy (`Reject` fails the split; `AllowWithIndex` emits deterministic collision-free indexed names — TAR applies it directly, while ZIP's reader-level duplicate collapse stays pinned historical behavior). Empty archives or directory-only archives emit zero fragments when configured to allow empty archives. Marshal remains a single `payload` entry; split and marshal are not inverses, and `AggregationStrategy::Original` is the supported preservation story.

## Affected crates

- `camel-api`: add stream-split format variants and schema representation.
- `camel-processor`: add TAR/TAR.GZ splitter implementation, shared path validation, caps, metadata, and tests.
- `camel-core`: compile new stream-split formats on bytes, text, and bounded stream paths.
- `camel-dsl`: accept TAR and TAR.GZ stream-split format names and reject unknown values as before.
- `camel-test`: add archive split integration coverage.
- `route-lsp` / `route-lint`: update any closed stream-format lists and diagnostics if present.
- `docs`: document TAR splitter behavior, limits, security boundary, and aggregation semantics.

## Architecture boundaries

The splitter remains a data-plane processor. `camel-api` owns the public format contract, `camel-core` only maps that contract into the existing pipeline, and `camel-processor` owns archive parsing. No runtime control-plane, component, service, language, or filesystem extraction surface is introduced. Security validation follows ADR-0032, ADR-0033, and the rc-0ks57 confinement lineage; outcome and aggregation behavior follows the existing splitter contract and ADR-0019.

## Alternatives considered

- **Generic archive splitter:** rejected for v1. ZIP and TAR have materially different metadata and ordering models; a lowest-common-denominator trait would hide useful guarantees. Reconsider when a third archive format requires it.
- **TAR mirror in a separate module:** selected, with shared path/cap helpers only. This preserves ZIP behavior while preventing semantic coupling.
- **Streaming entry bodies:** rejected for v1 because existing archive splitters materialize bounded bodies and downstream fragment contracts expect ordinary `Body` values.
- **Archive reassembly:** rejected because the current marshal contract accepts one body and cannot express a multi-entry aggregate without a new API.

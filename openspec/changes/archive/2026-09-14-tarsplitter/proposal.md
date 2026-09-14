# Proposal: tarsplitter

## Why

`tar` and `tar.gz` DataFormats currently expose only one regular-file body. Routes cannot process archive entries as individual Exchanges, unlike ZIP routes. This closes bd rc-3sule and completes the deferred archive-splitting surface without adding a new codec dependency.

## What Changes

- Add entry-per-Exchange TAR and TAR.GZ splitting to the existing splitting EIP surface.
- Add bounded configuration, archive-order metadata, duplicate handling, and safe entry-name validation.
- Expose the formats through the existing `StreamSplitFormat` compiler and documentation.
- Keep entry bodies materialized and in memory for v1; do not extract to disk or reassemble archives.
- Preserve ZIP behavior; use a TAR-specific implementation rather than a premature archive abstraction.

## Acceptance criteria

- TAR and single-member TAR.GZ split entries in archive order with bounded per-entry, total, compressed-input, entry-count, and path limits.
- Regular files become fragments; directories and link/device entries are skipped without following targets.
- Traversal and absolute names are rejected, and no entry path reaches the filesystem.
- Existing ZIP split behavior remains unchanged; unit and integration coverage exercise both formats.
- Docs describe ordering, limits, security boundary, and non-round-trip aggregation semantics.

## Risk budget

Acceptable risk is limited to the existing in-memory archive model and the documented TAR library parsing behavior. Out of bounds: disk extraction, per-entry streaming bodies, multi-member gzip, new compression codecs, ZIP64 parity work, and archive-reassembly aggregation.

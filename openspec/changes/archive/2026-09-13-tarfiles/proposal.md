# Proposal: tarfiles

## Why

`camel-processor` supports ZIP data formatting but cannot process the common TAR and TAR.GZ archive forms. Issue `rc-82eq7` requests parity for in-memory archive transformation while preserving the existing data-format contract and its bounded input/decompression behavior.

## What Changes

- Add `tar`, `gzip`, and `tar.gz` data formats to `camel-processor`.
- Marshal one exchange body as one fixed-name TAR entry and unmarshal the first regular-file entry.
- Support standalone gzip so `tar` and `gzip` remain composable; provide `tar.gz` as a direct combined format.
- Add bounded configuration, registration, schema/lint metadata, tests, and format documentation.
- Keep extraction filesystem-free. Do not add a TAR splitter in this change; splitter parity is a separate follow-up because data formatting and entry-to-exchange splitting have different contracts.

## Acceptance criteria

- Routes can resolve `tar`, `gzip`, and `tar.gz` through the built-in data-format registry.
- Round trips, malformed input, empty bodies, stream bodies, multi-entry policy, non-regular entries, and decompression limits are tested.
- TAR/GZIP behavior is bounded and does not follow or write archive paths, symlinks, hardlinks, directories, or device entries.
- Existing format behavior remains unchanged and documentation/schema/context metadata lists the new formats.

## Risk budget

Acceptable: deterministic in-memory buffering matching ZIP's current data-format model; first-regular-entry semantics for TAR. Out of bounds: filesystem extraction, archive splitting, new dependencies, unbounded decompression, or attacker-controlled output paths.

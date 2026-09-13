## ADDED Requirements

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

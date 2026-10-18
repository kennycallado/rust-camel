# bridge-cache-dir-errors Specification

## Purpose
TBD - created by archiving change xmlbridge-cache-hardening. Update Purpose after archive.
## Requirements
### Requirement: Unwritable bridge cache dir fails with an actionable error

When the bridge binary cache directory cannot be written at the point a download would be
required, binary resolution SHALL fail with an error that names the cache path, states the
likely ownership or permission problem, and names a remedy, instead of an opaque
permission failure.

#### Scenario: Root-owned cache dir yields actionable error before download

- **GIVEN** a bridge cache directory that denies writes to the current user (for example
  a directory left owned by root by a previous container run)
- **WHEN** the bridge binary is resolved and is not present in the cache
- **THEN** resolution fails with the actionable cache-unwritable error naming the path and
  the remedy, before any network download is attempted

### Requirement: Writable cache resolution path is unchanged

Binary resolution SHALL behave exactly as before when the cache directory is writable: a
verified cache hit is returned without any write probe, and a cache miss in a writable
directory proceeds to the existing download path.

#### Scenario: Verified cache hit skips the writability probe

- **GIVEN** a valid cached binary with a matching checksum in a writable cache directory
- **WHEN** the bridge binary is resolved
- **THEN** the cached path is returned unchanged and no writability probe is performed

#### Scenario: Cache miss in a writable directory behaves as before

- **GIVEN** a writable cache directory that does not contain the binary
- **WHEN** the bridge binary is resolved
- **THEN** no cache-unwritable error is produced and resolution proceeds to the existing
  download path


## MODIFIED Requirements

### Requirement: Disk payload offload decorator

The system SHALL provide a `DiskOffloadRepository` in `camel-core` that decorates
a registered `CacheRepository` (memory excluded — see the cache_repo
configuration matrix) and stores entry payloads as blob files under the
configured `payload_dir`, while the decorated backend holds a tiny index entry.
Offload SHALL be transparent to the EIP faces: expiry and stale-while-revalidate
semantics stay owned by the decorated backend (the decorator SHALL NOT
re-evaluate expiry; the single in-band check remains the backend's).

`set` SHALL: (1) derive the effective expiry from the supplied `ttl`
(`now + ttl`; when `ttl` is `None` it SHALL fabricate
`expires_at = now + payload_max_ttl` on the stored entry — no-TTL entries behave
as payload_max_ttl-TTL entries under disk offload); (2) write the blob file
FIRST within `payload_dir` to a UNIQUE temporary name per write attempt
(opened with create-new semantics — never a shared destination-derived
tmp name; on a name collision, retry with a fresh nonce), fsync, then rename
onto the destination filename
`<blake3-128-hex(key)>.<death_epoch_secs>.<blake3-128-hex(payload ∥
content_type)>.blob` where
`death_epoch = effective_expires_at + stale_retention + payload_sweep_interval`
(the grace keeps the blob alive at least as long as any backend sweeper or EXAT
keeps the index row; the trailing fingerprint domain-separates the content:
`blake3-128(payload || u8-discriminant(content_type))` — the one-byte enum
discriminant makes the encoding unambiguous) (3) then store the index entry with `bytes` emptied and
`payload_path` set; (4) after the decorated backend accepts the overwrite,
eagerly reclaim the predecessor blob by targeted unlink guided by the pre-swap
row's `payload_path` (see the Requirement: Eager predecessor reclaim on
overwrite). When the blob write fails (e.g. ENOSPC, EIO), `set` SHALL
degrade to inline storage — store the unstripped entry, log a WARN, and return
`Ok(())`: cache writes SHALL NOT introduce a new route-failure mode. Errors from
the decorated backend (including the `"cache: max_entries"` capacity contract)
SHALL propagate unchanged; only the decorator's own file-write errors are
contained. Concurrent same-key `set` calls (multi-replica) are safe without locks:
two writers produce the SAME filename only when key, death epoch, payload
bytes, and content_type all hash alike — identical content, in which case the
files are identical and any surviving index row is coherent. Any differing
component yields a distinct filename (up to a negligible 128-bit collision, a
manifestation of which is a complete-but-stale entry or a clean MISS — never
torn bytes), so the surviving index row references its OWN complete blob; the
superseded predecessor blob is eagerly reclaimed on the next successful
overwrite of the same key, and blobs not reachable by an eager reclaim
(crash-window strays, losing concurrent writers' blobs) are orphans reclaimed
at their own death epoch by the sweeper (last index write wins). Reads observe
a complete, coherent entry or a clean MISS — never
cross-writer pairing of one replica's bytes with another's metadata, never torn
bytes.

`get`/`peek_stale` SHALL re-inject: entries carrying `payload_path` SHALL have
the blob loaded and the bytes restored before returning. `payload_path` values
SHALL be sanitized on read — only a direct child of `payload_dir` is acceptable
(separators, `..`, and absolute paths are corrupt rows). A missing blob file
(early sweep, eager reclaim by a concurrent overwrite, NFS lag, clock skew, foreign reader) or a corrupt index row
(sanitization failure) SHALL return `Ok(None)` with a WARN — NEVER `Err`
(stale-serve resilience takes precedence). An I/O failure on a blob that
EXISTS (EIO, EACCES, EPERM) SHALL surface as `Err` per Contract C1 — a failing
disk is a storage failure, not a miss. Backend failures still surface as
Contract C1 Err as always. Entries without `payload_path` SHALL pass through unchanged,
including entries stored by older inline versions (serde `default` compatibility).

`invalidate` and `invalidate_prefix` SHALL delegate to the decorated backend
only; the returned count SHALL be index-scoped (blobs are reclaimed
asynchronously at their filename-encoded death epoch — after invalidation or
backend-side eviction the blob is an orphan awaiting its epoch). `clear` SHALL
best-effort unlink the `payload_dir` contents (unlink failures SHALL NOT turn
`clear` into `Err`) and then delegate. `stats` and `name` SHALL delegate
unchanged (`CacheStats.bytes` reports the decorated backend's value unchanged;
offloaded entries store an emptied `bytes` field, and redb's accounting sums
entry `bytes` lengths — so each offloaded entry contributes 0, and redis
reports `None`; blob bytes never appear in stats).

#### Scenario: set stores the blob on disk and a bytes-empty index entry

- **GIVEN** a repository decorated with `payload_dir` D holding a 50 KiB entry
  for key `"k"` (ttl 1h, stale_retention 168h, payload_sweep_interval 1h)
- **WHEN** `set("k", entry, Some(1h))` completes and `get("k")` runs
- **THEN** D contains a file named
  `<blake3-128-hex("k")>.<death_epoch>.<blake3-128-hex(payload ∥
  content_type)>.blob`
  with the entry bytes, the decorated backend's index row for `"k"` has empty
  `bytes` and `payload_path` set to that filename, and `get("k")` returns the
  original bytes and content type

#### Scenario: concurrent same-key writers never cross-pair content

- **GIVEN** two `set` calls for the same key with DIFFERENT payloads (or
  content types) completing within the same death-epoch second
- **WHEN** both writes settle and `get("k")` runs
- **THEN** the returned entry's bytes and content type both come from the SAME
  write (the index row references its own fingerprinted blob), the other blob
  is an orphan reclaimed at its own death epoch by the sweeper (the next
  overwrite reads the current row and cannot discover it)

#### Scenario: blob write failure degrades to inline storage

- **GIVEN** a decorated repository whose `payload_dir` cannot be written
  (read-only filesystem)
- **WHEN** `set("k", entry, Some(1h))` runs
- **THEN** `set` returns `Ok(())`, the index row stores the FULL entry bytes
  with `payload_path = None`, a WARN is logged, and `get("k")` returns the bytes

#### Scenario: index-alive file-dead read is a MISS with WARN, never Err

- **GIVEN** a set entry whose blob file is removed underneath the index
  (simulating early sweep, eager reclaim, or NFS lag)
- **WHEN** `get("k")` and `peek_stale("k")` run
- **THEN** both return `Ok(None)` and a WARN is logged — neither returns `Err`

#### Scenario: blob read failure on an existing file surfaces as Err

- **GIVEN** a set entry whose blob file exists but its permissions deny read
- **WHEN** `get("k")` runs
- **THEN** the call returns `Err` (Contract C1: a storage read failure is
  never swallowed as a miss)
#### Scenario: peek_stale re-injects bytes past expiry

- **GIVEN** a set entry with ttl 10ms and 10ms elapsed
- **WHEN** `peek_stale("k")` runs
- **THEN** the post-expiry entry returns with the original bytes re-injected
  from the blob file

#### Scenario: no-TTL entry is capped at payload_max_ttl

- **GIVEN** `payload_max_ttl = 24h` and `set("k", entry, None)`
- **WHEN** the index row and blob filename are inspected
- **THEN** the stored entry carries `expires_at = now + 24h` and the blob's
  death epoch equals that time plus `stale_retention` plus
  `payload_sweep_interval`

#### Scenario: payload_path traversal is rejected without file access

- **GIVEN** an index row (corrupt or foreign) whose `payload_path` is
  `"../../etc/passwd"` or an absolute path
- **WHEN** `get("k")` runs
- **THEN** the call returns `Ok(None)` with a WARN and no file outside
  `payload_dir` is opened

#### Scenario: legacy inline entries pass through

- **GIVEN** an index row stored before this change (JSON without
  `payload_path`, `bytes` populated)
- **WHEN** `get("k")` runs
- **THEN** the entry returns with its stored bytes unchanged

#### Scenario: invalidate delegates and the blob dies at its epoch

- **GIVEN** a set entry under disk offload
- **WHEN** `invalidate("k")` runs, then `get("k")`, then time passes the
  blob's death epoch and the sweeper runs
- **THEN** `get("k")` returns `Ok(None)` immediately, the blob file remains
  until its epoch, and the sweeper unlinks it afterwards

#### Scenario: clear unlinks the payload dir then delegates

- **GIVEN** two set entries under disk offload on a writable local `payload_dir`
- **WHEN** `clear()` runs and the unlinks succeed
- **THEN** the backend's `stats().entries` is 0 and `payload_dir` contains no
  blob files (an unlink failure is swallowed with a WARN — `clear` still
  returns `Ok(())`)

#### Scenario: stats and name delegate to the inner backend

- **GIVEN** a decorated redb-backed repository
- **WHEN** `stats()` and `name()` run
- **THEN** both return the inner backend's values (`name()` = `"persistent"`;
  `CacheStats.bytes` reports the inner backend's value)

## ADDED Requirements

### Requirement: Eager predecessor reclaim on overwrite

The system SHALL reclaim the predecessor blob of a key eagerly, inside the
overwrite `set()` call, by a targeted unlink guided by the index row — no
directory scan. For keys written by one writer at a time, on-disk blob
versions per overwritten key SHALL be bounded at 1 immediately after
`set()` returns WHEN the reclaim unlink succeeds or the predecessor is
already gone (ENOENT); when the unlink fails, the predecessor survives
until its death epoch and the sweeper reclaims it. Under concurrent
same-key writers, losing writers' blobs SHALL be transient orphans
reclaimed by the sweeper at their death epoch — the next overwrite reads
the current row and cannot discover them. The reclaim SHALL be best-effort
and backend-observable: it fires only when the decorated backend's `get`
observes the predecessor row (an expired-but-retained row is invisible to
`get` and is reclaimed by the sweeper at its epoch instead).

The reclaim SHALL read the current row's `payload_path` BEFORE the write,
perform the existing blob write and index swap, and unlink the predecessor
only after the decorated backend accepts the overwrite (`set` returned `Ok`).
If the decorated `set` fails, the reclaim SHALL be skipped — the surviving
row may still reference the predecessor blob. If the pre-swap row read
fails (`get` returns `Err`), the reclaim SHALL be skipped with a WARN and
the write SHALL proceed unchanged — the reclaim never adds a failure mode.
The reclaim SHALL also run when the blob write degrades to inline storage
(the new row no longer references the predecessor). An unlink failure
(EACCES, EIO) SHALL WARN and never turn `set` into `Err`; the predecessor
then dies at its filename-encoded death epoch via the sweeper. The unlink
target SHALL pass the same name guards as reads and sweeps, plus a
key-ownership guard: only a sanitized direct child of `payload_dir`
carrying a parseable death epoch AND the current key's blake3-128 filename
prefix is eligible (foreign files such as operator artifacts, and blobs
belonging to other keys named by a corrupt row, are never unlinked). On
the successful-blob path the fresh blob's own name SHALL never be unlinked
(a same-second identical rewrite yields the same filename); this equal-name
guard SHALL NOT apply on the inline-fallback path, where the failed blob
write leaves no fresh file owning that name. A reader holding a pre-swap
index row while the writer reclaims SHALL hydrate to `Ok(None)` MISS with
WARN — never `Err` (Contract C1 read semantics unchanged).

#### Scenario: overwrite reclaims the predecessor immediately

- **GIVEN** a key `"k"` set twice with different payloads, single writer, ttl
  1h, stale_retention 168h, sweep far in the future, and a writable
  `payload_dir` (the predecessor unlink can succeed)
- **WHEN** the second `set("k", …)` returns `Ok(())`
- **THEN** `payload_dir` contains exactly ONE blob for `"k"`'s key-hash
  prefix (the fresh one) and `get("k")` returns the second payload

#### Scenario: pre-swap row read failure skips the reclaim

- **GIVEN** a key `"k"` with an offloaded predecessor blob, and a decorated
  backend whose `get` returns `Err` (backend failure) at the pre-swap read
- **WHEN** `set("k", …)` runs
- **THEN** the write proceeds unchanged (no reclaim attempted, no new
  failure surfaced from the read), a WARN is logged, and the predecessor
  survives until its death epoch

#### Scenario: reader holding a pre-swap row hydrates to a miss, never an error

- **GIVEN** a set key `"k"` whose index row (payload_path = the old blob) is
  captured before an overwrite
- **WHEN** the overwrite completes (predecessor unlinked) and the captured
  pre-swap row is hydrated
- **THEN** hydration returns `Ok(None)` with a WARN — not `Err` — and a
  subsequent `get("k")` returns the new payload from the fresh blob

#### Scenario: reclaim unlink failure never fails the write

- **GIVEN** an overwritten key whose predecessor blob cannot be unlinked
  (directory permissions deny unlink)
- **WHEN** `set("k", …)` runs
- **THEN** `set` returns `Ok(())`, a WARN is logged, the fresh blob is
  present, and the predecessor survives until its death epoch (sweeper
  reclaims it)

#### Scenario: foreign and unparseable names are never unlinked

- **GIVEN** a `payload_dir` containing an operator artifact (a plain file
  with no parseable death epoch) and a key `"k"` whose index row's
  `payload_path` is a traversal path, a foreign bare name, or ANOTHER
  key's valid blob name (corrupt row)
- **WHEN** `"k"` is overwritten
- **THEN** no file outside the eligible-blob guard set is unlinked, `set`
  returns `Ok(())`, and the artifact and the other key's blob both remain

#### Scenario: same-second identical rewrite keeps the blob

- **GIVEN** a key `"k"` set with payload P, content type C, ttl T, and the
  clock frozen within one second
- **WHEN** `set("k", P, Some(T))` runs again (identical content, same
  death-epoch second — the destination filename is unchanged)
- **THEN** the blob file still exists after the overwrite (the fresh name
  equals the predecessor name and is not unlinked) and `get("k")` returns P

#### Scenario: inner set failure skips the reclaim

- **GIVEN** a decorated repository whose inner backend rejects the index
  write (returns `Err`) after the blob was written
- **WHEN** `set("k", …)` runs on a key with an existing predecessor blob
- **THEN** `set` returns the inner `Err` and the predecessor blob is NOT
  unlinked (the surviving row may still reference it)

#### Scenario: inline fallback reclaims the predecessor

- **GIVEN** a key `"k"` with an offloaded predecessor blob, and a
  `payload_dir` that has become unwritable
- **WHEN** `set("k", …)` degrades to inline storage and returns `Ok(())`
- **THEN** the predecessor blob is unlinked (the new inline row no longer
  references it), or its unlink failure WARNs and the sweeper reclaims it at
  its epoch

#### Scenario: inline fallback with an equal predecessor name reclaims it

- **GIVEN** a key `"k"` set with payload P and ttl T, the clock frozen
  within one second (a retry would produce the SAME destination filename),
  and a `payload_dir` that has become unwritable
- **WHEN** `set("k", P, Some(T))` degrades to inline storage and returns
  `Ok(())`
- **THEN** the unlink is attempted without the equal-name guard — if it
  succeeds or returns ENOENT the predecessor is absent; otherwise it WARNs
  and the predecessor stays for the sweeper at its death epoch (the
  equal-name guard applies only when a fresh blob owns that name, and the
  failed write produced none)

#### Scenario: inline-row predecessor attempts no unlink

- **GIVEN** a key `"k"` whose current index row is inline (`payload_path` is
  `None` — legacy or a prior inline fallback)
- **WHEN** `"k"` is overwritten successfully
- **THEN** no blob unlink is attempted for the predecessor (there is none to
  reclaim) and the write behaves exactly as a first write

#### Scenario: expired-but-retained row is reclaimed by the sweeper, not eagerly

- **GIVEN** a key `"k"` whose entry expired within `stale_retention` (the
  backend's `get` returns miss; `peek_stale` still serves the row)
- **WHEN** `"k"` is overwritten while the old blob's death epoch has not
  passed
- **THEN** the write succeeds and the old blob is not eagerly reclaimed by
  this write (the backend's `get` could not observe the row); the sweeper
  unlinks it at its death epoch

# Proposal: jms-journal-codec

## Why

`bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java` is 1374 lines. Its framing
block — magic/version header, length-prefixed frames, payload encode/decode, torn-tail
handling, and the record-type semantics over `JournalState` — is roughly 250 lines of pure,
I/O-free logic tangled with the service's paths, file locks, fsync ordering, and in-memory
session state. Mission 356 Phase 2 flagged it as a non-blocking extraction candidate
(bd `rc-647gc`).

The wire/disk format is independently pinned by the Rust test reader
`crates/camel-test/tests/support/jms_journal.rs`, which documents it byte for byte
(`RCJN` magic, `u8` version 1, `[u32 payload_len][u8 type][u16 id_len][id utf8][i64
last_touched_ms]`, big endian, torn trailing frame discarded).

## What Changes

- New package-private final class `JournalCodec` in `org.rustcamel.jms` owning, as pure
  functions only: the framing constants (`MAGIC`, `FORMAT_VERSION`, header/payload bounds,
  `TYPE_RESERVE/COMMIT/ROLLBACK/RETIRE`), `header()`, `frameLength()`, `encodeFrame()`,
  `replayBytes()` (decode/replay incl. torn-tail discard), the record-type switch over
  `JournalState`, and the `JournalState`/`Entry`/`ReplayResult` types.
- `JournalService` delegates to `JournalCodec`. Its public API, the on-disk framing format,
  lock ownership, and fsync/durability ordering do not change.
- `JournalService.JournalState` becomes `JournalCodec.JournalState`; all eleven
  `JournalService.JournalState` test references (JournalServiceTest lines 69, 88, 110,
  129, 396, 435, 534, 653, 683, 829, 881) are updated mechanically (assertions
  unchanged). Package-private `JournalService` method signatures whose return types are
  the moved nested types (`replay`, `replayForOpen`) change mechanically to the codec
  types; no public (protected or wider) signature changes.
- New `JournalCodecTest`: a boundary test asserting the codec source contains no
  `FileChannel`, `Files.`, `RandomAccessFile`, `synchronized`, or `FileLock`; a golden-byte
  test pinning exact encoded frames; a replay test over a hand-built frame stream with
  torn tails (truncated length field and truncated body variants).

Explicit non-goals: no format change, no version bump, no new record types, no lock
protocol change, no change to bridge configuration or the Rust reader, no refactors of
unrelated bridge classes.

## Acceptance criteria

- `JournalCodec` exists and holds all framing/state-semantics logic; `JournalService`
  compiles smaller and delegates to it.
- Codec source-text purity test passes; golden-byte encode test passes against the pinned
  layout; torn-tail replay test passes.
- Existing journal tests pass on the base commit and on the extracted tree with equal
  counts on the pre-existing suite (new codec tests excluded from the comparison);
  evidence recorded in the park file.
- `./gradlew test` and the formatter gate pass from `bridges/jms`; `openspec validate`
  passes; no Rust changed.

## Risk budget

Mechanical-move risk only. Acceptable: widening moved `JournalState` members from private
to package-private where the service calls them (same package, no signature changes).
Out of bounds: any byte-level format change, lock-ownership movement, fsync reordering, or
public API change.

Affected crates: none (Rust untouched). Affected area: `bridges/jms` Java bridge only.
Bd: `rc-647gc`.

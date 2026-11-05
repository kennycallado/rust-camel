# Design: jms-journal-codec

## Approach

Pure extraction, no behavior change. One new file
`bridges/jms/src/main/java/org/rustcamel/jms/JournalCodec.java` (package-private final
class, private constructor, same package) receives the framing and state-semantics block;
`JournalService.java` keeps every I/O and lifecycle concern and delegates.

### Moves into `JournalCodec`

- Constants (with their javadoc): `MAGIC`, `FORMAT_VERSION`, `FORMAT_VERSION_OFFSET`,
  `HEADER_LENGTH`, `MIN_PAYLOAD_LENGTH`, `MAX_MESSAGE_ID_BYTES`, `MAX_PAYLOAD_LENGTH`,
  `MAX_REPLAY_BYTES`, `TYPE_RESERVE`, `TYPE_COMMIT`, `TYPE_ROLLBACK`, `TYPE_RETIRE`.
- Pure static functions: `header()`, `frameLength(String)`, `encodedIdLength(String)`
  (private), `encodeFrame(byte, String, long)`, `decodeMessageId(byte[])` (private),
  `replayBytes(byte[])`, and the record-type switch (`applyRecord` — the
  `RESERVE → state.reserve(mid, ms, false)` / `ROLLBACK → state.rollback(mid)` /
  `COMMIT, RETIRE → state.remove(mid)` / unknown → `JournalPermanentException` block).
- Nested types: `JournalState` (with `Entry` and `isStale`), `ReplayResult`.
  `JournalState` is already pure (map operations plus `encodeSnapshot` returning
  `byte[]`), so it belongs with the codec.

### Stays in `JournalService`

All I/O and lifecycle: `replayForOpen` (FileChannel read; delegates the parse to
`JournalCodec.replayBytes`), test-only `replay(Path, long)`, `writeDurable` (+ `force`),
`writeFully`, `journalSize`, lock acquisition/release, `closeJournalChannel`,
`cleanupSnapshot`, `releaseLockOnly`, ceiling checks, `Session`, `JournalHandle`,
`Domain`, `destinationKey`, `DESTINATION_KEY_BYTES`, `DESTINATION_FLOOR_BYTES` (defined
from `JournalCodec.HEADER_LENGTH`), `LOCK_POLL_INTERVAL_MS`. Lock ownership, fsync
ordering, and the public API do not move.

### Mechanical adjustments (behavior-neutral)

- `JournalService` references to moved members qualify them (`JournalCodec.encodeFrame`,
  `JournalCodec.MAX_REPLAY_BYTES`, …) or use single-type imports; no signatures change.
- Visibility: codec members the service calls widen from private to package-private
  (same package, no signature changes): `header()`, `frameLength(String)`,
  `encodedIdLength(String)` (service `requireKeyableId` calls it), `encodeFrame(...)`,
  `replayBytes(byte[])`, and the constants `TYPE_*`, `HEADER_LENGTH`, `MAX_REPLAY_BYTES`
  used at service call sites. `decodeMessageId` stays private (codec-internal only).
  `JournalState.reserve/rollback/remove` widen likewise (3 service call sites in
  `Session`, plus the codec's own `applyRecord` calls). `Entry` fields stay private
  inside the codec. No public (protected or wider) visibility expands.
- Package-private `replay`/`replayForOpen` return types become
  `JournalCodec.JournalState` / `JournalCodec.ReplayResult` mechanically. Test-facing
  forwarding declarations stay in `JournalService` for exactly the moved members the
  existing suite references through `JournalService` (`MAGIC`, `FORMAT_VERSION`,
  `TYPE_RESERVE/COMMIT/ROLLBACK/RETIRE` as constant aliases initialized from the codec,
  and a delegating `static long frameLength(String)`) — imports do not re-export static
  members, so forwarding declarations keep the eleven nested-type edits the only
  test-file change.
- `JournalServiceTest`'s eleven `JournalService.JournalState` references (lines 69, 88,
  110, 129, 396, 435, 534, 653, 683, 829, 881) become `JournalCodec.JournalState` (or an
  equivalent single-type import); all assertions unchanged. No other test edits.

### Byte-sink framing ownership

The codec owns in-memory byte-sink framing only: `encodeFrame` returns the exact frame
bytes; `JournalState.encodeSnapshot` assembles its snapshot by writing codec-produced
arrays into a `ByteBuffer` (in-memory sink). File writes stay in `JournalService`:
`writeDurable` keeps its `FileChannel` append + `force(true)` ordering, `writeFully` its
write loop, and the service wraps `JournalCodec.header()` when priming the channel.
No channel write, fsync, or lock call moves into the codec.

## New tests (all in `JournalCodecTest`)

1. **Boundary (purity)**: read `JournalCodec.java` source text and assert it contains none
   of `FileChannel`, `Files.`, `RandomAccessFile`, `synchronized`, `FileLock`. Source-text
   assertion, not reflection (mission-preferred). The test locates the file relative to the
   Gradle project dir (`src/main/java/...`).
2. **Golden bytes**: `encodeFrame(TYPE_RESERVE, "m1", 1L)` must be exactly
   `00 00 00 0C 01 00 02 6D 31 00 00 00 00 00 00 00 01`, and
   `encodeFrame(TYPE_RESERVE, "order-1", 1712345678901L)` exactly
   `00 00 00 11 01 00 07 6F 72 64 65 72 2D 31 00 00 01 8E AF C1 84 35`; `header()` is
   `52 43 4A 4E 01`. Derived from the Rust reader's documented layout (big endian).
3. **Replay with torn tail**: hand-build header + one full RESERVE frame + a truncated
   trailing frame in two variants — (a) 3 bytes of a length field, (b) a complete length
   field with a truncated body — `replayBytes` returns state with the reservation count
   and `validBytes` = header + first frame only for both. Corruption cases (magic
   mismatch, unsupported version, length out of range) throw
   `JournalPermanentException` and publish no state (no result is returned), matching
   current behavior. Partial header and partial body remain torn-tail discards, not
   corruption.
4. **Byte-sink framing**: write a codec-encoded frame into an in-memory `ByteBuffer`
   sink and assert the sink's contents equal the golden bytes — pinning that frame
   assembly into sinks is codec-owned and channel-free.

## Affected crates

- None (Rust untouched). Java bridge only: `bridges/jms`.

## Architecture boundaries

Isolation inside the Java bridge service layer; no Runtime/DSL/Components surface changes.
The codec/service boundary mirrors the existing Rust-side discipline: `camel-test`'s
`jms_journal.rs` reader already treats framing as a pure concern independent of I/O.

## Phases (optional)

Single-phase change: one coherent extraction slice — move, delegate, boundary + golden +
replay tests, gates.

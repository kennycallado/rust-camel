## ADDED Requirements

### Requirement: Journal framing logic is isolated in a pure codec class

The JMS bridge SHALL own all journal framing and record-state semantics — framing
constants, header construction, frame length computation, record payload encoding,
frame-stream replay including torn-tail discard, and the record-type application over
`JournalState` — inside a single package-private final class `JournalCodec` in
`org.rustcamel.jms` that performs no file I/O, no locking, and no synchronization, while
`JournalService` retains every file I/O, file-lock, and durability-ordering concern and
delegates framing to the codec without any change to its public API.

#### Scenario: Codec source is free of I/O and synchronization tokens

- **GIVEN** the source file `src/main/java/org/rustcamel/jms/JournalCodec.java`
- **WHEN** a test reads the source text and searches it
- **THEN** none of the tokens `FileChannel`, `Files.`, `RandomAccessFile`,
  `synchronized`, or `FileLock` appear

#### Scenario: Service delegates framing without changing its API

- **GIVEN** the extracted `JournalService`
- **WHEN** the journal suite exercises acquire, reserve, commit, rollback, compaction,
  and replay through the unchanged `JournalService` API
- **THEN** every pre-existing test passes unchanged in behavior; its eleven mechanical
  nested-type references (`JournalService.JournalState` → `JournalCodec.JournalState`)
  are updated, package-private `replay`/`replayForOpen` return types become the codec
  types, and references to moved constants resolve via qualified names or forwarding
  declarations — no public (protected or wider) `JournalService` method signature
  changes

### Requirement: Codec framing is byte-for-byte identical to the pinned journal format

The codec SHALL encode and decode journal records exactly as pinned by the independent
Rust reader `crates/camel-test/tests/support/jms_journal.rs`: header `'R' 'C' 'J' 'N'`
followed by `u8` format version `1`; frames of
`[u32 big-endian payload_len][u8 type][payload]` with
`payload = [u16 big-endian id_len][id utf8][i64 big-endian last_touched_ms]`;
`payload_len` bounded to `[10, 10 + 0xFFFF]`; a partial trailing frame discarded as a torn
tail; record types `RESERVE = 1`, `COMMIT = 2`, `ROLLBACK = 3`, `RETIRE = 4` with
`COMMIT` and `RETIRE` removing entries, `ROLLBACK` keeping them, and `RESERVE`
incrementing them.

#### Scenario: Golden encode vectors and in-memory byte sinks

- **GIVEN** the codec's record encoder
- **WHEN** a RESERVE record is encoded for id `"m1"` with last-touched `1`
- **THEN** the encoded frame bytes are exactly
  `0000000C 01 0002 6D31 0000000000000001` (hex), and encoding RESERVE for id `"order-1"`
  with last-touched `1712345678901` yields exactly
  `00000011 01 0007 6F726465722D31 0000018EAFC18435`, and the header bytes are exactly
  `52434A4E 01`, and writing a codec-encoded frame into an in-memory `ByteBuffer` sink
  reproduces the same bytes with no channel or lock involvement

#### Scenario: Replay over a hand-built stream with torn tails

- **GIVEN** a byte stream of the header, one complete RESERVE frame, and a truncated
  trailing frame — first as 3 bytes of a length field, then as a complete length field
  with a truncated body
- **WHEN** the codec replays each stream
- **THEN** the resulting `JournalState` counts the complete reservation and the confirmed
  prefix covers the header plus the complete frame only in both variants, and corruption
  cases — wrong magic, unsupported version, payload length outside the valid range —
  throw `JournalPermanentException` and publish no state to the caller

### Requirement: Extraction preserves journal service behavior

The extraction SHALL NOT change the on-disk framing format, the format version, the
record-type set, the lock ownership protocol, the fsync/durability ordering, or the test
behavior of the journal suite; journal tests SHALL pass with equal counts on the base
commit and on the extracted tree, compared on the pre-existing suite only (new codec
tests excluded from the baseline comparison). The change SHALL NOT touch Rust code,
bridge configuration, or unrelated bridge classes.

#### Scenario: Before/after test equivalence

- **GIVEN** the journal test suite executed on the base commit before extraction
- **WHEN** the same suite executes on the extracted tree
- **THEN** pass/fail counts match the recorded baseline on the pre-existing tests

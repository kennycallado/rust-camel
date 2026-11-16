# Tasks: redisbmp

Additive command surface in `camel-redis`: bitmaps (SETBIT, GETBIT, BITCOUNT,
BITPOS, BITOP, BITFIELD, BITFIELD_RO), HyperLogLog (PFADD, PFCOUNT, PFMERGE)
and WAIT. All arguments are `CamelRedis.*` headers (PFADD's element is the
body per ADR-0098); all argument validation fails before send with named
errors (ADR-0032). Reply mapping uses the existing rules only
(`reply_to_body`, `redis_value_to_body`, integer → `Body::Json(json!(n))`).

Shared conventions for every task below:

- New `RedisCommand` variants (config.rs enum, the `"X" => Ok(...)` parse
  match at ~line 240, and the reverse `=> "X"` match at ~line 415):
  `Setbit`, `Getbit`, `Bitcount`, `Bitpos`, `Bitop`, `Bitfield`,
  `BitfieldRo`, `Pfadd`, `Pfcount`, `Pfmerge`, `Wait` — wire strings
  `SETBIT`, `GETBIT`, `BITCOUNT`, `BITPOS`, `BITOP`, `BITFIELD`,
  `BITFIELD_RO`, `PFADD`, `PFCOUNT`, `PFMERGE`, `WAIT`.
- Every new variant is added to `is_idempotent_command` (config.rs:1902) and
  to the dispatch corpus test in `executor.rs` (~line 2679) and to the
  `dispatch_command` match in `executor.rs` (~line 203).
- `commands/bitmap.rs` (new) hosts SETBIT/GETBIT/BITCOUNT/BITPOS/BITOP and
  the BITFIELD parser; `commands/hll.rs` (new) hosts PF*; WAIT goes in
  `commands/other.rs`. Each module gets a family guard
  (`is_bitmap_command`, `is_hll_command`) mirroring `is_string_command`.
- New header names (use exactly these): `CamelRedis.BitOffset`,
  `CamelRedis.BitValue`, `CamelRedis.BitRangeUnit`, `CamelRedis.BitOperation`,
  `CamelRedis.BitFieldOps`, `CamelRedis.BitFieldOverflow`,
  `CamelRedis.NumReplicas`, `CamelRedis.WaitTimeout`.
- Docs rows go in `docs/src/components/redis.md` per family table
  (`| Command | Required headers | Optional headers | Reply body |`) and are
  part of each task's acceptance: a command without a row is a defect.

## camel-redis: bit access (SETBIT / GETBIT)

### Task 1.1: SETBIT and GETBIT end-to-end

**Files:**
- `crates/components/camel-redis/src/commands/bitmap.rs` (new)
- `crates/components/camel-redis/src/commands/mod.rs` (modified — register
  `pub(crate) mod bitmap;`)
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `docs/src/components/redis.md` (modified)

**Steps:**
1. Add `Setbit` and `Getbit` variants, parse arms, reverse arms, and
   classification: `Getbit` idempotent (read), `Setbit` not idempotent
   (worst-case form governs: it writes a bit).
2. Create `commands/bitmap.rs` with:
   - `pub(crate) fn is_bitmap_command(cmd: &RedisCommand) -> bool` covering
     Setbit/Getbit (extended by later tasks).
   - `pub(crate) fn resolve_bit_offset(exchange: &Exchange) -> Result<u64, CamelError>`
     reading `CamelRedis.BitOffset` via the existing numeric-header helpers;
     missing → `"Missing required header: CamelRedis.BitOffset"`;
     present-but-invalid or outside `0..=4294967295` → a named
     `CamelRedis.BitOffset` error (the 512 MB bitstring cap).
   - `pub(crate) fn resolve_bit_value(exchange: &Exchange) -> Result<u8, CamelError>`
     reading `CamelRedis.BitValue`; only `0` or `1` accepted, otherwise a
     named `CamelRedis.BitValue` error.
   - `pub(crate) fn encode_setbit(key: &str, offset: u64, value: u8) -> redis::Cmd`
     building `SETBIT <key> <offset> <value>`.
   - `pub(crate) fn encode_getbit(key: &str, offset: u64) -> redis::Cmd`
     building `GETBIT <key> <offset>`.
   - `pub async fn dispatch(...)` following the `commands/string.rs` shape:
     `require_key(exchange)?`, resolve args, `query_async` wrapped in
     `crate::transport_error::redis_error_to_camel("SETBIT"/"GETBIT", e)`,
     integer reply mapped to `Body::Json(serde_json::json!(n))` per the
     existing integer-reply pattern (`INCR` family in string.rs).
3. Wire `RedisCommand::Setbit | RedisCommand::Getbit => commands::bitmap::dispatch(...)`
   in `dispatch_command`.
4. Add docs rows for SETBIT and GETBIT.

**Tests:**
- `test_setbit_encodes_wire_frame`: build `encode_setbit("k", 7, 1)` → the
  packed args equal `["SETBIT", "k", "7", "1"]`.
- `test_getbit_encodes_wire_frame`: build `encode_getbit("k", 7)` → packed
  args equal `["GETBIT", "k", "7"]`.
- `test_setbit_rejects_non_binary_bit_value`: dispatch with
  `CamelRedis.BitValue=2` against the existing loopback RESP peer test
  harness (commands/mod.rs tests) → error names `CamelRedis.BitValue` and
  fails before any Redis roundtrip.
- `test_setbit_rejects_missing_bit_offset`: no `CamelRedis.BitOffset` →
  error names `CamelRedis.BitOffset`, before send.
- `test_bit_offset_out_of_range_rejected`: `CamelRedis.BitOffset=4294967296`
  → named `CamelRedis.BitOffset` error, before send.
- `test_is_idempotent_command_bit_access`: `is_idempotent_command(&Getbit)`
  is `true`, `is_idempotent_command(&Setbit)` is `false`.
- command: `cargo test -p camel-component-redis` — expected: each new test
  FAILS before this task's implementation (red), PASSES after.

**Acceptance:**
- All listed tests pass; `cargo test -p camel-component-redis` green.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- Docs rows for SETBIT and GETBIT present in `docs/src/components/redis.md`.

- [x] 1.1

## camel-redis: bitmap ranges (BITCOUNT / BITPOS)

### Task 1.2: BITCOUNT and BITPOS with BYTE|BIT ranges

**Files:**
- `crates/components/camel-redis/src/commands/bitmap.rs` (modified)
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `docs/src/components/redis.md` (modified)

**Steps:**
1. Add `Bitcount` and `Bitpos` variants + parse/reverse arms; classify both
   idempotent (reads).
2. In `bitmap.rs` add:
   - `pub(crate) enum BitRangeUnit { Byte, Bit }` with
     `pub(crate) fn as_str(&self) -> &'static str` (`"BYTE"` / `"BIT"`).
   - `pub(crate) struct BitRange { pub(crate) start: i64, pub(crate) end: Option<i64>, pub(crate) unit: BitRangeUnit }`
     and `pub(crate) fn resolve_bit_range(exchange: &Exchange, verb: &str) -> Result<Option<BitRange>, CamelError>`
     reading `CamelRedis.Start` / `CamelRedis.End` (i64, negative indexes
     from the end) and `CamelRedis.BitRangeUnit` (default `BitRangeUnit::Byte`).
     Header strictness (fail-closed, ADR-0032): a MISSING optional header
     uses the default, but a PRESENT header with a malformed value
     (non-string like `42`, or unknown token like `NIBBLE`) is a named
     `CamelRedis.BitRangeUnit` error before send — never silently treated as
     absent. Use a strict accessor that distinguishes absent from malformed
     (`get_str_header` alone is insufficient: it collapses non-strings to
     absent). Grammar (Redis 7, named errors before send): `BITCOUNT` (verb
     `"BITCOUNT"`) needs `Start` AND `End` together; `BITPOS` (verb
     `"BITPOS"`) may give `Start` alone but the unit requires both
     endpoints; `End` without `Start`, or a unit without any range, is a
     named error.
   - `pub(crate) fn resolve_bit_pos_value(exchange: &Exchange) -> Result<u8, CamelError>`
     — `CamelRedis.BitValue` 0|1, same rule as Task 1.1's
     `resolve_bit_value` (reuse it; do not duplicate).
   - `pub(crate) fn encode_bitcount(key: &str, range: Option<&BitRange>) -> redis::Cmd`
     building `BITCOUNT <key> [<start> <end> <BYTE|BIT>]` — when a range is
     present both endpoints exist and the unit token is ALWAYS emitted
     explicitly (explicitness decision, valid on Redis 7).
   - `pub(crate) fn encode_bitpos(key: &str, value: u8, range: Option<&BitRange>) -> redis::Cmd`
     building `BITPOS <key> <value> [<start> [<end> <BYTE|BIT>]]` with the
     same explicit-unit rule; when `end` is `None` only the start token is
     emitted and no unit token (unit requires both endpoints).
   - dispatch arms mapping the integer reply (`BITPOS` may return `-1`) to
     `Body::Json(json!(n))`.
3. Extend `is_bitmap_command` and wire dispatch arms in `executor.rs`.
4. Add docs rows for BITCOUNT and BITPOS (note: `CamelRedis.Start`/`End`
   signed, unit default `BYTE`, documented — not accidental).

**Tests:**
- `test_bitcount_encodes_range_with_explicit_byte_unit`:
  `encode_bitcount("k", Some(&BitRange { start: 0, end: Some(2), unit: BitRangeUnit::Byte }))` →
  `["BITCOUNT", "k", "0", "2", "BYTE"]`.
- `test_bitcount_encodes_bit_unit`:
  `encode_bitcount("k", Some(&BitRange { start: 0, end: Some(2), unit: BitRangeUnit::Bit }))` →
  `["BITCOUNT", "k", "0", "2", "BIT"]`.
- `test_bitpos_encodes_start_only_without_unit`:
  `encode_bitpos("k", 1, Some(&BitRange { start: 5, end: None, unit: BitRangeUnit::Byte }))` →
  `["BITPOS", "k", "1", "5"]` (start-only: no end, no unit token).
- `test_bitcount_rejects_incomplete_range`: `command=BITCOUNT` with only
  `CamelRedis.Start=0` (and separately only `CamelRedis.End=2`) → named
  error naming `CamelRedis.Start` and `CamelRedis.End` and the BITCOUNT
  both-endpoints rule, before send.
- `test_bitpos_unit_requires_both_endpoints`: `command=BITPOS`,
  `Start=0`, `BitRangeUnit=BIT`, no `End` → named error naming
  `CamelRedis.End`, before send.
- `test_bitrange_unit_rejects_present_malformed_values`:
  `CamelRedis.BitRangeUnit=42` and (separately) `=NIBBLE` with a complete
  range → named `CamelRedis.BitRangeUnit` error before send (NOT silently
  defaulted to `BYTE`); the loopback peer receives zero commands.
- `test_bitpos_negative_range_indexes_allowed`: `BitRange { start: -1, end: Some(-1), unit: Byte }` encodes to
  `["BITPOS", "k", "1", "-1", "-1", "BYTE"]` (signed semantics preserved).
- `test_is_idempotent_command_bitmap_reads`: `Bitcount`/`Bitpos` classify
  idempotent.
- command: `cargo test -p camel-component-redis` — expected: FAIL before
  implementation, PASS after.

**Acceptance:**
- All listed tests pass; `cargo test -p camel-component-redis` green.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- Docs rows for BITCOUNT and BITPOS present.

- [x] 1.2

## camel-redis: bitmap aggregation (BITOP)

### Task 1.3: BITOP with AND|OR|XOR|NOT over N sources

**Files:**
- `crates/components/camel-redis/src/commands/bitmap.rs` (modified)
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `docs/src/components/redis.md` (modified)

**Steps:**
1. Add `Bitop` variant + parse/reverse arms; classify NOT idempotent
   (destination write).
2. In `bitmap.rs` add:
   - `pub(crate) enum BitOpOperation { And, Or, Xor, Not }` with
     `as_str() -> &'static str` (`"AND"`/`"OR"`/`"XOR"`/`"NOT"`) and a
     parser from the `CamelRedis.BitOperation` header value.
   - `pub(crate) fn resolve_bit_operation(exchange: &Exchange) -> Result<BitOpOperation, CamelError>`
     — missing → `Missing required header: CamelRedis.BitOperation`;
     unknown or non-string value (e.g. `NAND` or `42`) → named error listing
     the four valid operations (present-but-malformed is never treated as
     absent).
   - Operand resolution per the SINTERSTORE convention with STRICT
     fail-closed parsing (ADR-0032): `CamelRedis.Destination` via
     `require_str_header` and `CamelRedis.Keys` via `require_string_list(exchange, "CamelRedis.Keys", "BITOP", "key")`
     (the strict SMISMEMBER-style helper — NOT `get_str_vec_header`, which
     silently drops non-string entries). Validation with named errors:
     `NOT` requires exactly one source key (error names `CamelRedis.Keys` and
     the single-source rule); `AND`/`OR`/`XOR` require one or more (empty or
     missing → named `CamelRedis.Keys` error); a non-string element or a
     non-list value → named `CamelRedis.Keys` error before send.
   - `pub(crate) fn encode_bitop(op: BitOpOperation, dest: &str, keys: &[String]) -> redis::Cmd`
     building `BITOP <AND|OR|XOR|NOT> <dest> <key>...`.
   - dispatch arm mapping the integer reply (destination length in bytes)
     to `Body::Json(json!(n))`.
3. Extend `is_bitmap_command`, wire dispatch arm, docs row for BITOP.

**Tests:**
- `test_bitop_or_encodes_wire_frame`: `encode_bitop(Or, "d", ["a","b","c"])` →
  `["BITOP", "OR", "d", "a", "b", "c"]`.
- `test_bitop_not_rejects_multiple_sources`: `BitOperation=NOT`,
  `Destination=d`, `Keys=[a, b]` → named error naming `CamelRedis.Keys` and
  the single-source rule, before send.
- `test_bitop_and_rejects_empty_sources`: `Keys=[]` (or missing) with
  `BitOperation=AND` → named `CamelRedis.Keys` error, before send.
- `test_bitop_rejects_non_string_key_entries`: `Keys=["a", 42]` → named
  `CamelRedis.Keys` error before send (strict list parsing — the exchange
  must not silently execute against fewer sources).
- `test_bitop_rejects_unknown_operation`: `BitOperation=NAND` and
  (separately) `BitOperation=42` → named `CamelRedis.BitOperation` error
  listing AND|OR|XOR|NOT, before send (zero commands sent).
- `test_is_idempotent_command_bitop`: classifies not idempotent.
- command: `cargo test -p camel-component-redis` — expected: FAIL before
  implementation, PASS after.

**Acceptance:**
- All listed tests pass; `cargo test -p camel-component-redis` green.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- Docs row for BITOP present (note the NOT single-key rule).

- [x] 1.3

## camel-redis: BITFIELD

### Task 2.1: BITFIELD and BITFIELD_RO end-to-end

**Files:**
- `crates/components/camel-redis/src/commands/bitmap.rs` (modified)
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `docs/src/components/redis.md` (modified)

**Steps:**
1. In `bitmap.rs` add the BITFIELD grammar (pure functions):
   - `pub(crate) enum BitFieldOverflow { Wrap, Sat, Fail }` with
     `as_str() -> &'static str` and parsing from `WRAP|SAT|FAIL`.
   - `pub(crate) struct BitFieldType { pub(crate) signed: bool, pub(crate) width: u8 }`
     and `pub(crate) fn parse_bitfield_type(token: &str) -> Result<BitFieldType, CamelError>`
     — `[iu]<width>`; signed widths 1..=64, unsigned 1..=63; `u64` rejected
     with the named rule "u64 is not supported but i64 is"; anything else is
     a named `CamelRedis.BitFieldOps` error.
   - `pub(crate) enum BitFieldOffset { Absolute(u64), Sharp(u64) }` with
     `pub(crate) fn bit_offset(&self, width: u8) -> Result<u64, CamelError>`:
     `Absolute(n)` returns `n` (must fit i64 — reject above `i64::MAX`);
     `Sharp(n)` returns `n × width` computed with checked multiplication —
     each `#<n>` is evaluated INDEPENDENTLY as `n × <this entry's width>`
     (the Redis rule: the multiplier is the current entry's encoding width,
     never a running position; two `GET u8 #1` entries address the same
     bits); overflow → named `CamelRedis.BitFieldOps` error.
   - `pub(crate) enum BitFieldEntry { Overflow(BitFieldOverflow), Get { ty: BitFieldType, offset: BitFieldOffset }, Set { ty: BitFieldType, offset: BitFieldOffset, value: i64 }, Incrby { ty: BitFieldType, offset: BitFieldOffset, increment: i64 } }`
     and `pub(crate) fn parse_bitfield_ops(raw: &[String]) -> Result<Vec<BitFieldEntry>, CamelError>`
     tokenizing each list element on whitespace: `GET <type> <offset>`,
     `SET <type> <offset> <value>`, `INCRBY <type> <offset> <increment>`
     (value/increment parsed as i64 — out of i64 or malformed is a named
     error), or `OVERFLOW <WRAP|SAT|FAIL>`. Unknown ops, bad arity, bad
     types and bad offsets are named `CamelRedis.BitFieldOps` errors.
   - `pub(crate) fn validate_bitfield_overflow(entries: &[BitFieldEntry], initial: Option<BitFieldOverflow>, read_only: bool) -> Result<(), CamelError>`
     enforcing: (a) every `Set`/`Incrby` entry has an EFFECTIVE overflow
     mode in force from the `initial` mode or an `Overflow` entry earlier in
     the list — a trailing `OVERFLOW` does NOT retroactively cover a
     preceding mutation; (b) `read_only=true` (BITFIELD_RO) accepts only
     `Get` entries and rejects `Overflow`, `Set`, `Incrby` entries and a
     present `initial` mode (the header), each with named errors
     (`CamelRedis.BitFieldOps` / `CamelRedis.BitFieldOverflow`).
2. Add `Bitfield` and `BitfieldRo` variants + parse/reverse arms in
   `config.rs`; classify: `Bitfield` NOT idempotent (worst-case form
   governs — it may SET/INCRBY), `BitfieldRo` idempotent (read-only).
3. In `bitmap.rs` add the dispatch layer:
   - `pub(crate) fn resolve_bitfield_overflow(exchange: &Exchange) -> Result<Option<BitFieldOverflow>, CamelError>`
     reading `CamelRedis.BitFieldOverflow` (optional: absent → `None`, but a
     PRESENT header with a non-string or unknown value — `42`, `SOMETIMES` —
     is a named `CamelRedis.BitFieldOverflow` error before send, even when an
     inline `OVERFLOW` entry exists; present-but-malformed is never silently
     treated as absent).
   - `pub(crate) fn encode_bitfield(key: &str, initial: Option<BitFieldOverflow>, entries: &[BitFieldEntry], read_only: bool) -> redis::Cmd`
     building `BITFIELD`/`BITFIELD_RO <key> [OVERFLOW <WRAP|SAT|FAIL>] <ops...>`:
     if `initial` is present, an `OVERFLOW <mode>` token is emitted BEFORE
     the entry list; entry tokens are translated verbatim (`#<n>` offsets
     sent back in `#<n>` form — send what was validated, the server
     computes the same independent `n × width`).
   - dispatch arms for `Bitfield`/`BitfieldRo`: `require_key`, read
     `CamelRedis.BitFieldOps` via `require_string_list(exchange,
     "CamelRedis.BitFieldOps", "BITFIELD"/"BITFIELD_RO", "operation")`,
     run the step-1 parser + `validate_bitfield_overflow`, encode,
     `query_async` with `redis_error_to_camel("BITFIELD"/"BITFIELD_RO", e)`,
     reply `Vec<Option<i64>>` mapped to `Body::Json(serde_json::json!(vec))`
     (numbers and nulls per the existing `redis_value_to_body` rules).
4. Extend `is_bitmap_command`, wire dispatch arms in `executor.rs`, add docs
   rows for BITFIELD and BITFIELD_RO (document the explicit-overflow
   decision: no default, fail-before-send, and the `#<n>` = `n × width`
   rule).

**Tests:**
- `test_parse_bitfield_type_accepts_i64_and_u63_rejects_u64`: `i64` and `u63`
  parse; `u64` errors with the named "u64 is not supported but i64 is" rule;
  `x8`, `u0`, `u65`, `i65` error.
- `test_bitfield_sharp_offset_multiplies_entry_width`: entries
  `["GET u8 #1", "GET i16 #1"]` → `bit_offset` results 8 and 16 (each entry's
  own width).
- `test_bitfield_repeated_sharp_offsets_address_same_bits`: entries
  `["GET u8 #1", "GET u8 #1"]` → both offsets are 8 (independent
  evaluation, matches the spec scenario for bytes `[0,1,2]` → `[1,1]`).
- `test_bitfield_rejects_offset_multiplication_overflow`: entry
  `["INCRBY i64 #9223372036854775807 1"]` → named `CamelRedis.BitFieldOps`
  error, before send.
- `test_bitfield_requires_explicit_overflow_before_mutation`: entries
  `["INCRBY u8 #0 1"]`, no initial mode → named
  `CamelRedis.BitFieldOverflow` error.
- `test_bitfield_trailing_overflow_does_not_cover_mutation`: entries
  `["INCRBY u8 0 1", "OVERFLOW SAT"]`, no initial mode → named
  `CamelRedis.BitFieldOverflow` error.
- `test_bitfield_ro_rejects_mutations_and_overflow_header`: read_only=true
  with a `Set` entry, with an `OVERFLOW` entry, and with a present header →
  each a named error.
- `test_parse_bitfield_rejects_malformed_entries`: `["FOO u8 0"]`,
  `["GET u8"]`, `["GET u8 abc"]`, `["SET u8 0 notanint"]` → named
  `CamelRedis.BitFieldOps` errors.
- `test_bitfield_wire_frame_emits_header_overflow_first`: header `WRAP`,
  entries `["GET u8 0", "SET i16 #8 -1", "INCRBY u4 #0 3"]` →
  `["BITFIELD", "k", "OVERFLOW", "WRAP", "GET", "u8", "0", "SET", "i16",
  "#8", "-1", "INCRBY", "u4", "#0", "3"]`.
- `test_bitfield_reply_maps_ints_and_nils_to_json_array`: reply
  `[Some(1), None, Some(-3)]` → body `Body::Json([1, null, -3])`.
- `test_bitfield_ro_wire_frame`: header absent, entries `["GET u8 #1"]` →
  `["BITFIELD_RO", "k", "GET", "u8", "#1"]`.
- `test_bitfield_dispatch_rejects_missing_overflow_before_send`: dispatch
  `command=BITFIELD` with `["INCRBY u8 #0 1"]` and no overflow source →
  named `CamelRedis.BitFieldOverflow` error before any roundtrip (loopback
  peer harness).
- `test_bitfield_overflow_header_rejects_present_malformed_values`:
  `CamelRedis.BitFieldOverflow=42` and (separately) `=SOMETIMES`, each with
  inline `["OVERFLOW SAT", "INCRBY u8 #0 1"]` entries → named
  `CamelRedis.BitFieldOverflow` error before send (a malformed header is
  never ignored); the loopback peer receives zero commands.
- `test_is_idempotent_command_bitfield`: `Bitfield` → false,
  `BitfieldRo` → true.
- command: `cargo test -p camel-component-redis` — expected: FAIL before
  implementation, PASS after.

**Acceptance:**
- All listed tests pass; `cargo test -p camel-component-redis` green.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- Docs rows for BITFIELD and BITFIELD_RO present with the overflow decision.

- [x] 2.1

## camel-redis: HyperLogLog

### Task 3.1: PFADD, PFCOUNT and PFMERGE end-to-end

**Files:**
- `crates/components/camel-redis/src/commands/hll.rs` (new)
- `crates/components/camel-redis/src/commands/mod.rs` (modified — register
  `pub(crate) mod hll;`)
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `docs/src/components/redis.md` (modified)

**Steps:**
1. Add `Pfadd`, `Pfcount`, `Pfmerge` variants + parse/reverse arms;
   classify: `Pfadd`/`Pfmerge` not idempotent (writes), `Pfcount` idempotent
   (its single-key form may refresh the HLL's cached cardinality — a
   converging internal write; the multi-key form only reads its sources).
2. Create `commands/hll.rs` with:
   - `pub(crate) fn is_hll_command(cmd: &RedisCommand) -> bool`.
   - `pub(crate) fn resolve_pf_keys(exchange: &Exchange, verb: &str) -> Result<Vec<String>, CamelError>`
     reading `CamelRedis.Keys` via `require_string_list(exchange, "CamelRedis.Keys", verb, "key")`
     (strict fail-closed parsing — NOT `get_str_vec_header`, which silently
     drops non-string entries); empty or missing → named `CamelRedis.Keys`
     error naming the ≥1-key rule; a non-string element → named
     `CamelRedis.Keys` error before send.
   - `pub(crate) fn encode_pfadd(key: &str, element: &[u8]) -> redis::Cmd`,
     `pub(crate) fn encode_pfcount(keys: &[String]) -> redis::Cmd`
     (`PFCOUNT <key>...`), `pub(crate) fn encode_pfmerge(dest: &str, keys: &[String]) -> redis::Cmd`
     (`PFMERGE <dest> <key>...`).
   - dispatch arms: `Pfadd` takes the element from the BODY via
     `body_value_bytes(exchange, "PFADD", tombstones)` (ADR-0098 — the
     tombstoned payload headers `CamelRedis.Value`/`Values`/`Member` are
     rejected by that helper); `Pfcount` uses `resolve_pf_keys` (single-key
     is a one-element list); `Pfmerge` uses `require_str_header(exchange, "CamelRedis.Destination")`
     plus `resolve_pf_keys` (≥1 source key), the destination+sources
     convention. Replies: `PFADD` integer → `Body::Json(json!(n))`;
     `PFCOUNT` integer → `Body::Json(json!(n))`; `PFMERGE` status →
     `Body::Text("OK")` per the status-reply rule.
3. Wire dispatch arms in `executor.rs`; docs rows for PFADD, PFCOUNT,
   PFMERGE.

**Tests:**
- `test_pfadd_uses_body_element_wire_frame`: text body `café` →
  `["PFADD", "k", "café"]` (UTF-8 body bytes, body-only contract).
- `test_pfadd_rejects_tombstoned_value_header`: `CamelRedis.Value` header
  present → the named "removed in 0.69; put the value in the body" error,
  before send.
- `test_pfcount_multi_key_wire_frame`: `CamelRedis.Keys=[h1, h2]` →
  `["PFCOUNT", "h1", "h2"]`.
- `test_pfcount_rejects_empty_keys`: `CamelRedis.Keys=[]` (or missing) →
  named `CamelRedis.Keys` error naming the ≥1-key rule, before send.
- `test_pfcount_rejects_non_string_key_entries`: `CamelRedis.Keys=["h1", 42]`
  → named `CamelRedis.Keys` error before send (strict list parsing).
- `test_pfmerge_wire_frame`: `Destination=h3`, `Keys=[h1, h2]` →
  `["PFMERGE", "h3", "h1", "h2"]`.
- `test_pfmerge_rejects_missing_destination`: no `CamelRedis.Destination` →
  named error, before send.
- `test_is_idempotent_command_hll`: `Pfcount` → true; `Pfadd`, `Pfmerge` →
  false.
- `test_pfadd_reply_maps_integer_to_json_number`: successful `PFADD` dispatch
  (stubbed successful reply `Int(1)`, using the existing test stub
  infrastructure) → body is `Body::Json(json!(1))`.
- `test_pfcount_reply_maps_integer_to_json_number`: successful `PFCOUNT`
  dispatch (stubbed `Int(3)`) → body is `Body::Json(json!(3))`.
- `test_pfmerge_reply_maps_status_to_text`: successful `PFMERGE` dispatch
  (stubbed status reply `Okay`/`"OK"`) → body is `Body::Text("OK")` (the
  spec scenario "PFMERGE merges into the destination" requires the status
  reply to become the body as text).
- command: `cargo test -p camel-component-redis` — expected: FAIL before
  implementation, PASS after.

**Acceptance:**
- All listed tests pass; `cargo test -p camel-component-redis` green.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- Docs rows for PFADD, PFCOUNT, PFMERGE present.

- [x] 3.1

## camel-redis: WAIT durability probe

### Task 4.1: WAIT with bounded aggregate deadline

**Files:**
- `crates/components/camel-redis/src/commands/other.rs` (modified)
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `crates/components/camel-redis/src/producer.rs` (modified)
- `docs/src/components/redis.md` (modified)

**Steps:**
1. Add `Wait` variant + parse/reverse arms; classify idempotent (barrier
   probe, no server state change; a replay re-probes the current
   connection's offset and may return a different count — count-style
   reply, `DEL` precedent).
2. In `commands/other.rs` add:
   - `pub(crate) fn resolve_num_replicas(exchange: &Exchange) -> Result<u64, CamelError>`
     reading `CamelRedis.NumReplicas`; missing → named error; above
     `i64::MAX` (Redis parses `WAIT`'s arguments as signed 64-bit) → named
     error naming the wire range.
   - `pub(crate) fn resolve_wait_timeout(exchange: &Exchange) -> Result<u64, CamelError>`
     reading `CamelRedis.WaitTimeout` (milliseconds — distinct from the TTL
     header `CamelRedis.Timeout`); missing or `0` (Redis `WAIT ... 0` blocks
     forever) → named error; above `i64::MAX` → named wire-range error.
   - `pub(crate) fn encode_wait(num_replicas: u64, timeout_ms: u64) -> redis::Cmd`
     building `WAIT <numreplicas> <timeout>`.
   - dispatch arm mapping the integer reply (acked replicas) to
     `Body::Json(json!(n))`; extend the existing family guard
     `is_other_command` in `commands/other.rs` to cover `Wait` (the guard
     rejects unknown commands before the per-command match — omitting it
     makes WAIT unreachable) and cover the extension with a guard test.
   - extend `build_redis_cmd` in `commands/other.rs` with a
     `RedisCommand::Wait` arm that resolves `CamelRedis.NumReplicas` /
     `CamelRedis.WaitTimeout` through the resolvers above and returns
     `encode_wait(...)` — the trailing `_ => unreachable!(...)` arm must
     never fire for `Wait` (today it would panic).
3. In `producer.rs` add
   `pub(crate) fn wait_aggregate_timeout(wait_timeout_ms: u64, endpoint_timeout_secs: u64) -> Result<std::time::Duration, CamelError>`
   computing `Duration::from_millis(wait_timeout_ms) +
   Duration::from_secs(endpoint_timeout_secs)` with CHECKED duration
   arithmetic (`checked_add`) — overflow → named error naming
   `CamelRedis.WaitTimeout`, never a panic. In the producer's WAIT path
   (`RedisProducer::call`, around the call to `execute_with_retry`), resolve
   the deadline instant with `tokio::time::Instant::now().checked_add(duration)`
   (a duration that fits `Duration` but not `Instant` is still
   unrepresentable — that check is required, `Duration::checked_add` alone
   is insufficient) and enforce it with `tokio::time::timeout_at`; a
   `None` from `checked_add` fails before send with the same named error. On
   expiry return the named error `"WAIT: aggregate deadline exceeded
   (CamelRedis.WaitTimeout + endpoint timeout)"`. The deadline covers
   connection checkout, every retry attempt, backoff and replies
   (`execute_with_retry` alone is not bounded by
   `RedisEndpointConfig::timeout`). A server-side `WAIT` that outlives the
   deadline is harmless (read-only probe); release/cleanup behavior is
   unchanged.
4. Executor deadline policy: extend the blocking-endpoint deadline bypass in
   `executor.rs` (the `is_blocking_endpoint` policy that keeps blocking
   commands off the driver's default response timeout — canon requirement
   "Blocking deadline bypass") to cover `Wait`. WAIT must be deadline-free
   at the driver layer: the driver's default response timeout (500 ms) would
   make a valid `WAIT 1 1000` (which legitimately blocks up to 1000 ms
   server-side and then returns its count) fail locally and retry. WAIT's
   only local bound is the producer aggregate deadline from step 3 (plus the
   server-side `CamelRedis.WaitTimeout`).
5. Wire dispatch arm in `executor.rs`; docs row for WAIT (reply = acked
   replicas as number; scope = connection-local acknowledgement probe, not
   a durability guarantee for earlier route writes; idempotent; bounded).

**Tests:**
- `test_wait_encodes_wire_frame`: `encode_wait(1, 1000)` →
  `["WAIT", "1", "1000"]`.
- `test_wait_rejects_zero_timeout`: `CamelRedis.WaitTimeout=0` → named
  `CamelRedis.WaitTimeout` error, before send.
- `test_wait_rejects_missing_headers`: missing `CamelRedis.NumReplicas` or
  `CamelRedis.WaitTimeout` → named errors, before send.
- `test_wait_rejects_values_above_wire_range`: `CamelRedis.WaitTimeout=u64::MAX`
  and (separately) `CamelRedis.NumReplicas=u64::MAX` → named errors naming
  the header and the signed-64-bit wire range, before send.
- `test_wait_aggregate_timeout_checked_overflow`:
  `wait_aggregate_timeout(u64::MAX, u64::MAX)` (or any combination that
  overflows `Duration`) → `Err` naming `CamelRedis.WaitTimeout`; a normal
  combination returns the exact summed `Duration`.
- `test_wait_deadline_instant_unrepresentable`: a `Duration` that fits
  `Duration::checked_add` but not `Instant::checked_add` (e.g. endpoint
  `timeout` of `u64::MAX` seconds) → the producer path fails before send
  with the named `CamelRedis.WaitTimeout` error; no panic.
- `test_wait_times_out_against_stalled_peer`: through `RedisProducer::call`
  (the deadline wraps the producer path, not the raw command dispatch) with
  the existing executor stub (`crate::executor::test_stub`, as used by
  `producer.rs` tests) configured to accept the request and never reply;
  `command=WAIT` with a small deadline → the call fails with the named
  "WAIT: aggregate deadline exceeded" error naming `CamelRedis.WaitTimeout`;
  the test itself completes (bounded).
- `test_wait_deadline_bounds_producer_call_with_retries`: executor stub
  failing every attempt with a transport error (triggering the bounded retry
  loop and backoff); `command=WAIT` → `RedisProducer::call` returns within
  the aggregate deadline with the named "WAIT: aggregate deadline exceeded"
  error and does not keep retrying past the deadline.
- `test_is_idempotent_command_wait`: classifies idempotent.
- `test_build_redis_cmd_wait_ok`: `build_redis_cmd` with valid
  `NumReplicas=1`/`WaitTimeout=1000` headers → `Ok` command with args
  `["WAIT", "1", "1000"]` (no `unreachable!` panic).
- `test_build_redis_cmd_wait_invalid_headers_no_panic`: `build_redis_cmd`
  with `CamelRedis.WaitTimeout=0` (or missing) → `Err` naming
  `CamelRedis.WaitTimeout`; the `unreachable!` arm is never reached.
- `test_wait_bypasses_driver_response_timeout`: a successful `WAIT` whose
  reply arrives after the driver's default response timeout (500 ms) but
  within `CamelRedis.WaitTimeout` + endpoint timeout → `RedisProducer::call`
  SUCCEEDS and the body is `Body::Json(json!(n))` (acked replicas) — pin the
  deadline-free driver policy AND the reply-body mapping (use the existing
  fake-server/blocking-test time pattern; no raw sleeps — `lint-test-sleep`
  gate).
- `test_wait_selects_blocking_deadline_policy`: `Wait` is treated as a
  blocking endpoint by the executor's deadline selection (no
  `Deadline::DriverDefault` response timeout applies to it).
- command: `cargo test -p camel-component-redis` — expected: FAIL before
  implementation, PASS after.

**Acceptance:**
- All listed tests pass; `cargo test -p camel-component-redis` green.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- Docs row for WAIT present with the connection-local scope note.

- [x] 4.1

## camel-test: live tests

### Task 5.1: live tests per family against redis:7-alpine

**Files:**
- `crates/camel-test/tests/redis_test.rs` (modified)

**Steps:**
1. Add live tests following the existing patterns in `redis_test.rs`
   (`#![cfg(feature = "integration-tests")]`, `tests/support/redis.rs`
   `shared_redis()` returning `127.0.0.1:{port}`, routes as
   `redis://{conn}?command=...` with headers via `set_header`, asserted
   through mock endpoints). One representative command per family:
   - bitmap access + range: SETBIT then GETBIT/BITCOUNT with a BIT range,
   - BITOP NOT over one source,
   - BITFIELD with explicit `OVERFLOW SAT` and a mixed GET/INCRBY entry
     list (assert the reply array of numbers/nulls),
   - PFCOUNT over two PFADD-ed keys (multi-key),
   - WAIT with `CamelRedis.NumReplicas=1`, `CamelRedis.WaitTimeout=1000`
     (assert the reply body is the acked-replica count as a number).
2. Each live test asserts the reply BODY per the body-only contract (number
   / array / text) and the wire effect on Redis (e.g. GETBIT reads back the
   set bit; BITOP destination length). Every test uses fresh, isolated keys
   (unique per test) so assertions are exact, not fuzzy.

**Tests:**
- `redis_live_setbit_getbit_roundtrip`: SETBIT `k` offset 7 bit 1 → GETBIT
  returns 1 (body JSON number 1).
- `redis_live_bitcount_bit_range`: after `SETBIT k 7 1`, `BITCOUNT k 7 7 BIT`
  body is exactly 1 (offset 7 is the only set bit — do NOT scan `0 0 BIT`,
  which examines only offset 0 and would be 0).
- `redis_live_bitop_not_single_key`: `BITOP NOT d a` body is the byte
  length of `d`.
- `redis_live_bitfield_overflow_sat_reply_array`: `BITFIELD` with
  `CamelRedis.BitFieldOverflow=SAT` and entries
  `["SET u8 #0 255", "INCRBY u8 #0 1", "GET u8 #0"]` → body
  `[0, 255, 255]` (SET returns the old value 0; SAT saturates at 255).
- `redis_live_pfcount_multi_key`: PFADD `h1`/`h2` distinct elements →
  PFCOUNT `Keys=[h1,h2]` body ≥ 2.
- `redis_live_wait_reply_body`: WAIT 1/1000 → body is JSON number 0 or 1
  (single-replica test container).
- command: `cargo test -p camel-test --features integration-tests --test redis_test` —
  expected: FAIL before implementation, PASS after (requires Docker and the
  `redis:7-alpine` image).

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test redis_test` exits 0.
- `cargo test -p camel-test --features integration-tests --test redis_repositories_test` exits 0 (no regression).
- `cargo test -p camel-component-redis` green.

- [x] 5.1

## docs: decisions and completeness

### Task 6.1: document BITFIELD/WAIT decisions and audit the command table

**Files:**
- `docs/src/components/redis.md` (modified)

**Steps:**
1. Add a short "Phase 1b part 2 decisions" note to `docs/src/components/redis.md`:
   - BITFIELD overflow is always explicit: `CamelRedis.BitFieldOverflow`
     (`WRAP|SAT|FAIL`) sets the initial mode and inline `OVERFLOW` entries
     switch it per-op; a `SET`/`INCRBY` without an effective mode fails
     before send (no default — the server default `WRAP` silently wraps
     counters and must never apply by accident).
   - WAIT is a connection-local acknowledgement probe: it acknowledges the
     writes sent on the invoking connection up to the `WAIT` command; a
     different pooled connection or a reconnect resets that scope, so it is
     NOT a durability guarantee for earlier route writes. Reply = acked
     replicas as a JSON number. Idempotent (barrier, safe to replay).
     Bounded by `CamelRedis.WaitTimeout` (> 0, ms, required) and a
     WAIT-only aggregate deadline of `WaitTimeout + endpoint timeout`; it
     can never hang forever.
   - `#<n>` BITFIELD offsets are `n × <entry width>` evaluated
     independently (never accumulated).
2. Audit `docs/src/components/redis.md`: exactly one row per command —
   SETBIT, GETBIT, BITCOUNT, BITPOS, BITOP, BITFIELD, BITFIELD_RO, PFADD,
   PFCOUNT, PFMERGE, WAIT — each listing required/optional headers and the
   reply body. Every new header name appears in the header column of at
   least one row or in the note.

**Tests:**
- `docs_table_lists_all_eleven_commands`: grep the table for one row per
  command name — assert 11 rows matching the exact command names.
- command: `cd docs && mdbook test` — expected: FAIL if any doc snippet is
  broken; PASS after.

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- All 11 commands have exactly one row in `docs/src/components/redis.md`.
- No placeholder or unfinished markers in the docs additions.

- [x] 6.1

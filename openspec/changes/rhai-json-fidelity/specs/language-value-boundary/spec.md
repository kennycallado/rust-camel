# language-value-boundary

Delta for change `rhai-json-fidelity`. Every requirement below is scoped to the
Rhai host JSON helper functions (`parse_json`, `to_json`) and the JSON value
wrapper they use. The exchange inbound conversion (`json_to_dynamic`) and scope
preparation remain unchanged; the only converter change is two outbound refusal
arms in `dynamic_to_value`, so no existing requirement is modified.
Authority: bd rc-qka42.

Scope note: the implementation uses a private raw-token tree plus
`indexmap::IndexMap` and descends with `serde_json::value::RawValue`. It does
NOT enable serde_json `arbitrary_precision` or `unbounded_depth`. `raw_value` and
`preserve_order` (both already workspace-unified) plus `indexmap` are declared
per crate; no workspace-wide feature is added.

## ADDED Requirements

### Requirement: Rhai JSON helpers shadow the stock built-ins from a shared module

The Rhai sandbox SHALL build one process-wide host module containing the JSON
wrapper type, functions, and indexers, and SHALL register it globally on every
engine it builds after the standard package, so its functions take precedence
over the engine's stock `parse_json` and `to_json`. The host functions SHALL
read `max-string-size`, `max-array-size`, and `max-map-size` from the calling
engine, not from captured state.

#### Scenario: Host helpers win on every engine

- **GIVEN** an engine built by the rhai sandbox
- **WHEN** a script calls `parse_json` or `to_json`
- **THEN** the host implementation SHALL run, not the stock built-in, on the
  read-only, mutating, and expression engines alike

### Requirement: Rhai JSON parsing is strict RFC 8259

`parse_json` SHALL accept the full RFC 8259 grammar for any JSON value (object,
array, or scalar), including the `\/` escape, EXCEPT that an unpaired surrogate
escape SHALL be refused. Malformed input, including unpaired surrogates, SHALL
fail with a typed `parse` error that contains no parsed input text and no
parser line or column; the error SHALL carry the script call position instead.

#### Scenario: Escaped solidus is accepted

- **GIVEN** the JSON text `{"path":"a\/b"}`
- **WHEN** a script evaluates `parse_json` on it
- **THEN** parsing SHALL succeed and the `path` value SHALL be `a/b`

#### Scenario: Unpaired surrogate is refused

- **GIVEN** JSON text containing the escape `"\uD800"` with no low surrogate
- **WHEN** a script calls `parse_json`
- **THEN** the step SHALL fail with a `parse` error naming the script call
  position, with no input text and no parser line or column

### Requirement: Rhai JSON number projection is integer-form only

`parse_json` SHALL store each number as its exact source token and SHALL NOT
convert any number to `f64`. A token SHALL project to a native Rhai `INT` only
when it is an integer form with no fraction and no exponent and its
`parse::<i64>()` fits; `-0` MAY normalize. Every other token, including any
decimal or exponent form and any integer outside the `i64` range, SHALL be a
`JsonNumber` holding the raw token. The only conversion SHALL be an explicit
`JsonNumber.to_float()`, which returns a finite `f64` or an `arithmetic` error.

#### Scenario: Integer forms project, decimals and exponents wrap

- **GIVEN** the JSON numbers `9223372036854775807`, `-0`, `1.0`, `1e2`, and
  `18446744073709551615`
- **WHEN** a script reads them after `parse_json`
- **THEN** `9223372036854775807` (and the normalized `-0`) SHALL be native
  integers, while `1.0`, `1e2`, and `18446744073709551615` SHALL each be a
  `JsonNumber`

#### Scenario: Large magnitude round-trips exactly

- **GIVEN** the JSON numbers `18446744073709551615`,
  `123456789012345678901234567890`, and `1e400`
- **WHEN** a script converts `parse_json` output back with `to_json`
- **THEN** each number SHALL re-emit with its exact source token and no `f64`
  rounding

#### Scenario: to_float is explicit and checked

- **GIVEN** the `JsonNumber` for `1e400`
- **WHEN** a script calls `to_float()` on it
- **THEN** the step SHALL fail with an `arithmetic` error because the result is
  not finite

### Requirement: Rhai JSON objects preserve insertion order and duplicate semantics

`parse_json` SHALL retain object keys in authored insertion order in an
`IndexMap`. A JSON duplicate key SHALL take the last value while keeping the
position of its first occurrence. Replacing the value at an existing key SHALL
retain that key's position. `remove` SHALL use shift semantics (remaining keys
keep their relative order) and re-inserting a removed key SHALL place it at the
end. `to_json` SHALL emit keys in stored order.

#### Scenario: Authored order is retained

- **GIVEN** the JSON text `{"z":1,"a":2,"m":3}`
- **WHEN** a script re-emits it with `to_json`
- **THEN** the keys SHALL appear in the order `z`, `a`, `m`

#### Scenario: Duplicate key keeps first position, last value

- **GIVEN** the JSON text `{"b":1,"a":2,"b":3}`
- **WHEN** a script re-emits it with `to_json`
- **THEN** the output SHALL be `{"b":3,"a":2}` in that key order

#### Scenario: Removing a keeps the order of the rest

- **GIVEN** a parsed object `{"a":1,"b":2,"c":3}`
- **WHEN** a script removes `a` and re-emits the object
- **THEN** the output SHALL contain `b` then `c` in that order

### Requirement: Rhai parsed JSON uses an Arc copy-on-write wrapper with a minimal surface

`parse_json` SHALL use the same projection at the root and on indexed reads:
objects/arrays become `JsonValue`, strings/bools/integer-form i64 become native,
other numbers become `JsonNumber`, and null becomes unit. Containers are backed
by an `Arc` tree. Reading a wrapper SHALL clone the `Arc`
(O(1)); mutating a value reached by a bare read SHALL NOT affect its source.
The wrapper SHALL expose only: string-key and integer-index get and set
(including chained assignment), dot-property fallback, `len`, `contains`,
`keys`, `remove`, and `push`. Public `type_of` names SHALL be exactly
`json value` and `json number`; outbound errors SHALL use these stable labels,
never Rust type paths.

#### Scenario: Bare read is isolated

- **GIVEN** a parsed object `{"a":{"b":1}}`
- **WHEN** a script evaluates `let x = j["a"]; x["b"] = 9;` and then
  `to_json(j)`
- **THEN** the output SHALL still contain `"b":1`

#### Scenario: type_of uses stable labels

- **GIVEN** a parsed object and a parsed `JsonNumber`
- **WHEN** a script evaluates `type_of` on each
- **THEN** the results SHALL be `json value` and `json number` respectively

### Requirement: Rhai JSON wrapper indexing, assignment, and mutation semantics

The wrapper SHALL use one projection everywhere: object or array SHALL read as a
`JsonValue`; string, bool, and unit SHALL read natively; an `i64` number SHALL
read as native `INT`; any other number SHALL read as `JsonNumber`. A missing key
and a JSON `null` SHALL both read as unit; `contains` and the `in` operator
SHALL distinguish a present key from an absent one. `contains` SHALL accept a
string key on an object; calling it on an array SHALL fail with `type-mismatch`.
Dot access on an object SHALL fall back to string-key indexing. A negative array
index SHALL count from the end; an out-of-range index SHALL raise
`ErrorArrayBounds`. `push` SHALL only append and SHALL recheck the array cap.
`remove` SHALL return the removed value, or unit when the key or index is
absent. Assigning unit SHALL store JSON `null`.

Accepting setters SHALL be: unit, bool, `i64`, finite `f64`, `String`,
`JsonNumber`, `JsonValue`, native `Map`, and native `Array`. Index and helper
setters SHALL be registered as single `Dynamic`-accepting functions that
validate the value type internally, so Rhai never fails to resolve an overload
and raises a discarded `ErrorIndexingType`. A setter with a wrong-container
target SHALL fail with a `type-mismatch` error.

#### Scenario: Chained, dot, and negative-index access

- **GIVEN** a parsed object `{"a":[{"b":1}]}`
- **WHEN** a script evaluates `j["a"][0]["b"] = 2`, then `j.a[-1].b = 3`
- **THEN** `to_json(j)` SHALL contain the updated nested value, and an
  out-of-range index SHALL fail with `ErrorArrayBounds`

#### Scenario: Missing key versus null

- **GIVEN** a parsed object `{"n":null}` with no key `m`
- **WHEN** a script evaluates `j["n"]`, `j["m"]`, `contains("n")`,
  `contains("m")`, and `"m" in j`
- **THEN** both reads SHALL return unit, `contains("n")` SHALL be `true`,
  `contains("m")` SHALL be `false`, and `"m" in j` SHALL be `false`

#### Scenario: Contains on an array fails typed

- **GIVEN** a parsed array
- **WHEN** a script calls `contains("k")` on it
- **THEN** the step SHALL fail with a `type-mismatch` error

#### Scenario: Push grows and remove returns

- **GIVEN** a parsed array `[1,2]`
- **WHEN** a script calls `push(3)`, then `remove(0)`
- **THEN** the array SHALL grow to `[1,2,3]`, and `remove(0)` SHALL return `1`

#### Scenario: Unit assignment stores null

- **GIVEN** a parsed object
- **WHEN** a script assigns `j["k"] = ()` and re-emits it
- **THEN** the output SHALL contain `"k":null`

#### Scenario: Nested setter failure rolls back

- **GIVEN** a parsed object
- **WHEN** a chained assignment fails on a nested wrong-typed setter
- **THEN** no part of the wrapper SHALL be mutated

### Requirement: Rhai JSON wrapper comparisons refuse implicit coercion

`==`, `!=`, `<`, `>`, `<=`, and `>=` SHALL be registered between
`JsonNumber`/`JsonValue` and native `i64`, `f64`, `String`, and `bool`, and
between wrappers, in both operand orders. All these comparisons SHALL fail with
`type-mismatch`; no exact-number comparison engine SHALL be added. Scripts MAY
explicitly call `to_float()` for lossy comparisons. Pairs outside the registered
set retain Rhai builtin behavior, including comparisons with unit. JSON null
projects to unit, so its equality with unit is true.

#### Scenario: Mixed numeric comparison refuses implicit coercion

- **GIVEN** a `JsonNumber` holding `18446744073709551615`
- **WHEN** a script evaluates `j == 2` or `j < 3`
- **THEN** each SHALL fail with `type-mismatch`

#### Scenario: Null compares with unit

- **GIVEN** a parsed JSON `null`
- **WHEN** a script evaluates the parsed value `== ()`
- **THEN** the comparison SHALL be `true`

### Requirement: Rhai JSON serialization is faithful and compatible

`to_json` SHALL be registered for `Map`, `Array`, `JsonValue`, `JsonNumber`,
and the scalar types `String`, `bool`, `i64`, `f64`, and unit, in both function
and method form. `to_string` and `to_debug` on a wrapper SHALL return the same
compact JSON (for `JsonNumber`, the number token). Native `Map` keys SHALL be
emitted sorted; a wrapper nested inside a native `Map` or `Array` SHALL be
inlined. Output SHALL be raw UTF-8 (non-ASCII emitted directly), SHALL NOT
escape `/`, and SHALL escape control characters. Unsupported values (function
pointers, closures, timestamps, custom types) and non-finite floats SHALL fail
with a `type-mismatch` error and SHALL NOT use a `Debug`/`Display` fallback or
include the value in the error. Serializing a native container SHALL apply the
same depth invariant as parsing.

#### Scenario: Native containers serialize sorted with nested wrappers

- **GIVEN** a native Rhai `Map` `#{"b":1,"a":<parsed value>}`
- **WHEN** a script calls `to_json` on it
- **THEN** the output SHALL order keys `a` then `b` and inline the parsed value

#### Scenario: Function and method forms

- **GIVEN** a parsed value
- **WHEN** a script evaluates `to_json(j)` and `j.to_json()`
- **THEN** both SHALL return the same compact JSON

#### Scenario: Raw UTF-8 and no slash escaping

- **GIVEN** a parsed value containing `café` and `a/b`
- **WHEN** a script calls `to_json`
- **THEN** the output SHALL contain the literal UTF-8 text and the literal `/`

#### Scenario: Non-finite float is refused

- **GIVEN** a Rhai `Map` whose value is `0.0/0.0`
- **WHEN** a script calls `to_json` on it
- **THEN** the step SHALL fail with a `type-mismatch` error and SHALL NOT emit
  `NaN` or a debug string

### Requirement: Rhai JSON wrappers are bounded by the sandbox limits

Parsing SHALL descend the input level at a time using
`serde_json::value::RawValue` and validate iteratively with an owned depth
counter. A top-level scalar SHALL have depth 0 and a top-level container depth
1; depth 128 SHALL be accepted and depth 129 SHALL fail with `limit`. Input
`max-string-size` SHALL be enforced before descent, and total parse cost SHALL
be bounded by 128 × the input byte length. The implementation SHALL NOT enable
`arbitrary_precision` or `unbounded_depth`. Parsing SHALL enforce
`max-array-size` and `max-map-size` per container and fail with `limit` when
either cap is exceeded.

The depth cap SHALL be invariant across parsing and mutation. Every set, push,
and conversion, and every `to_json` of a native container, SHALL check the
depth of the target path plus the depth of the inserted value and SHALL fail
with `limit` before applying when the total would exceed 128; a self-assignment
such as `j["a"] = j` that would exceed the cap SHALL be refused to avoid a stack
crash. Array and map caps and a total serialized-size estimate SHALL be
rechecked on every mutation, and a mutation that would exceed `max-string-size`
SHALL fail with `limit` before being applied. The serializer SHALL be bounded
and SHALL stop at the limit with a `limit` error rather than truncating. These
limits SHALL be read from the calling engine. A size limit configured as `0`
SHALL keep Rhai's unlimited meaning for that cap; the owned depth cap of 128
and the 128 × input-byte parse cost bound SHALL still apply. Exceeding any bound SHALL NOT
commit any Exchange change.

#### Scenario: Depth boundary

- **GIVEN** JSON text whose top-level container has depth 128
- **WHEN** a script calls `parse_json`
- **THEN** parsing SHALL succeed; one more level SHALL fail with a `limit` error

#### Scenario: Mutation depth and cap are rechecked

- **GIVEN** a wrapper at depth 128
- **WHEN** a script assigns a nested value that would deepen it, or pushes
  beyond `max-array-size`
- **THEN** the step SHALL fail with a `limit` error and the wrapper SHALL be
  unchanged

#### Scenario: Self-assignment at the cap is refused

- **GIVEN** a wrapper at depth 128
- **WHEN** a script evaluates `j["a"] = j`
- **THEN** the step SHALL fail with a `limit` error before a stack crash

### Requirement: Rhai JSON errors follow the existing classifier

An unavailable operation SHALL be classified by the engine's existing
classifier and SHALL NOT be given a new failing overload. `JsonNumber`
arithmetic (`+`, `+=`) SHALL fail as `function-not-found`; `values` and `merge`
SHALL fail as `function-not-found`; an unsupported `for` iteration SHALL fail
as `runtime` (ErrorFor). Every registered wrapper comparison SHALL fail as
`type-mismatch`; these guards are the only failing overloads added.
A size or depth violation SHALL be `limit`. A malformed parse
SHALL be `parse` with no input text and no parser line or column. A setter type
failure SHALL be `type-mismatch` and SHALL NEVER be `ErrorIndexingType`, because
Rhai silently discards indexing errors during index-chain write-back. No error
SHALL contain exchange data.

#### Scenario: Arithmetic and container helpers are function-not-found

- **GIVEN** a `JsonNumber` and a parsed object
- **WHEN** a script evaluates `j + 1`, `j += 1`, `obj.values()`, or
  `obj.merge(x)`
- **THEN** each SHALL fail with a `function-not-found` error

#### Scenario: Iteration is a runtime error

- **GIVEN** a parsed object
- **WHEN** a script iterates it with `for`
- **THEN** the step SHALL fail with a `runtime` error (ErrorFor)

#### Scenario: Setter failure propagates

- **GIVEN** a parsed object
- **WHEN** a script assigns a function pointer to a key
- **THEN** the step SHALL fail with a `type-mismatch` error; the failure SHALL
  NOT be an `ErrorIndexingType` and SHALL NOT be discarded

#### Scenario: Parse error is redacted

- **GIVEN** a script that calls `parse_json` on malformed text holding secret
  data
- **WHEN** the script runs
- **THEN** the error SHALL NOT contain the input text or a parser line/column;
  it SHALL carry only the class and the script call position

### Requirement: Rhai host JSON helpers leave the exchange converter unchanged

`parse_json` and `to_json` SHALL be script-local host helpers. They SHALL NOT
change the exchange inbound conversion (`body`, `headers`, and `properties`
binding) or scope preparation, and a JSON integer greater than `i64::MAX` on
that path SHALL still be refused. The outbound `dynamic_to_value` converter
SHALL gain explicit refusal arms for `JsonValue` and `JsonNumber` that use only
the stable labels `json value` and `json number`; no other converter behavior
SHALL change. Direct outbound conversion of a wrapper SHALL remain deferred, so
persisting parsed JSON SHALL require `to_json` or a native leaf. Assigning
`body = to_json(j)` SHALL yield a text body holding valid JSON. Assigning a
wrapper directly to a body SHALL fail as an explicit compatibility break. The
`rc-141om` inbound behavior SHALL remain untouched.

#### Scenario: Exchange inbound behavior is unchanged

- **GIVEN** an exchange property holding a JSON integer greater than `i64::MAX`
- **WHEN** a Rhai script reads that property
- **THEN** the evaluation SHALL still fail with the existing typed conversion
  error

#### Scenario: Outbound refusal uses stable labels

- **GIVEN** a Rhai script whose expression result is a `JsonValue`
- **WHEN** the outbound conversion runs
- **THEN** it SHALL refuse with a conversion error whose source label is
  `json value` (or `json number`) and no Rust type path

#### Scenario: Explicit serialization yields a text body

- **GIVEN** a script that parses JSON and assigns `body = to_json(j)`
- **WHEN** the script runs
- **THEN** the body SHALL be a text body holding valid JSON

### Requirement: Rhai JSON round-trip is semantic, not textual

`to_json(parse_json(s))` SHALL re-emit JSON with the same value, numeric
magnitude, and key order as `s`. Wrapped number tokens SHALL remain exact;
native i64 spelling MAY normalize, including `-0` to `0`.
Whitespace, indentation, and escape spelling
SHALL NOT be preserved and no byte-for-byte guarantee SHALL be made.

#### Scenario: Equivalent, not byte-identical

- **GIVEN** the JSON text `{ "a" : 1 , "b" : "x\/y" }`
- **WHEN** a script re-emits it with `to_json`
- **THEN** the output SHALL be valid JSON with the same value and key order,
  with different whitespace permitted

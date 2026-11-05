# Proposal: simple-bytes-interpolation

## Why

Downstream report (camel-112, bd rc-0v855): after an `http:` producer, a Simple
template `set_body: { simple: "status=${header.CamelHttpResponseCode} body=${body}" }`
renders `body=[60,115,111,97,...]` instead of the response text. The mechanism:
`body_value` (`crates/languages/camel-language-simple/src/evaluator.rs`) maps
`Body::Bytes` to a `Value::Array` of integers, and template interpolation stringifies
that array. The byte-array projection itself is normative for a bare body access —
the canonical spec "Body exposure is native and non-destructive" pins it — but
interpolation is string concatenation, and a number list inside a composed string is
never what the author meant.

## What Changes

- A **bare** `${body}` (the whole expression) keeps the native byte-array projection.
  The integrity rule is not weakened.
- When a Bytes-backed value is **embedded in a larger template string** (anything that
  makes the result a concatenation: `pre${body}`, `${body}suffix`, `${a}${body}`),
  the Simple language renders it as UTF-8 text, and fails with a typed `conversion`
  error when the bytes are not valid UTF-8. Never lossy, never a byte list. This is
  the same rule the canonical requirement already gives MiniJinja.
- Nested field access on a Bytes body (`${body.field}`) is out of scope: behavior
  unchanged.
- Rhai, JavaScript, and MiniJinja are untouched. `convert_body_to` is untouched.
- Docs (`docs/src/languages/simple.md`) gain the one-line rule next to the existing
  Bytes note.

Affected crates: `camel-language-simple` (only implementation crate), `docs`.

## Acceptance criteria

- `bare_body_bytes_stays_byte_array`: `${body}` over `Body::Bytes([0,128,255])`
  returns the array (existing pinned behavior, unchanged).
- `interpolated_bytes_render_as_utf8_text`: template `body=${body}` over
  `Body::Bytes(b"hello")` yields `body=hello`.
- `interpolated_bytes_prefix_and_suffix`: `pre${body}post` yields `prehellopost`
  (exactly one concatenation, no brackets).
- `interpolated_invalid_utf8_bytes_fail_typed`: `Body::Bytes(&[0xff, 0xfe])` in a
  template fails with error class `conversion`; no U+FFFD replacement characters.
- `interpolated_mixed_headers_and_bytes`: the reported case
  `status=${header.CamelHttpResponseCode} body=${body}` renders text.
- `non_bytes_interpolation_unchanged`: Text bodies interpolate exactly as before.
- Canonical spec delta MODIFIES `language-value-boundary`, keeps every existing
  scenario name in the MODIFIED requirement block, and adds the interpolation-context
  rule + scenario.

## Risk budget

Low. The change is confined to the Simple template assembly path in one crate. The
risk of touching the bare-access projection is out of bounds — the acceptance suite
pins it first. Invalid UTF-8 must remain a typed error, never lossy decoding.

Refs: bd rc-0v855.

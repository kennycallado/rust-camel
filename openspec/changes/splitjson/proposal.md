# Proposal: splitjson

## Why

The programmatic `split_body_json_array()` splitter (`crates/camel-api/src/splitter.rs`)
wraps every array element in `Body::Json(val.clone())`. When the array holds
strings, each fragment renders in `${body}` as a JSON-quoted string (`"a"`
instead of `a`) — the same defect class the declarative split fixed in mission
251 (bd rc-etf0q, landed 938bea3d): there, `Value::String` array elements
became `Body::Text` and non-string elements kept `Body::Json`. e_glm's 251
validation conditioned the fix set on repairing the programmatic path
separately, for cross-path consistency (bd rc-fflhv).

Both paths must agree: a string element types the same fragment whether the
split was declared in YAML or built with `split_body_json_array()`.

## What Changes

Included:

- `camel-api` `splitter.rs` `split_body_json_array`: string elements map to
  `Body::Text` (exact parity with the declarative fix); number, boolean,
  object, nested-array, and null elements keep `Body::Json`.
- Flip 251's transitional pin test
  (`split_body_json_array_string_elements_stay_json`) to pin the new behavior;
  add the empty-string delta pin (empty string yields an empty `Body::Text`,
  matching 251) and non-string element pins (number, object, null).
- Scope scan of the other programmatic splitters in `camel-api` for the same
  defect class (`split_body_lines` already yields `Body::Text`; custom
  `split_body` owns its typing policy; streaming split checked and reported).
- Docs callout clause: the fragment-typing rule names the programmatic splitter
  (`docs/src/eip/splitter.md`).
- Spec delta: the `eip-splitter` "Fragment body typing is element-driven"
  requirement drops its declarative-only scoping; the scenario that pinned the
  old programmatic behavior flips to element-driven parity.

Excluded: any `${body}` rendering change in the simple language (Option B of
251, rejected there); `//item` subtree serialization (separate bd); the
declarative path itself (already fixed, precedent, unchanged).

## Acceptance criteria

- A JSON array of strings split by `split_body_json_array()` yields
  `Body::Text` fragments carrying the raw strings; `${body}` renders unquoted
  (`Body::Text` variant is the unquoted-rendering proxy, per 251's accepted
  r_glm finding #1).
- Number, boolean, object, nested-array, and null elements still yield
  `Body::Json` fragments.
- The empty-string element yields an empty `Body::Text` fragment, not `""`.
- The declarative path is untouched (251's six guards stay green).
- All order gates pass: fmt, clippy 4 legs, camel-api tests, lint-unbounded-wait
  ratchet 296 (markers on their own line), schema check, doc build.

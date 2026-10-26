# Design: 413-rhai-parse-json-typed-rejections

The guard is an exact-typed native overload registered in the shared host module
next to the string overload (`crates/languages/camel-language-rhai/src/json/host.rs`).
Each guard covers one concrete Rhai type (`Blob`, `Map`, `Array`, `JsonValue`,
`JsonNumber`, `i64`, `f64`, `bool`, `()`, `char`, `FnPtr`, `Instant`) and raises
`ErrorMismatchDataType` with a reserved expected label and a static actual label.

`map_eval_alt` rewrites only that partition (reserved expected label + an
allowlisted actual label) into `LanguageError::TypeMismatch { expected: "string",
actual, position }`. No `Dynamic` catch-all is registered, so overload precedence
for the string path and every other function stays unchanged. `Body::Stream`
keeps its existing conversion refusal.

Rejected alternatives: accepting `Blob` and decoding UTF-8 inside the sandbox
(the sandbox has no lossless byte-to-string path; `Blob.as_string` uses
`from_utf8_lossy`), and changing body binding so `Bytes` becomes text (global
blast radius across languages, hides the encoding decision from the operator).

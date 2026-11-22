## Context

Rustdoc (the `-D warnings` build) flags exactly three links, verified on main
(`doc-before.log`, exit 101):

| File | Line | Link target |
|------|------|-------------|
| `compile/materialize.rs` | 15 (module doc) | `standard_materialization_candidates` |
| `compile/materialize.rs` | 138 (`Materialization::create` doc) | `standard_materialization_candidates` |
| `compile/policy.rs` | 42 (module doc) | `is_in_memory_class` |

Both targets are `pub(crate)`.

## Decision

Replace ``[`name`]`` with ``` `name` ``` at those three sites only. Rejected:
making the items `pub` (API change), `--document-private-items` (changes the
gate), `#[allow(rustdoc::private_intra_doc_links)]` (hides a real doc defect).

Other ``[`is_in_memory_class`]`` links (policy.rs lines ~126-150, on private
`pub(crate)` consts) and `runtime.rs:683` are in docs of private items; rustdoc
does not flag them and they stay unchanged (minimal diff).

## Verification

`RUSTDOCFLAGS="-D warnings" cargo doc -p camel-cli --no-deps` goes from exit
101 to exit 0. Standard gate sweep, workspace build, and the declared camel-cli
lane stay green.

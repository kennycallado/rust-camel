# Relocation Manifest (eliminate-devdep-cycles, per ADR-0055)

One row per relocated item. For FILE rows the origin path no longer
exists; for FUNCTION rows the function definition was removed from the
origin FILE (the file itself remains).

## Functions relocated from camel-core (Task 1.1)

| Origin (function) | Origin file | Destination |
|---|---|---|
| `all_phase2_schemes_have_options` | `crates/camel-core/src/component_metadata_catalog.rs` | `crates/camel-test/tests/core_catalog_real_metadata_test.rs` |
| `no_duplicate_option_names` | `crates/camel-core/src/component_metadata_catalog.rs` | `crates/camel-test/tests/core_catalog_real_metadata_test.rs` |

## Files relocated from camel-endpoint-macros (Task 1.2)

| Origin file | Destination file |
|---|---|
| `crates/camel-endpoint-macros/tests/derive_integration.rs` | `crates/camel-endpoint/tests/endpoint_macros_derive_integration_test.rs` |
| `crates/camel-endpoint-macros/tests/ui_tests.rs` | `crates/camel-endpoint/tests/endpoint_macros_ui_tests.rs` |
| `crates/camel-endpoint-macros/tests/ui/duplicate_path_field_fail.rs` | `crates/camel-endpoint/tests/ui/duplicate_path_field_fail.rs` |
| `crates/camel-endpoint-macros/tests/ui/duplicate_path_field_fail.stderr` | `crates/camel-endpoint/tests/ui/duplicate_path_field_fail.stderr` |
| `crates/camel-endpoint-macros/tests/ui/kind_typo_fail.rs` | `crates/camel-endpoint/tests/ui/kind_typo_fail.rs` |
| `crates/camel-endpoint-macros/tests/ui/kind_typo_fail.stderr` | `crates/camel-endpoint/tests/ui/kind_typo_fail.stderr` |
| `crates/camel-endpoint-macros/tests/ui/missing_uri_scheme_fail.rs` | `crates/camel-endpoint/tests/ui/missing_uri_scheme_fail.rs` |
| `crates/camel-endpoint-macros/tests/ui/missing_uri_scheme_fail.stderr` | `crates/camel-endpoint/tests/ui/missing_uri_scheme_fail.stderr` |
| `crates/camel-endpoint-macros/tests/ui/non_struct_fail.rs` | `crates/camel-endpoint/tests/ui/non_struct_fail.rs` |
| `crates/camel-endpoint-macros/tests/ui/non_struct_fail.stderr` | `crates/camel-endpoint/tests/ui/non_struct_fail.stderr` |
| `crates/camel-endpoint-macros/tests/ui/no_optin_no_metadata_fn_fail.rs` | `crates/camel-endpoint/tests/ui/no_optin_no_metadata_fn_fail.rs` |
| `crates/camel-endpoint-macros/tests/ui/no_optin_no_metadata_fn_fail.stderr` | `crates/camel-endpoint/tests/ui/no_optin_no_metadata_fn_fail.stderr` |
| `crates/camel-endpoint-macros/tests/ui/secret_with_default_fail.rs` | `crates/camel-endpoint/tests/ui/secret_with_default_fail.rs` |
| `crates/camel-endpoint-macros/tests/ui/secret_with_default_fail.stderr` | `crates/camel-endpoint/tests/ui/secret_with_default_fail.stderr` |
| `crates/camel-endpoint-macros/tests/ui/unknown_key_fail.rs` | `crates/camel-endpoint/tests/ui/unknown_key_fail.rs` |
| `crates/camel-endpoint-macros/tests/ui/unknown_key_fail.stderr` | `crates/camel-endpoint/tests/ui/unknown_key_fail.stderr` |

## Verification

- `cargo run -p xtask -- publish --show-cycles` → `no_verify set: 0 crate(s)`, zero broken edges.
- Every destination path exists; every FILE origin path is deleted; the two FUNCTION origin definitions are absent from `crates/camel-core/src/component_metadata_catalog.rs`.
- `cargo test -p camel-test --test core_catalog_real_metadata_test` passes; `cargo test -p camel-endpoint --test endpoint_macros_derive_integration_test` + `--test endpoint_macros_ui_tests` pass.

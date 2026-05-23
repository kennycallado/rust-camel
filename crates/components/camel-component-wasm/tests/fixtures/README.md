# WASM Test Fixtures

Integration tests that execute real WASM modules require a compiled `.wasm`
artifact placed in this directory.

## Building the fixture

```bash
# From the workspace root:
cargo build --target wasm32-wasip2 -p <fixture-crate-name>
```

Copy the resulting `.wasm` file from `target/wasm32-wasip2/debug/` into this
`tests/fixtures/` directory and reference it in the test URI:

```
wasm:fixtures/<name>.wasm
```

The `#[ignore]` test `wasm_integration_with_compiled_fixture` will be skipped
until the fixture is available.

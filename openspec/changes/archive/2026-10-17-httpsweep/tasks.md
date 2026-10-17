# Tasks: httpsweep (mission 320, LIGHT)

## 1. Dedup the fallback client-config preamble (rc-jr2g8)

- [x] 1.1 In `crates/components/camel-http/src/tls.rs`, extract
  `fn fallback_client_config_builder()
  -> rustls::ConfigBuilder<rustls::ClientConfig, rustls::WantsVerifier>`
  carrying the provider-mirror comment + resolution and the
  version-pinned `expect` (keep the `// allow-unwrap` marker). Chain both
  `webpki_root_client_config` and `fallback_client_config` through it;
  preserve the "Computed ALWAYS" store comment at its original
  `fallback_root_store` call site. Verify:
  `cargo fmt --check`,
  `cargo clippy -p camel-component-http --all-targets -- -D warnings`,
  scoped battery `cargo test -p camel-component-http -j4` — ALL result
  lines summed == 461, zero failures.

## 2. Zero-code closes / verifies (rc-oltht, rc-i4yjl)

- [x] 2.1 rc-oltht: confirm at base 76687d95 that
  `webpki_root_client_config` already routes through `mozilla_only()`
  (ed5417df); close bd zero-code-change citing the commit.
- [x] 2.2 rc-i4yjl: confirm the only `Client::new()` site in the crate is
  the deliberate `plain_http_test_client` guard; record verify-only
  disposition (bd already closed via httpxtract).

## 3. Sweep-inventory scan + stale-comment fix

- [x] 3.1 Scan zone for duplicate fn names / TODO/FIXME / dead arms /
  stale comments. Fix the one hit: `plain_http_test_client` doc comment
  reference "lib.rs `mod tests`" → `lib_tests.rs` (post-b393d15b split).

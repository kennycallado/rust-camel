# Proposal: emergcli

## Why

The P0 castrict fix (a3ba5136, bd rc-hl9cn) made the webpki fallback
honor strict TLS material on configuration errors, but left one escape:
when the fallback client REBUILD itself fails (the `second_error`
terminal in `webpki_fallback_client`, `crates/components/camel-http/
src/lib.rs:2811-2825`), the code returns `Ok(emergency_webpki_client())`
unconditionally. That emergency client carries only bundled Mozilla
roots — it drops custom CA, mTLS identity, and disabled-verification
parity even when `tls.strict=true`. Same escape class as the original
P0. The branch is documented as unreachable on reqwest 0.13.4 without
HTTP/3, but it is the explicit error path and becomes security-relevant
if builder fallibility changes. bd: rc-3x5qj (P1, castrict-rpt
finding).

## What Changes

- In the fallback rebuild-failure terminal: when `tls.strict=true`,
  return a typed `CamelError::EndpointCreationFailed` prefixed
  `tls.strict/webpki-fallback:` (existing convention) — never a
  material-free client. When strict is off, keep the current
  warn-degrade: loud `error!` log plus material-free emergency client.
- A `#[cfg(test)]` seam that deterministically forces the rebuild
  failure (the real error cannot be produced on reqwest 0.13.4), so
  the branch is testable hermetically.
- Adversarial tests: strict rebuild failure produces the typed error
  (custom CA and mTLS configured — cannot silently drop); the typed
  error folds into `strict_tls_error` and surfaces at endpoint
  creation (constructor stays no-panic); non-strict rebuild failure
  degrades to the emergency client with `Ok`.
- Doc-comment and `CONTEXT.md` alignment for the changed terminal.

Excluded: primary-path behavior, `fallback_client_config` material
handling (already fail-closed), any change to non-strict degradation
semantics elsewhere.

## Acceptance criteria

- Strict + rebuild failure: `build_client` returns
  `EndpointCreationFailed` containing `tls.strict/webpki-fallback:`;
  no emergency client is served to endpoints.
- Via the component constructor, the same failure surfaces at endpoint
  creation; construction itself never panics.
- Non-strict + rebuild failure: `build_client` returns `Ok` with the
  emergency client; degradation is logged.
- Mission gates green: `cargo fmt --check --all`,
  `cargo clippy -p camel-component-http -- -D warnings`,
  `cargo test -p camel-component-http`.

## Risk budget

- Touches one terminal in one crate; no API signature changes.
- Accepted risk: none beyond the strict fail-closed itself. Out of
  bounds: making the non-strict path stricter, HTTP/3 enablement,
  refactor of `client_builder`.

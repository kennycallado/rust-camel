# Design: emergcli

## Approach

The second-error terminal lives inside the `build_with_backend`
closure of `webpki_fallback_client` (lib.rs ~2811-2825). Today its
`Err(second_error)` arm logs `error!` (log-policy: system-broken) and
returns `Ok(emergency_webpki_client())` regardless of strict mode.

Restructure:

1. Extract the terminal into a small function
   `fallback_rebuild_failed(config, failure)` taking the failure as
   `&dyn std::fmt::Display` (real `reqwest::Error` or the
   test-forced variant). The closure delegates to it.
2. Inside the terminal: log the existing system-broken `error!` line
   first (operator signal, unchanged text). Then branch on
   `config.tls.as_ref().is_some_and(|tls| tls.strict)`:
   - strict: `Err(CamelError::EndpointCreationFailed(format!(
     "tls.strict/webpki-fallback: webpki fallback client rebuild failed \
      — refusing material-free emergency client under tls.strict: {failure}")))`
   - non-strict: `Ok(emergency_webpki_client())` (current behavior).
3. Test seam: `#[cfg(test)] thread_local FORCE_FALLBACK_REBUILD_FAIL:
   Cell<bool>` beside the existing `FORCE_WEBPKI_FALLBACK` seams. When
   armed, `build_with_backend` short-circuits into the terminal with a
   forced-failure value (reqwest 0.13.4 cannot fabricate a real build
   error; the terminal only needs `Display`). Release builds unchanged.
4. Constructor no-panic is preserved by existing plumbing:
   `client_or_emergency` folds `build_client`'s `Err` into
   `(emergency_client, Some(err))` and the constructors store it as
   `strict_tls_error: strict_err.or(build_err)`, which surfaces at
   endpoint creation (`if let Some(err) = &self.strict_tls_error`).
   No constructor changes needed.

Doc comments to update: the "Shared second-error terminal" block and
the `webpki_fallback_client` doc claim "`Err` is strict-only by
construction" — both stay true and get sharpened: the rebuild terminal
is now strict-fail-closed too. `crates/components/camel-http/
CONTEXT.md` fallback section gains one sentence on the rebuild policy.

## Affected crates

- camel-component-http: `src/lib.rs` (terminal + seam + tests),
  `CONTEXT.md` (one sentence). No other crates.

## Architecture boundaries

Component-layer change only (camel-component-http). No Runtime, DSL,
Services, or Languages surface moves. The typed error is the existing
`CamelError::EndpointCreationFailed` used throughout the strict
fallback work — same boundary crossing as the P0 fix (rc-hl9cn), same
`tls.strict/webpki-fallback:` prefix convention. Follows ADR-0012
log-level policy: the `error!` stays system-broken classification (the
rebuild failure means the TLS stack is broken process-wide); the strict
typed error is the fail-closed contract, not a log change.

## Alternatives considered

- Remove the emergency substitution entirely (always propagate):
  rejected — breaks the non-strict no-panic startup contract on
  CA-less platforms (rc-3j4mq regression territory).
- Fold the strict check into `client_or_emergency`: rejected — the
  terminal is where the policy diverges; folding later would serve a
  material-free client to `build_client` callers that contractually
  expect strict-only `Err` (pinned-client cache path).
- Panic on strict rebuild failure: rejected — constructor no-panic is
  a hard requirement; typed error + endpoint-time surfacing is the
  established pattern.

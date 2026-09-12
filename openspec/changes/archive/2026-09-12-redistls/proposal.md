# Proposal: redistls

## Why

Redis `rediss://` currently verifies the Redis server with `tls_ca_cert`, but
does not authenticate the client with a certificate. No supported deployment
requires client authentication today. bd rc-76exv records this as a trust-model
decision so the omission is deliberate rather than an undocumented gap.

## What Changes

- Document that Redis TLS provides server authentication only in v1.
- Document that Redis client-certificate authentication (mTLS) is unsupported
  and that `TlsCertificates.client_tls` remains `None`.
- Correct the existing `tls_ca_cert` guidance to state that Sentinel TLS also
  supports the configured CA.
- Update the Redis component context with the same architectural decision.

This change does not add client certificate or key configuration, alter Redis
fixtures, or change standalone or Sentinel connection behavior.

## Acceptance criteria

- Redis component documentation states that `rediss://` and `tls_ca_cert`
  authenticate the server only and that client-certificate authentication is
  unsupported.
- Redis component documentation does not claim that Sentinel ignores
  `tls_ca_cert`; it records the existing Sentinel CA support.
- `crates/components/camel-redis/CONTEXT.md` records the fixed
  `client_tls: None` behavior and the trigger for revisiting it.
- No Rust code, fixture, URI parameter, or TLS handshake behavior changes.

## Risk budget

The risk budget is documentation-only. Do not introduce configuration fields,
secret-handling paths, fixture changes, or client TLS wiring. Revisit this
decision when a deployment requires Redis `--tls-auth-clients yes`.

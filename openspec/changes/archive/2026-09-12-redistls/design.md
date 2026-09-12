# Design: redistls

## Approach

Record the Redis TLS trust model at both documentation authorities: the public
Redis component guide and the camel-redis context document. Correct the stale
Sentinel CA note in the public guide, then place the decision next to the
existing `tls_ca_cert` explanation so operators can distinguish server
verification from mutual TLS. State the implementation anchor
(`TlsCertificates.client_tls = None`) and the concrete trigger for future work.

## Affected crates

- `camel-component-redis`: no source changes; its context document records the
  existing topology behavior.
- `docs`: Redis component security documentation changes.

## Architecture boundaries

This is a documentation change. It does not cross Runtime, DSL, Components,
Services, Languages, or Functions boundaries. It preserves the existing
one-way TLS contract: the Redis client verifies server certificates using the
configured CA, while client certificate authentication remains unsupported.

## Alternatives considered

- **Wire mTLS now:** rejected. It would require new certificate and key config,
  validation and redaction, standalone and Sentinel topology changes, fixtures
  using `--tls-auth-clients yes`, and live positive and negative coverage. No
  current deployment needs this P4 capability.
- **Leave behavior undocumented:** rejected. The current `client_tls: None`
  behavior is deliberate and must be visible to operators and maintainers.

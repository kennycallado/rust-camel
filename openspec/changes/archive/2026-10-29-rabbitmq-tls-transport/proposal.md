# Proposal: rabbitmq-tls-transport

## Why

`RabbitBrokerConfig` currently rejects every `tls.*` key through `deny_unknown_fields`
(fail-closed interim per the 2026-10-08 master ruling) because the real transport was
never implemented; before that ruling, `tls.ca_cert` / `tls.server_name` were parsed
and silently ignored — a fail-open defect (bd rc-l8ohw). Users cannot point the
rabbitmq component at an `amqps://` broker with a custom trust anchor. This change
implements the real TLS transport over lapin + rustls with the ring provider only
(no aws-lc, rc-2bofp).

## What Changes

- `[components.rabbitmq.brokers.<name>.tls]` becomes a supported broker section with
  `ca_cert` (PEM path for a custom trust anchor) and `server_name` (see below).
  Unknown keys under `tls` keep failing.
- `tls.ca_cert` is loaded at manager construction and handed to lapin through
  `OwnedTLSConfig.cert_chain`. Trust policy: system native roots PLUS the custom CA
  (additive). A missing, unreadable, or invalid PEM fails closed with a payload-free
  error naming the broker.
- `tls.server_name` CANNOT be honoured: lapin 4.12.0 derives TLS SNI and certificate
  verification from the broker URL host with no override seam (e_gpt ruling
  2026-10-29, mission 446). Setting it fails closed at boot with a named error.
- `amqp://` URLs combined with any `tls.*` parameter fail closed; `amqps://` never
  falls back to plaintext (transport-enforced by lapin's AMQPS scheme handling).
- Docs: component page and README document the TLS block, the `amqps://` requirement,
  the URL-host verification/SNI behavior, and every fail-closed case.

## Acceptance criteria

- Config parse/reject matrix: bare `tls` key on `amqp://` rejected; unknown `tls`
  sub-key rejected; `tls.ca_cert` on `amqps://` accepted.
- `server_name` set (any value, including empty or host-equal) → boot error with the
  prescribed text, before any network activity.
- CA load failures (missing file, non-PEM contents) → boot errors naming the broker,
  no PEM contents or credentials in the message.
- rustls config unit test: CA PEM added to the trust store; ring provider active.
- `cargo tree -p camel-component-rabbitmq -e features -i aws-lc-rs` shows no new
  component-owned aws-lc edge.

## Risk budget

Transport seams are verified against vendored lapin 4.12.0 / amq-protocol-tcp 10.6.3
sources; no dependency bumps. Reconnect path untouched — TLS is a transport option on
the existing connect seam. Out of bounds: mTLS/client certificates, `server_name`
emulation, aws-lc, `danger_accept_invalid_certs`.

Bd: rc-l8ohw

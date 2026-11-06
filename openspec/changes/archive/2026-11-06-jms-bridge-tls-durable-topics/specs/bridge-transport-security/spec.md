# bridge-transport-security — delta (Artemis verifyHost pin)

## ADDED Requirements

### Requirement: Artemis secure schemes pin hostname verification explicitly

For Artemis broker URLs with a secure scheme (`ssl://`, `wss://`), the bridge SHALL
set the connector's `verifyHost` transport property explicitly to `true` at
transport-config construction, so hostname verification is always on regardless of
the Artemis 2.36 client default (decision D1: verification always on; no
skip-verification knob exists). The pin SHALL be covered by a JVM test asserting
the transport config carries `verifyHost=true` for secure schemes and that
plaintext schemes carry no verify-host property. This does not change the Classic
path, which enforces the same D1 posture via `endpointIdentificationAlgorithm=HTTPS`
on the socket.

#### Scenario: Artemis secure scheme pins verifyHost

- **GIVEN** broker_type `artemis` with a complete keystore contract and an
  `ssl://` broker URL
- **WHEN** the Artemis transport config is built
- **THEN** SSL is enabled and the verify-host property is exactly `true`

#### Scenario: plaintext schemes stay untouched

- **GIVEN** broker_type `artemis` with a `tcp://` broker URL
- **WHEN** the transport config is built
- **THEN** no SSL properties and no verify-host property are present

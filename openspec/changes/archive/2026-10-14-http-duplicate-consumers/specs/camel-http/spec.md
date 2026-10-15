## ADDED Requirements

### Requirement: Duplicate HTTP Consumer registration fails closed

For each shared listener, legacy HTTP Consumers SHALL have unique path keys and method-aware HTTP Consumers SHALL have unique method and parsed path-template keys. Registration SHALL reject an occupied key atomically with CamelError::Config without replacing its sender. A rejected Consumer SHALL fail before signaling readiness and SHALL NOT unregister another Consumer. Distinct methods SHALL coexist, and completed orderly cancellation SHALL free the key for another registration. Existing co-hosting precedence and template ambiguity handling SHALL remain unchanged.

#### Scenario: Duplicate legacy path preserves the original Consumer

- **GIVEN** a ready Consumer without httpMethod on /duplicate
- **WHEN** another Consumer starts on the same listener and path
- **THEN** startup returns a configuration error before readiness, and cancelling the rejected Consumer leaves requests routed to the original Consumer

#### Scenario: Duplicate method and template preserves the original Consumer

- **GIVEN** a ready GET Consumer on /users/{id}
- **WHEN** another GET Consumer starts on the same listener and parsed template
- **THEN** startup returns a configuration error before readiness and the original remains registered

#### Scenario: Concurrent registration has exactly one winner

- **GIVEN** an unoccupied registry key
- **WHEN** two distinct senders register that key concurrently
- **THEN** exactly one registration succeeds and the stored sender belongs to that winner

#### Scenario: Sibling methods survive cancellation

- **GIVEN** GET and POST Consumers share /users
- **WHEN** GET is cancelled and its start loop exits
- **THEN** POST still dispatches and a replacement GET can register

#### Scenario: Orderly restart reclaims a key

- **GIVEN** a registered Consumer
- **WHEN** it is cancelled and its start loop completes, then a new Consumer starts with the same key
- **THEN** the replacement becomes ready and receives requests

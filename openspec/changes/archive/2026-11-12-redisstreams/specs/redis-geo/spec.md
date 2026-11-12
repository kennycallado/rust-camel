## MODIFIED Requirements

### Requirement: GEO commands register with correct resolution and retry classification
The five GEO command names SHALL parse from the endpoint URI only. The
endpoint URI SHALL be the only command source for every Redis producer
command (ADR-0032): a `CamelRedis.Command` header present on a producer
exchange — with any value — SHALL fail the exchange with a named processor
error before a connection is attempted. Retry classification SHALL treat
reads as idempotent and GEOADD as non-idempotent.

#### Scenario: URI resolves GEO commands and the header is rejected

<!-- openspec-scenario-ops
renamed: URI and header resolve GEO commands -> URI resolves GEO commands and the header is rejected
-->

- **GIVEN** an endpoint URI with `command=GEOSEARCH` and an exchange
  carrying a `CamelRedis.Command` header with `GEOADD`
- **WHEN** the producer resolves the command
- **THEN** the URI alone yields `RedisCommand::Geosearch`, the header
  overrides nothing, and the exchange fails before connecting with a
  processor error naming `CamelRedis.Command`

#### Scenario: Reads are idempotent and GEOADD is not

- **WHEN** `is_idempotent_command` classifies the geo variants
- **THEN** `Geopos`, `Geodist`, `Geosearch`, and `Geohash` return true
- **AND** `Geoadd` returns false

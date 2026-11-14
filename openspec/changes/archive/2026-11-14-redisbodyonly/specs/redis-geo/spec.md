## MODIFIED Requirements

### Requirement: GEOADD stores a member with coordinates

<!-- openspec-scenario-ops
renamed: The body is the member when the member header is absent -> The body is the member
-->

The producer SHALL store one member with its longitude and latitude under a key, taking the member from the exchange body through the shared body codec only and validating coordinates before any command is sent. `CamelRedis.Member` is removed as the GEOADD payload source and SHALL fail with the named tombstone error before any frame is sent; it remains an addressing operand on `GEOPOS`, `GEODIST`, `GEOSEARCH`, and `GEOHASH`.

#### Scenario: New member is added

- **GIVEN** a producer endpoint with `command=GEOADD` and a running Redis
- **WHEN** an exchange carries `CamelRedis.Key`, `CamelRedis.Longitude` (`13.361389`), `CamelRedis.Latitude` (`38.115556`), and a `Body::Text("Palermo")` body
- **THEN** GEOADD executes and the exchange body is the JSON integer `1`

#### Scenario: Existing member position is updated

- **GIVEN** member `Palermo` already stored under the key
- **WHEN** GEOADD runs again with different coordinates for `Palermo` in the body
- **THEN** the body is the JSON integer `0`

#### Scenario: Malformed latitude is rejected at parse time

- **GIVEN** an exchange with `CamelRedis.Latitude` set to `95.0`
- **WHEN** the GEOADD handler resolves headers
- **THEN** the operation fails with `CamelError::ProcessorError` naming `CamelRedis.Latitude` before any command is sent to Redis

#### Scenario: Malformed longitude is rejected at parse time

- **GIVEN** an exchange with `CamelRedis.Longitude` set to `-181.0`
- **WHEN** the GEOADD handler resolves headers
- **THEN** the operation fails with `CamelError::ProcessorError` naming `CamelRedis.Longitude` before any command is sent to Redis

#### Scenario: Missing member with an empty body fails before sending

- **GIVEN** an exchange with key and coordinates and a `Body::Empty` exchange body
- **WHEN** the GEOADD handler resolves the member value
- **THEN** the operation fails with `CamelError::ProcessorError` naming the command and the rejected body form before any command is sent to Redis

#### Scenario: The body is the member

- **GIVEN** an exchange with key, coordinates, and a `Body::Text("Palermo")` body
- **WHEN** GEOADD is processed against the fake RESP server
- **THEN** the GEOADD wire frame carries `Palermo` as the member argument

#### Scenario: A CamelRedis.Member header on GEOADD fails before sending

- **GIVEN** a GEOADD exchange with `CamelRedis.Member` set and a `Body::Text("Palermo")` body
- **WHEN** the GEOADD handler resolves the member value
- **THEN** the operation fails with `CamelError::ProcessorError` naming `CamelRedis.Member` and the move to the body before any command is sent to Redis

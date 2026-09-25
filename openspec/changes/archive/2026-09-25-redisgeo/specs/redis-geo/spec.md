# redis-geo Delta Spec

## ADDED Requirements

### Requirement: GEOADD stores a member with coordinates
The producer SHALL store one member with its longitude and latitude under a key, validating coordinates before any command is sent.

#### Scenario: New member is added

- **GIVEN** a producer endpoint with `command=GEOADD` and a running Redis
- **WHEN** an exchange carries `CamelRedis.Key`, `CamelRedis.Longitude`
  (`13.361389`), `CamelRedis.Latitude` (`38.115556`), and
  `CamelRedis.Member` (`Palermo`)
- **THEN** GEOADD executes and the exchange body is the JSON integer `1`

#### Scenario: Existing member position is updated

- **GIVEN** member `Palermo` already stored under the key
- **WHEN** GEOADD runs again with different coordinates for `Palermo`
- **THEN** the body is the JSON integer `0`

#### Scenario: Malformed latitude is rejected at parse time

- **GIVEN** an exchange with `CamelRedis.Latitude` set to `95.0`
- **WHEN** the GEOADD handler resolves headers
- **THEN** the operation fails with `CamelError::ProcessorError` naming
  `CamelRedis.Latitude` before any command is sent to Redis

#### Scenario: Malformed longitude is rejected at parse time

- **GIVEN** an exchange with `CamelRedis.Longitude` set to `-181.0`
- **WHEN** the GEOADD handler resolves headers
- **THEN** the operation fails with `CamelError::ProcessorError` naming
  `CamelRedis.Longitude` before any command is sent to Redis

#### Scenario: Missing member header fails

- **GIVEN** an exchange with key and coordinates but no
  `CamelRedis.Member`
- **WHEN** the GEOADD handler resolves headers
- **THEN** the operation fails with `CamelError::ProcessorError` naming
  `CamelRedis.Member`

### Requirement: GEOPOS returns member positions
The producer SHALL return the stored coordinate pairs for requested members, preserving input order.

#### Scenario: Existing members return coordinate pairs

- **GIVEN** members `Palermo` and `Catania` stored with known coordinates
- **WHEN** GEOPOS runs with `CamelRedis.Members`
  `["Palermo", "Catania"]`
- **THEN** the body is a JSON array of `[longitude, latitude]` pairs
  matching the stored positions within a `1e-4` tolerance (Redis stores
  52-bit geohash coordinates)

#### Scenario: Missing member yields a null entry

- **GIVEN** member `Palermo` stored and member `Nowhere` absent
- **WHEN** GEOPOS runs with `CamelRedis.Members` `["Palermo", "Nowhere"]`
- **THEN** the body is a JSON array where the entry for `Nowhere` is
  `null` and the entry for `Palermo` is its coordinate pair

#### Scenario: Missing key yields null entries

- **GIVEN** no member stored under the key
- **WHEN** GEOPOS runs with any members list
- **THEN** the body is a JSON array of `null` entries, one per requested
  member, and the operation reports success

### Requirement: GEODIST returns the distance between two members
The producer SHALL return the geodesic distance between two stored members in a selectable unit.

#### Scenario: Distance between stored members

- **GIVEN** members `Palermo` and `Catania` stored with known coordinates
- **WHEN** GEODIST runs with `CamelRedis.Member` `Palermo`,
  `CamelRedis.Member2` `Catania`, and `CamelRedis.Unit` `km`
- **THEN** the body is a JSON number greater than `100` (kilometers)

#### Scenario: Missing member yields null

- **GIVEN** member `Palermo` stored and member `Nowhere` absent
- **WHEN** GEODIST runs between `Palermo` and `Nowhere`
- **THEN** the body is JSON `null` and the operation reports success

#### Scenario: Missing key yields null

- **GIVEN** no members stored under the key
- **WHEN** GEODIST runs between any two members
- **THEN** the body is JSON `null` and the operation reports success

#### Scenario: Unit defaults to meters

- **GIVEN** two stored members roughly one hundred kilometers apart
- **WHEN** GEODIST runs without `CamelRedis.Unit`
- **THEN** the body is a number in meters (greater than `100000`)

#### Scenario: Unknown unit is rejected at parse time

- **GIVEN** `CamelRedis.Unit` set to `furlongs`
- **WHEN** the GEODIST handler resolves headers
- **THEN** the operation fails with `CamelError::ProcessorError` naming
  `CamelRedis.Unit` before any command is sent to Redis

### Requirement: GEOSEARCH radius queries return plain members or structured rows
The producer SHALL query members around a center point by radius, returning plain member strings without extras and structured row objects with extras.

#### Scenario: Plain member list stays plain strings

- **GIVEN** members `Palermo` and `Catania` stored under the key
- **WHEN** GEOSEARCH runs from longitude `15.0`, latitude `37.0`,
  `CamelRedis.Radius` `200`, `CamelRedis.Unit` `km`, no extras
- **THEN** the body is a JSON array of plain member strings containing
  both members, with no quoted-JSON blob and no nested objects

#### Scenario: WITHDIST returns structured rows

- **GIVEN** the same stored members
- **WHEN** GEOSEARCH runs with the same center and radius and
  `CamelRedis.WithDist` true
- **THEN** the body is a JSON array of objects, each with a `member`
  string field and a `distance` number field in the requested unit

#### Scenario: WITHCOORD returns structured rows with positions

- **GIVEN** the same stored members
- **WHEN** GEOSEARCH runs with `CamelRedis.WithCoord` true
- **THEN** each row object carries `member`, `longitude`, and `latitude`
  fields matching the stored coordinates within a `1e-4` tolerance

#### Scenario: Missing key yields an empty array

- **GIVEN** no members stored under the key
- **WHEN** GEOSEARCH runs with any valid center and radius
- **THEN** the body is an empty JSON array and the operation reports
  success

#### Scenario: Invalid center is rejected at parse time

- **GIVEN** `CamelRedis.Latitude` set to `91.0`
- **WHEN** the GEOSEARCH handler resolves headers
- **THEN** the operation fails with `CamelError::ProcessorError` naming
  `CamelRedis.Latitude` before any command is sent to Redis

#### Scenario: Non-positive radius is rejected at parse time

- **GIVEN** `CamelRedis.Radius` set to `0`
- **WHEN** the GEOSEARCH handler resolves headers
- **THEN** the operation fails with `CamelError::ProcessorError` naming
  `CamelRedis.Radius` before any command is sent to Redis

### Requirement: GEOSEARCH box queries select by bounding rectangle
The producer SHALL query members inside an axis-aligned rectangle centered on the query point, with radius and box mutually exclusive.

#### Scenario: Box query returns members inside the rectangle

- **GIVEN** members `Palermo` and `Catania` stored with known coordinates
- **WHEN** GEOSEARCH runs from a center between them with
  `CamelRedis.Width` `400`, `CamelRedis.Height` `400`, unit `km`
- **THEN** the body contains both members and excludes a member stored
  outside the rectangle

#### Scenario: Radius and box together are rejected

- **GIVEN** headers carrying both `CamelRedis.Radius` and
  `CamelRedis.Width`/`CamelRedis.Height`
- **WHEN** the GEOSEARCH handler resolves headers
- **THEN** the operation fails with `CamelError::ProcessorError`
  explaining that radius and box are mutually exclusive

#### Scenario: Neither radius nor box is rejected

- **GIVEN** headers carrying a valid center but neither `CamelRedis.Radius`
  nor `CamelRedis.Width`/`CamelRedis.Height`
- **WHEN** the GEOSEARCH handler resolves headers
- **THEN** the operation fails with `CamelError::ProcessorError`
  explaining that a radius or a box is required

### Requirement: GEOHASH returns geohash strings
The producer SHALL return standard base-32 geohash strings for requested members.

#### Scenario: Stored members return geohash strings

- **GIVEN** members `Palermo` and `Catania` stored with known coordinates
- **WHEN** GEOHASH runs with `CamelRedis.Members` `["Palermo", "Catania"]`
- **THEN** the body is a JSON array of two geohash strings (base-32,
  non-empty), parallel to the input order

#### Scenario: Missing member yields a null entry

- **GIVEN** member `Palermo` stored and member `Nowhere` absent
- **WHEN** GEOHASH runs with `CamelRedis.Members` `["Palermo", "Nowhere"]`
- **THEN** the entry for `Nowhere` is `null` and the entry for `Palermo`
  is its geohash string

### Requirement: GEO commands register with correct resolution and retry classification
The five GEO command names SHALL parse from URI and header, and retry classification SHALL treat reads as idempotent and GEOADD as non-idempotent.

#### Scenario: URI and header resolve GEO commands

- **GIVEN** an endpoint URI with `command=GEOSEARCH`
- **WHEN** the producer resolves the command
- **THEN** it yields `RedisCommand::Geosearch`; a
  `CamelRedis.Command` header with `GEOADD` overrides it to
  `RedisCommand::Geoadd` (case-insensitive)

#### Scenario: Reads are idempotent and GEOADD is not

- **WHEN** `is_idempotent_command` classifies the geo variants
- **THEN** `Geopos`, `Geodist`, `Geosearch`, and `Geohash` return true
- **AND** `Geoadd` returns false

### Requirement: Redis cache repository exposes a geo surface
The Redis cache repository SHALL offer typed geo storage and proximity queries over namespaced keys, validating arguments before any command is sent.

#### Scenario: geo_add then radius search returns members with distances

- **GIVEN** a `RedisCacheRepository` connected to a running Redis
- **WHEN** `geo_add` stores `Palermo` and `Catania` under key `sicily`
  and `geo_search_radius` queries from a nearby center within `400` km
- **THEN** both members are returned, each row carrying the member string
  and a positive distance number in the requested unit

#### Scenario: Box search returns members inside the rectangle

- **GIVEN** members stored on both sides of the center
- **WHEN** `geo_search_box` queries with width and height covering them
- **THEN** both members are returned with positive distances

#### Scenario: Keys are namespaced per repository instance

- **GIVEN** two repository instances with different cache names
- **WHEN** one stores a member under key `sicily` and the other searches
  the same logical key
- **THEN** the second repository returns an empty result

#### Scenario: Missing key yields an empty result

- **GIVEN** no members stored under a key
- **WHEN** `geo_search_radius` or `geo_search_box` queries it
- **THEN** the result is an empty vector and the operation reports
  success

#### Scenario: Malformed coordinates are rejected at parse time

- **GIVEN** `geo_add` called with latitude `95.0`
- **WHEN** the repository validates arguments
- **THEN** the call fails with `CamelError::ProcessorError` before any
  command is sent to Redis

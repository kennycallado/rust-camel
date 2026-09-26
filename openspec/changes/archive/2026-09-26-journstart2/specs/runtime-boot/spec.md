# runtime-boot delta — journstart2

## ADDED Requirements

### Requirement: Auto-startup routes start on every boot with a durable journal

When a durable runtime journal is configured, the system SHALL start every
route whose definition has `auto_startup` enabled on every boot, regardless
of route lifecycle events or command IDs recorded in the journal by earlier
boots. Context lifecycle command IDs SHALL be unique per boot by deriving a
boot nonce from the recovered durable dedup store at journal recovery; the
nonce SHALL be a deterministic function of the recorded command IDs (no wall
clock), and no issued command ID SHALL equal a command ID still recorded in
the store. When the deterministic nonce value space is exhausted, the system
SHALL fail the boot with an explicit startup error naming the offending
recorded ID instead of issuing a command ID. Journals written by earlier
versions (four-segment command IDs, including route IDs that themselves
contain colons) SHALL interoperate without suppression.

#### Scenario: Full lifecycle sequence on every boot

- **GIVEN** a durable journal at a fresh path and one route with
  `auto_startup` enabled
- **WHEN** the context boots, stops, and is dropped, then boots again on the
  same journal path, and this cycle repeats once more
- **THEN** every boot appends `RouteRegistered`, `RouteStartRequested`, and
  `RouteStarted` to the journal for that route, and the route reports the
  `Started` state at the end of every boot

#### Scenario: Stop commands are accepted on every boot

- **GIVEN** a durable journal that has already recorded one full boot cycle
  (start and stop) for an auto-startup route
- **WHEN** the context boots again on the same journal and later shuts down
- **THEN** the second boot's `StopRoute` command is accepted (not classified
  as a duplicate) and `RouteStopped` is appended to the journal

#### Scenario: Boot nonce derives from the recorded dedup store

- **GIVEN** a journal whose dedup store records command IDs from prior boots
- **WHEN** a new boot recovers the journal and issues its first context
  command
- **THEN** the command's ID differs from every ID recorded in the store,
  and two recoveries over identical journal state derive the same nonce
  (identical state yields identical nonces, without reading the wall clock;
  full IDs additionally depend on the per-process issuance order)

#### Scenario: Legacy recorded IDs never collide

- **GIVEN** a journal written before this change whose dedup store records
  four-segment context command IDs (`context:{op}:{route_id}:{seq}`),
  including at least one whose route ID contains a colon so the recorded
  string's final segments are numerically ambiguous (for example
  `context:start:foo:0:0` for route `foo:0`)
- **WHEN** a new boot derives its boot nonce and issues five-segment IDs
  (`context:{op}:{route_id}:{nonce}:{seq}`)
- **THEN** the chosen nonce is strictly greater than the penultimate segment
  of every recorded ID whose final two segments both parse as integers, so
  no issued ID equals a recorded ID, and every auto-startup route's
  `StartRoute` command is accepted rather than classified as a duplicate

#### Scenario: Exhausted deterministic nonce space

- **GIVEN** a journal whose dedup store records a command ID whose final two
  segments both parse as integers and whose penultimate segment equals the
  maximum 64-bit unsigned integer value
- **WHEN** a new boot derives its boot nonce
- **THEN** the boot fails closed: an explicit startup error names the
  offending recorded ID and tells the operator to clean or rotate the
  journal, and no context command ID is issued for that boot

#### Scenario: No journal configured changes lifecycle outcomes

- **GIVEN** a context built without a runtime journal
- **WHEN** the context boots
- **THEN** auto-startup proceeds through the same command path with the same
  lifecycle outcomes as before this change (no journal recovery runs; the
  command ID string gains only a constant zero nonce segment)

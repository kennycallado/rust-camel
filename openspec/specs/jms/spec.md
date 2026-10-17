# jms Specification

## Purpose
The camel-jms component and its bridge. It pins reconnect defaults, endpoint
URI validation, broker URL redaction, client-acknowledge bridge consumption,
durable reservation accounting and journaling, poison-message policy, producer
header allowlisting, and native-image `jms.*` options.

## Requirements
### Requirement: JMS reconnect defaults remain explicit

The JMS configuration unit suite SHALL pin every field overridden by `jms_reconnect_default`.

#### Scenario: Reconnect policy contains documented defaults

- **GIVEN** the component reconnect default
- **WHEN** `jms_reconnect_default()` is evaluated
- **THEN** `max_attempts` is 0, `initial_delay` is 5 seconds, `multiplier` is 2.0, `max_delay` is 30 seconds, and `jitter_factor` is 0.0; `multiplier` and `max_delay` are also recorded as equivalent to their `NetworkRetryPolicy::default()` values

### Requirement: JMS endpoint URI validation rejects ambiguous input

The JMS configuration unit suite SHALL verify endpoint URI scheme, destination-name, shorthand, and boundary validation.

#### Scenario: Unsupported scheme is rejected

- **GIVEN** a URI without the `jms`, `activemq`, or `artemis` scheme
- **WHEN** `JmsEndpointConfig::from_uri` parses it
- **THEN** parsing fails with the expected scheme error

#### Scenario: Empty destination name is rejected

- **GIVEN** `jms:queue:`, `jms:topic:`, or bare `jms:` with no destination name
- **WHEN** `JmsEndpointConfig::from_uri` parses it
- **THEN** parsing fails with the destination-format error rather than the `jms:` shorthand ambiguity error

#### Scenario: JMS shorthand is rejected as ambiguous

- **GIVEN** a non-empty `jms:<name>` URI
- **WHEN** `JmsEndpointConfig::from_uri` parses it
- **THEN** parsing fails with guidance to use an explicit queue or topic prefix

#### Scenario: Validation boundary has the documented polarity

- **GIVEN** a priority query value of 9 or 10
- **WHEN** `JmsEndpointConfig::from_uri` parses them
- **THEN** priority 9 is accepted and priority 10 is rejected

### Requirement: JMS bridge cache default remains concrete

The JMS configuration unit suite SHALL verify that `default_bridge_cache_dir` delegates to the concrete bridge download cache default.

#### Scenario: Bridge cache default is not an empty path

- **GIVEN** the process environment used by the bridge cache helper
- **WHEN** `default_bridge_cache_dir()` is evaluated
- **THEN** it equals the concrete helper result and differs from `PathBuf::default()`

### Requirement: Broker URL redaction keeps benign query keys and masks credential-shaped values

The camel-jms broker URL redaction path (`redact_broker_url`, backed by
the canonical `camel_api::redact` query-allowlist variant) SHALL mask
userinfo in every authority window, SHALL redact any query pair whose
key — single-pass percent-decoded (`%HH`, hex case-insensitive) and
lowercased — contains a sensitive substring (`password`, `passwd`,
`secret`, `credential`, `token`, `username`, `user`) as
`key=<redacted>` (the rendered key keeps its original encoded bytes,
except under the key-position credential-shape exception below),
and SHALL keep all other pairs visible — ActiveMQ failover URIs encode
non-secret transport policy in query parameters, the sole diagnostic
value of the broker URL in Debug output (documented exception to
whole-query drop). The fragment SHALL never echo: everything from the
first `#` is dropped and replaced with the `#[redacted]` sentinel. The
result SHALL cap at 256 bytes on a UTF-8 character boundary with the
sentinel budget reserved before the cap.

A kept (benign-keyed) pair SHALL render as `<redacted>` when its
single-pass minimal decode — `%40` to `@`, `%3a` to `:`, `%2f` to `/`,
case-insensitive, applied to the whole pair — contains `@` AND (`:` OR
`//`). This closes percent-encoded credential smuggling under benign
keys (bd rc-r7v8s). A value whose decode carries `@` but neither `:` nor
`//` — an email address such as `contact=admin%40corp.example` — SHALL
stay visible; over-masking a `user@host:port`-shaped value is accepted
(ADR-0051: over-masking is safe, under-masking is not).

A redacted (denylist-matched) pair whose raw key's single-pass minimal
decode — `%40` to `@`, `%3a` to `:`, `%2f` to `/`, case-insensitive —
contains `@` AND (`:` OR `//`) SHALL render as a bare `<redacted>`:
the key never echoes, because the key bytes may themselves embed the
credential (`user%3Asecret%40host` decodes to `user:secret@host`).
Otherwise the denylist-matched pair renders as `{raw_key}=<redacted>`
— well-known JMS/ActiveMQ parameter names never decode to the
credential shape, so the transport-policy diagnostic value of the
broker URL survives intact (ADR-0076 appendix, key-position
credential-shape symmetry, e_opus ruling OPTION B; bd rc-tfugr).

#### Scenario: percent-encoded credentials under a benign key are masked

- **GIVEN** `tcp://h:61616?redirect=http%3A%2F%2Fuser%3Asecret%40host`
  and its fully-lowercase variant
  `tcp://h:61616?redirect=http%3a%2f%2fuser%3asecret%40host`
- **WHEN** each passes through the broker URL redaction path
- **THEN** neither `secret` nor `user%3Asecret%40` renders — the
  `redirect` pair renders as `<redacted>`, and the broker host and port
  stay visible for failover diagnosis

#### Scenario: literal credential shapes under a benign key are masked

- **GIVEN** `tcp://h:61616?next=%2F%2Fuser:pass@host` (encoded slashes,
  literal `@`) and `tcp://h:61616?next=user:pass@host` (fully literal)
- **WHEN** each passes through the broker URL redaction path
- **THEN** neither `pass` nor `user:pass` renders — each `next` pair
  renders as `<redacted>`

#### Scenario: lone percent-encoded email value stays visible

- **GIVEN** `tcp://h:61616?contact=admin%40corp.example`
- **WHEN** it passes through the broker URL redaction path
- **THEN** the pair stays visible — `@` without `:` or `//` is not
  credential-shaped

#### Scenario: sensitive keys redact regardless of value shape

- **GIVEN** `tcp://host:61616?password=p&user=u&keepAlive=true` and
  `tcp://host:61616?pass%77ord=shortsecret` (percent-encoded key that
  decodes to `password`)
- **WHEN** each passes through the broker URL redaction path
- **THEN** the first output carries `password=<redacted>` and
  `user=<redacted>` while `keepAlive=true` stays visible; the second
  output carries `pass%77ord=<redacted>` — the decoded key matches, the
  secret value never renders

#### Scenario: credential-shaped kept value is masked

- **GIVEN** `tcp://h:61616?next=user%40host%3Aport` — a benign key
  whose value decodes to `user@host:port` (`@` plus `:`)
- **WHEN** it passes through the broker URL redaction path
- **THEN** the pair renders as `<redacted>` — over-masking is accepted
  per ADR-0051 (over-masking is safe, under-masking is not)

#### Scenario: fragment never echoes and the cap holds

- **GIVEN** a broker URL with a fragment and a kept query longer than
  256 bytes combined
- **WHEN** it passes through the broker URL redaction path
- **THEN** the output carries `#[redacted]` (never fragment bytes), is
  at most 256 bytes, cuts on a UTF-8 character boundary, and the
  sentinel renders complete

#### Scenario: credential-shaped key never echoes

- **GIVEN** `tcp://h:61616?user%3Asecret%40host=1`, its
  fully-lowercase variant `tcp://h:61616?user%3asecret%40host=1`, and
  the literal form `tcp://h:61616?user:pass@host=1`
- **WHEN** each passes through the broker URL redaction path
- **THEN** each renders as a bare `<redacted>` — neither `secret`,
  `user%3Asecret%40`, nor `user:pass` echoes in any position

#### Scenario: non-shaped sensitive keys keep their key names

- **GIVEN** `tcp://h:61616?password=p&jms.userName=admin&user=u&keepAlive=true`,
  `tcp://h:61616?user@host=1` (denylist-matched key with a lone `@`),
  and `tcp://h:61616?pass%77ord=p`
- **WHEN** each passes through the broker URL redaction path
- **THEN** the first renders
  `password=<redacted>&jms.userName=<redacted>&user=<redacted>&keepAlive=true`,
  the second renders `user@host=<redacted>`, and the third renders
  `pass%77ord=<redacted>` — keys that are not credential-shaped keep
  their authored bytes

### Requirement: JMS bridge consumer settles each delivery after route completion

The JMS bridge consumer SHALL open the session with `CLIENT_ACKNOWLEDGE` and
SHALL hold at most one unacknowledged delivery per consumer session. It SHALL
attach a unique reservation token and a subscription generation to each
forwarded delivery. The Rust consumer SHALL submit the route with
`send_and_wait` rather than fire-and-forget `send`. A unary `Complete` RPC SHALL
validate the subscription, the generation, and the token, and SHALL hand the
decision to the owner thread, which is the only thread that touches the
session. A stop SHALL wake the owner thread.

Settlement SHALL linearize at the owner thread's terminal decision for the
current reservation. A validated `COMMIT` SHALL mark the reservation committed
and acknowledge exactly once. A validated `ROLLBACK` SHALL recover the session.
A stop, a cancel, or a disconnect observed before the decision SHALL prevent
any acknowledgement or recovery. A stop, a cancel, or a disconnect observed
after the decision SHALL NOT undo it; the session closes and an executed
acknowledgement stands. Settlement SHALL be idempotent and terminal per
reservation: a duplicate or late completion SHALL never produce a second
acknowledgement. When a `COMMIT` executes and its response is lost, the
acknowledgement SHALL stand; the Rust consumer SHALL NOT retry the completion
and SHALL NOT produce a speculative acknowledgement. The Rust consumer SHALL
drop the stream when a completion fails or is uncertain.

On the Rust side, `send_and_wait` SHALL map `Ok` to `COMMIT`. It SHALL map
`Err`, including `ChannelClosed`, to `ROLLBACK` or a stream drop, and SHALL
never map an error to `COMMIT`. The `b-prime:jms:consumer-send` metric and the
`outside-contract` log annotation SHALL stay in place.

A message conversion failure SHALL count as a reservation failure. The owner
thread SHALL recover the session before it receives another message, and SHALL
NOT receive another message until recover completes. If recover fails, the
owner thread SHALL close the session without acknowledging. `Completed`,
`Stopped`, and a handled error SHALL all count as success (ADR-0024). There
SHALL be no completion deadline. Teardown SHALL trigger only on cancel,
disconnect, stop, or shutdown.

`acknowledgementMode` and `transactionMode` SHALL NOT change the bridge-managed
settlement. The bridge SHALL always use `CLIENT_ACKNOWLEDGE` with completion
settlement. `transactionMode=Session` SHALL keep a startup warning that names
the bridge-managed settlement as the effective behavior.

#### Scenario: Successful route commits exactly once

- **GIVEN** a route that completes or stops successfully
- **WHEN** the Rust consumer reports `COMMIT` for the current token and generation
- **THEN** the bridge SHALL acknowledge the message exactly once
- **AND** a duplicate completion for the same reservation SHALL NOT acknowledge again

#### Scenario: Propagated route error rolls back

- **GIVEN** a route that returns a propagated error
- **WHEN** the Rust consumer reports `ROLLBACK` for the current reservation
- **THEN** the bridge SHALL recover the session
- **AND** the message SHALL NOT be acknowledged
- **AND** the message SHALL be redelivered by the broker or provider

#### Scenario: ChannelClosed never commits

- **GIVEN** a `send_and_wait` that returns `Err(ChannelClosed)`
- **WHEN** the Rust consumer classifies the result
- **THEN** it SHALL report `ROLLBACK` or drop the stream
- **AND** it SHALL NOT report `COMMIT`

#### Scenario: Handled error and Stop commit

- **GIVEN** a route where an error handler absorbs a failure, or a Stop step halts the route
- **WHEN** the pipeline result reaches the Rust consumer
- **THEN** the result SHALL be a success
- **AND** the bridge SHALL acknowledge the message

#### Scenario: Cancel before the decision prevents acknowledgement

- **GIVEN** a delivered reservation with no terminal decision yet
- **WHEN** a stop, cancel, or disconnect reaches the owner thread
- **THEN** the bridge SHALL close the session without acknowledging or recovering
- **AND** the broker SHALL redeliver the message

#### Scenario: Cancel after the decision does not undo it

- **GIVEN** an owner thread that already decided `COMMIT` and acknowledged
- **WHEN** a stop, cancel, or disconnect follows
- **THEN** the acknowledgement SHALL stand
- **AND** the session SHALL close without a second acknowledgement

#### Scenario: Lost commit response still stands

- **GIVEN** a `COMMIT` that the owner thread executed successfully
- **WHEN** the completion response is lost and the Rust consumer sees an error
- **THEN** the acknowledgement SHALL stand
- **AND** the Rust consumer SHALL NOT retry the completion and SHALL NOT speculate a second acknowledgement

#### Scenario: Conversion failure recovers before the next receive

- **GIVEN** a message that fails conversion
- **WHEN** the owner thread counts the reservation failure
- **THEN** it SHALL recover the session before receiving another message
- **AND** it SHALL NOT receive another message until recover completes
- **AND** if recover fails it SHALL close the session without acknowledging

#### Scenario: Bridge-managed settlement ignores endpoint options

- **GIVEN** an endpoint with `acknowledgementMode=Auto` or `transactionMode=Session`
- **WHEN** the consumer starts
- **THEN** the bridge SHALL use `CLIENT_ACKNOWLEDGE` with completion settlement
- **AND** `transactionMode=Session` SHALL warn that bridge-managed settlement is effective

#### Scenario: A slow healthy route stays pending

- **GIVEN** a delivered message and a route that takes a long time to finish
- **WHEN** the stream stays healthy and no cancel, disconnect, stop, or shutdown occurs
- **THEN** the bridge SHALL keep the reservation pending
- **AND** it SHALL NOT apply a completion deadline

### Requirement: Durable reservation accounting scope and identity

The bridge SHALL account each delivery as a durable reservation, not as a
completed handoff. A reservation SHALL be recorded before the message leaves
the bridge and SHALL include conversion failures and crashes between the record
and the handoff. The accounting domain SHALL be stable across reconnects and
SHALL be scoped to the broker and the destination. The bridge SHALL key a
reservation by the provider `JMSMessageID` and SHALL NOT key it by an exchange
header or an ephemeral subscription identifier.

Rust SHALL derive the broker key in one place as a stable hash of the broker
name, the broker type, and the redacted broker URL. Rust SHALL be the single
source of the default resolution of the journal directory, verified by a
repository source scan: `$XDG_STATE_HOME/rust-camel/jms-bridge` when
`XDG_STATE_HOME` is set, else `$HOME/.local/state/rust-camel/jms-bridge`. When
neither variable resolves, Rust SHALL fail at startup. Rust SHALL pass the
resolved directory (`JMS_ATTEMPT_JOURNAL_DIR`) and the broker key
(`JMS_BRIDGE_BROKER_KEY`) to the bridge through named environment variables on
the initial launch and on every restart launch. Java SHALL NOT re-derive the
default. Neither a raw broker URL nor a raw message ID SHALL be used as a file
path. The journal path SHALL be
`<JMS_ATTEMPT_JOURNAL_DIR>/<broker-key>/<destination-key>.journal`.

The journal domain SHALL be the tuple (`JMS_ATTEMPT_JOURNAL_DIR`, broker key,
destination key) with exactly one live owner at a time. Budgets SHALL be shared
concurrently only within the owning bridge process. A sequential owner that
acquires the domain through lock handoff SHALL inherit the retained counts. A
reservation SHALL be one durable delivery-attempt record; the
`JMS_ATTEMPT_JOURNAL_*` variables and `JMS_MAX_DELIVERY_ATTEMPTS` SHALL count
reservations. Cross-process or cross-host duplicates SHALL be documented as
bounded at N times `JMS_MAX_DELIVERY_ATTEMPTS`, where N SHALL be the number of
distinct journal domains — `(directory, broker key, destination key)` tuples —
consuming the queue within one retention window.
Durable topic subscriptions are unsupported. For topic destinations the bridge
SHALL use the same completion settlement with a subscription-scoped volatile
counter, and SHALL document that the topic poison cap is not crash-stable.

The directory and its files SHALL be owner-private (mode 0700 and 0600 on
POSIX). Producer-only bridges and topic consumption SHALL NOT touch the journal
or take its lock. When `JMS_ATTEMPT_JOURNAL_DIR` is unset or
`JMS_BRIDGE_BROKER_KEY` is unset, the bridge SHALL report readiness and SHALL
serve producers and topic consumption; a queue subscribe SHALL then fail with
gRPC `FAILED_PRECONDITION` naming `JMS_ATTEMPT_JOURNAL_DIR` or
`JMS_BRIDGE_BROKER_KEY`. When either variable is set but invalid
(non-resolvable, not owner-private, or a symlink escape), the bridge SHALL treat
it as a startup permanent failure.

#### Scenario: Consumers of one bridge process share one budget

- **GIVEN** two consumers on one queue served by one bridge process
- **WHEN** one message is redelivered between them
- **THEN** the reservation count SHALL be shared
- **AND** the cap SHALL apply to the queue domain

#### Scenario: Cross-process duplicates are bounded

- **GIVEN** two bridge processes with distinct journal directories consuming one queue
- **WHEN** the same message is redelivered
- **THEN** each directory SHALL keep an independent budget
- **AND** the total SHALL be bounded at N times the configured maximum, where N is the number of distinct journal domains (directory, broker key, destination key) consuming the queue within one retention window

#### Scenario: Unset env defers the failure to queue subscribe

- **GIVEN** a bridge started with `JMS_ATTEMPT_JOURNAL_DIR` unset or `JMS_BRIDGE_BROKER_KEY` unset
- **WHEN** it starts and a producer or topic path runs
- **THEN** readiness SHALL succeed and those paths SHALL work
- **AND** a queue subscribe SHALL fail with `FAILED_PRECONDITION` naming the missing variable

#### Scenario: Invalid env is a startup permanent failure

- **GIVEN** a journal directory that is non-resolvable, not owner-private, or a symlink escape
- **WHEN** the bridge starts
- **THEN** it SHALL signal a startup permanent failure
- **AND** it SHALL NOT report readiness

#### Scenario: Sequential owners inherit counts

- **GIVEN** one journal domain released by a previous owner
- **WHEN** a new owner acquires the domain through lock handoff
- **THEN** it SHALL inherit the retained reservation counts

#### Scenario: Rust owns the key and the default directory

- **GIVEN** a broker name, type, and redacted URL
- **WHEN** Rust launches the bridge
- **THEN** Rust SHALL pass the derived broker key and the resolved directory as environment variables
- **AND** Java SHALL NOT derive a default directory

#### Scenario: Broker and destination namespaces stay separate

- **GIVEN** two brokers and two queues
- **WHEN** each records a reservation
- **THEN** each SHALL use its own broker and destination namespace

#### Scenario: No raw identifiers in paths

- **GIVEN** a credential-bearing broker URL and a raw message ID
- **WHEN** the bridge derives the journal path
- **THEN** neither value SHALL appear in a file path

#### Scenario: Producer and topic paths never lock

- **GIVEN** a producer-only bridge or a topic subscription
- **WHEN** it runs
- **THEN** it SHALL NOT create, lock, or replay a queue journal

#### Scenario: Topic scope is explicit

- **GIVEN** a topic destination
- **WHEN** the bridge consumes it
- **THEN** it SHALL use a subscription-scoped volatile counter
- **AND** it SHALL document that the cap is not crash-stable

### Requirement: Lazy per-destination journal ownership

The bridge SHALL acquire journal ownership lazily, per queue destination
journal, at the first subscribe for that domain and before the first receive.
It SHALL hold the lock while the process has an active subscription to that
domain and SHALL release it after the last subscription to that domain ends,
including on stream cancellation, disconnect, stop, and shutdown. The bridge
SHALL never replace the lock file. When the journal directory variable is set,
readiness SHALL validate only that directory: resolvable, owner-private, and not
a symlink escape. When it is unset, readiness SHALL still succeed and the
queue-subscribe `FAILED_PRECONDITION` SHALL apply. It SHALL NOT open, lock, or
replay a journal before a subscribe. The lock SHALL be an OS file lock that the
kernel releases at process exit. On the orphan path, where the client process is
killed, the bridge SHALL release the lock when the gRPC stream ends.

When the destination lock is busy, the bridge SHALL wait a bounded,
configurable time (`JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS`, default 30000) and then
fail the subscribe with gRPC `FAILED_PRECONDITION` naming
`JMS_ATTEMPT_JOURNAL_DIR`. `JMS_ATTEMPT_JOURNAL_MAX_BYTES` (default 4 MiB) SHALL
be per destination journal. Concurrent bridge processes consuming one queue
SHALL use distinct journal domains, and a journal domain SHALL have one live
owner. Within one bridge process, multiple consumers of a destination SHALL
share that domain and its reservation count; they SHALL NOT require distinct
domains. When two processes resolve the same journal domain, the contender
SHALL wait the bounded `JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS` and SHALL fail closed
only when that wait expires.

#### Scenario: Lock is acquired at first subscribe

- **GIVEN** a bridge that has not yet subscribed
- **WHEN** the first subscribe for a queue domain arrives
- **THEN** the bridge SHALL acquire that destination journal before the first receive
- **AND** producer-only bridges SHALL NOT acquire it

#### Scenario: Lock is released when the last subscription ends

- **GIVEN** an active queue subscription holding the journal lock
- **WHEN** the stream is cancelled, disconnects, stops, or the bridge shuts down
- **THEN** the bridge SHALL release the destination lock
- **AND** a later subscriber SHALL acquire it and observe the recorded count

#### Scenario: Busy lock fails after a bounded wait

- **GIVEN** a destination journal locked by another process
- **WHEN** a subscribe waits longer than `JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS`
- **THEN** the subscribe SHALL fail with `FAILED_PRECONDITION`
- **AND** the message SHALL name `JMS_ATTEMPT_JOURNAL_DIR`

#### Scenario: Readiness validates only the directory

- **GIVEN** a start with a resolvable, owner-private directory
- **WHEN** the bridge reports readiness
- **THEN** it SHALL NOT have opened, locked, or replayed a journal
- **AND** a symlinked or non-resolvable directory SHALL fail closed
- **AND** an unset directory SHALL still report readiness

#### Scenario: Same journal domain forces one owner

- **GIVEN** two processes that resolve the same journal domain (same directory, broker key, and destination key)
- **WHEN** both subscribe
- **THEN** one SHALL hold the lock
- **AND** the other SHALL wait the bounded lock time and SHALL fail closed only when it expires

#### Scenario: Kernel releases the lock at process exit

- **GIVEN** a bridge holding a destination OS file lock
- **WHEN** the bridge process exits
- **THEN** the kernel SHALL release the lock
- **AND** a later owner SHALL acquire the domain

#### Scenario: Orphan bridge releases on stream end

- **GIVEN** an orphaned bridge whose client was killed
- **WHEN** the gRPC stream ends
- **THEN** the bridge SHALL release the destination lock

### Requirement: Journal durability, retention, and storage bounds

The bridge SHALL provide one process-wide serialized journal service that owns
every journal write. The service SHALL write framed, versioned records. On
startup of a journal it SHALL replay records and SHALL treat an incomplete
trailing frame as a torn tail: it SHALL discard only that partial frame and
SHALL keep every previously confirmed count. Replay SHALL NOT reset a confirmed
count.

The service SHALL make an update durable before the message leaves the bridge:
it SHALL append the reservation record and fsync. Compaction SHALL run when a
journal opens and before any fail-closed decision at the ceiling. Compaction
SHALL write a fsynced temporary snapshot, atomically replace the journal, and
fsync the parent directory. The service SHALL reserve append and removal space
before it writes. The byte ceiling SHALL include temporary snapshots, and the
snapshot peak SHALL be bounded so the service cannot fill the ceiling and get
stuck full.

The bridge SHALL validate its configuration at startup and SHALL treat an
out-of-range value as a startup permanent failure:
`JMS_ATTEMPT_JOURNAL_RETENTION_MS` SHALL be in `[60000, 31536000000]` (default
86400000); `JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS` SHALL be in `[100, 300000]`
(default 30000); `JMS_ATTEMPT_JOURNAL_MAX_BYTES` SHALL be in `[65536, 268435456]`
(default 4194304); `JMS_MAX_DELIVERY_ATTEMPTS` SHALL be in `[1, 100]` (default
6).

A reservation SHALL carry a last-touched time that each new receipt refreshes.
During compaction the service SHALL retire entries untouched longer than a
validated retention window (`JMS_ATTEMPT_JOURNAL_RETENTION_MS`, default
86400000). Retirement is distinct from live eviction. Retirement SHALL NOT
apply to the reservation of the message currently being processed. A message
absent longer than the window SHALL restart its reservation budget, and the
documentation SHALL state that the cap is not an absolute forever cap. The
bridge SHALL fail closed only when the ceiling is reached by unretirable
entries. The bridge SHALL NOT evict a live reservation to admit another.

#### Scenario: Torn tail keeps confirmed counts

- **GIVEN** a journal whose last frame is incomplete
- **WHEN** the service replays it
- **THEN** it SHALL discard only the partial frame
- **AND** every earlier confirmed count SHALL remain

#### Scenario: Snapshot replacement is atomic and durable

- **GIVEN** a compaction
- **WHEN** the snapshot replaces the journal
- **THEN** the snapshot SHALL be fsynced, the replacement SHALL be atomic, and the parent directory SHALL be fsynced

#### Scenario: Space is reserved and the ceiling includes snapshots

- **GIVEN** a journal near its per-destination ceiling
- **WHEN** an append or a compaction must run
- **THEN** the service SHALL reserve the needed space first
- **AND** the ceiling SHALL count the temporary snapshot peak

#### Scenario: Stale entries retire at compaction

- **GIVEN** an entry untouched longer than `JMS_ATTEMPT_JOURNAL_RETENTION_MS`
- **WHEN** compaction runs
- **THEN** the service SHALL retire the entry
- **AND** a later receipt of that message SHALL restart its budget

#### Scenario: The current reservation is never retired

- **GIVEN** an in-flight reservation older than the retention window
- **WHEN** compaction runs
- **THEN** the service SHALL keep it

#### Scenario: No live eviction

- **GIVEN** a full journal of unretirable entries
- **WHEN** a new reservation must be recorded
- **THEN** the bridge SHALL fail closed
- **AND** it SHALL NOT evict a live reservation

#### Scenario: Compaction runs at open and before the ceiling decision

- **GIVEN** a journal opened by the service
- **WHEN** the service opens it or reaches the ceiling decision
- **THEN** compaction SHALL run before the decision
- **AND** retirement SHALL be attempted before any fail-closed outcome

#### Scenario: Out-of-range configuration is a startup permanent failure

- **GIVEN** a value outside the validated range for a journal or cap variable
- **WHEN** the bridge starts
- **THEN** it SHALL signal a startup permanent failure
- **AND** it SHALL NOT report readiness

#### Scenario: The cap is not absolute forever

- **GIVEN** a message absent longer than the retention window
- **WHEN** it is received again
- **THEN** its reservation budget SHALL restart
- **AND** the documentation SHALL state this bound

### Requirement: Reservation threshold and reject-then-log poison policy

The bridge SHALL permit at most `JMS_MAX_DELIVERY_ATTEMPTS` durable
reservations per `JMSMessageID` (a validated configurable maximum, default 6).
On a receipt whose reservation count would exceed the maximum, the bridge SHALL
apply the terminal policy without a handoff. The terminal policy SHALL be
reject-then-log: acknowledge the message once and log at `error!` with the
message ID and reservation count only, never the body or headers. The bridge
SHALL NOT forward to a dead-letter destination in 0.8.0.

The bridge cap is the documented terminal only where it fires before any
provider or broker native redelivery or expiry limit. The documentation SHALL
name the broker-native limit as taking precedence where it fires first. A
redelivery SHALL NOT be promised to land on the source queue; the bridge SHALL
assert only that the message is not acknowledged and is made available again
per broker or provider semantics. The discard SHALL be an explicit, documented
terminal exception to at-least-once, applied to every message type, and
recorded in `docs/src/components/jms.md`, `bridges/jms/README.md`, and the
ADR-0067 policy note. Repeated crashes before handoff can exhaust the
reservation budget before any route runs; the bridge SHALL document that
consequence.

#### Scenario: Six reservations then terminal

- **GIVEN** a message rolled back on every reservation
- **WHEN** the bridge receives it a seventh time
- **THEN** it SHALL NOT hand it off again
- **AND** it SHALL apply reject-then-log

#### Scenario: Terminal is acknowledge plus one error log

- **GIVEN** a message that reaches the maximum
- **WHEN** the bridge applies the terminal policy
- **THEN** it SHALL acknowledge exactly once
- **AND** it SHALL log the message ID and reservation count at `error!`
- **AND** the log SHALL contain no body bytes and no header names or values

#### Scenario: Bridge cap precedence is explicit

- **GIVEN** a broker-native redelivery or expiry limit that fires before six reservations
- **WHEN** the message is disposed
- **THEN** the documentation SHALL name the native limit as taking precedence
- **AND** the bridge SHALL NOT claim its terminal policy ran

#### Scenario: Redelivery destination is not guaranteed

- **GIVEN** a rolled-back message
- **WHEN** the broker redelivers it
- **THEN** the bridge SHALL assert only that it was not acknowledged and was made available again
- **AND** it SHALL NOT assert the source queue as the destination

#### Scenario: Behavior is verified with both brokers

- **GIVEN** a Dockerized ActiveMQ Classic or Artemis broker
- **WHEN** phase 5 runs the threshold and disposition tests
- **THEN** each observed terminal disposition SHALL be recorded
- **AND** a mismatch with the documented precedence SHALL be reported, not passed

### Requirement: Permanent failures park the slot or fail only the route

The bridge SHALL distinguish permanent failures from transient transport
failures. A startup journal-directory failure SHALL be permanent. The bridge
SHALL signal a startup permanent failure with a reserved, distinguishable
signal: a ready-protocol line `{"status":"permanent-failure",...}` and a
reserved non-zero exit code. The Rust pool SHALL map that signal to a new
terminal `Parked` state and SHALL never route it through `Restarting`. Slot
park SHALL apply only to startup and directory failures. The parked slot SHALL
stay parked until an explicit operator repair or restart. `Parked` SHALL be
terminal for every waiter: `await_ready_channel`, the consumer pre-flight probe,
and producer sends SHALL fail fast with an error naming the operator repair, and
`get_or_create_slot` and the health monitor SHALL NOT respawn a `Parked` slot.

Subscribe-time and consumption-time permanent failures (lock, replay, capacity,
or unkeyable `JMSMessageID`) SHALL fail only the affected route through gRPC
`FAILED_PRECONDITION`; the slot SHALL stay `Ready`. The bridge SHALL preserve
the unacknowledged broker message, SHALL NOT manufacture an empty or shared
message ID, and SHALL NOT acknowledge an unkeyable delivery.

Any consumer task exit other than a requested cancellation SHALL cancel its
sibling tasks and SHALL surface as `Err` through the supervised background-task
handle, so route supervision records the failure (ADR-0007). The exit trigger
SHALL include a permanent failure, a panic, and reconnect exhaustion. A
transient error that reconnects inside the task SHALL NOT count as an exit. A
requested cancellation SHALL be normal and SHALL NOT surface as `Err`. A
supervision restart against a still-broken journal SHALL fail start again
without a hot loop, via the pre-flight slot probe or a subscribe-time
`FAILED_PRECONDITION`. Transient transport reconnect SHALL stay unchanged. The
operator recovery action SHALL be documented: stop the route, repair or remove
the corrupt journal or the unkeyable source, then restart the context.

#### Scenario: Startup permanent failure parks the slot

- **GIVEN** a bridge that reports `{"status":"permanent-failure"}` and a reserved exit code
- **WHEN** the pool starts the slot
- **THEN** the slot SHALL become `Parked`
- **AND** it SHALL never enter `Restarting`
- **AND** the bridge SHALL NOT acknowledge

#### Scenario: Waiter on a Parked slot fails fast

- **GIVEN** a `Parked` slot
- **WHEN** `await_ready_channel`, the consumer pre-flight probe, or a producer send waits on it
- **THEN** each SHALL return `Err` within a bounded time
- **AND** the error SHALL name the operator repair
- **AND** the pool and monitor SHALL NOT respawn the slot

#### Scenario: Destination permanent failure fails only the route

- **GIVEN** a subscription whose lock, replay, or capacity failure is permanent
- **WHEN** it fails
- **THEN** that route SHALL fail with `FAILED_PRECONDITION`
- **AND** the broker slot SHALL stay `Ready`
- **AND** other routes and producers on that slot SHALL keep working

#### Scenario: Any consumer task failure cancels siblings

- **GIVEN** concurrent consumer tasks on one route
- **WHEN** one task exits for a permanent failure, a panic, or reconnect exhaustion
- **THEN** it SHALL cancel its siblings
- **AND** the supervised handle SHALL return `Err` so supervision records the failure

#### Scenario: Requested cancellation is normal

- **GIVEN** concurrent consumer tasks on one route
- **WHEN** the route requests cancellation
- **THEN** the tasks SHALL stop
- **AND** the supervised handle SHALL NOT surface `Err`

#### Scenario: Restart against a broken journal does not hot-loop

- **GIVEN** a still-broken journal
- **WHEN** supervision restarts the route
- **THEN** the start SHALL fail again through the pre-flight probe or a subscribe-time `FAILED_PRECONDITION`
- **AND** it SHALL NOT loop without bound

#### Scenario: Unkeyable ID preserves the message

- **GIVEN** a broker message with no usable `JMSMessageID`
- **WHEN** the bridge would reserve it
- **THEN** it SHALL terminate the subscription without acknowledging
- **AND** it SHALL preserve the message for the broker to redeliver
- **AND** it SHALL NOT manufacture an ID and SHALL NOT force a discard

#### Scenario: Transient transport failures still reconnect

- **GIVEN** a transient transport failure
- **WHEN** the consumer reconnects
- **THEN** the existing bounded reconnect behavior SHALL apply

#### Scenario: Operator recovery is documented

- **GIVEN** a parked slot
- **WHEN** a reader consults the documentation
- **THEN** it SHALL state the repair or removal step and the required restart

### Requirement: JMS bridge crash before route completion preserves the message

A crash of the Rust consumer process before route completion SHALL leave the
message unacknowledged, and the broker SHALL redeliver it. The proof contract
is: `jms_bridge_crash_before_completion_redelivers_activemq` and
`jms_bridge_crash_before_completion_redelivers_artemis` run against a dedicated,
non-shared broker. An isolated crash-probe child process consumes one message
with a route that writes a handoff marker as its first step and then blocks.
The test SHALL wait for the marker, send `SIGKILL` to the probe, and then
assert that a surviving consumer receives the same message with an unchanged
`JMSMessageID`, body bytes, and user headers, an increased reservation count,
and no observed `COMMIT`. The survivor SHALL acquire the destination journal
and observe the recorded reservation count. The bridge session SHALL close, or
the bridge SHALL restart, and either SHALL leave the message unacknowledged.
The crash-test harness SHALL reap the orphan bridge after its assertions.
Parent-death binding for orphaned bridges is out of scope.

#### Scenario: Kill at the handoff barrier redelivers the message

- **GIVEN** an isolated probe that has written the handoff marker for one message
- **WHEN** the test sends `SIGKILL` to the probe before route completion
- **THEN** a surviving consumer SHALL receive the same message
- **AND** its `JMSMessageID`, body bytes, and user headers SHALL be unchanged
- **AND** no `COMMIT` SHALL have been observed

#### Scenario: Survivor acquires the journal and sees the count

- **GIVEN** the killed probe held the destination journal
- **WHEN** the survivor subscribes
- **THEN** it SHALL acquire the journal
- **AND** it SHALL observe the reservation count recorded before the handoff

#### Scenario: The harness reaps the orphan bridge

- **GIVEN** a crash test that killed the probe and left an orphan bridge
- **WHEN** the test finishes its assertions
- **THEN** the harness SHALL reap the orphan bridge
- **AND** it SHALL leave no process holding the destination lock

#### Scenario: Classic and Artemis coverage

- **GIVEN** the crash scenario
- **WHEN** the suite runs it on ActiveMQ Classic and on Artemis
- **THEN** both runs SHALL record the observed redelivery disposition
- **AND** a gap in broker semantics SHALL be reported, not passed

### Requirement: JMS producer forwards only allowlisted non-reserved headers

The Rust JMS producer SHALL forward an exchange header only when the endpoint
`jmsHeaderAllowlist` (alias `jms_header_allowlist`) contains its normalized
name. Normalization SHALL casefold and remove `-`, `_`, and `.`. The allowlist
SHALL default to empty. Before the allowlist check the producer SHALL deny, in
normalized form, a reserved JMS name, the `jms` provider-control namespace
(`JMS`, `JMSX`, and `JMS_`), a framework-internal name in the `camel` or
`xcamel` namespace, and any name that contains a credential substring. The
credential substring set SHALL reuse the existing
`BROKER_SENSITIVE_KEY_SUBSTRINGS` constant and SHALL add `apikey`,
`authorization`, `bearer`, and `principal`. Denial SHALL match
case-insensitively and SHALL catch separator variants such as `api-key`,
`api_key`, and `API_KEY`. Producer QoS headers SHALL remain set after header
extraction, so the allowlist does not regress them. `Content-Type` SHALL remain
a dedicated protocol field and inbound mapping SHALL stay unchanged. No denied
header SHALL appear in the IPC send payload or as a JMS property. The Java
bridge SHALL apply the same normalized rules in a `HeaderPolicy` helper,
verified by a parity test.

#### Scenario: Empty allowlist forwards no user headers

- **GIVEN** no `jmsHeaderAllowlist` is configured
- **WHEN** a route sends an exchange with user headers
- **THEN** the IPC send payload SHALL carry no user header
- **AND** the bridge SHALL set no JMS property for them

#### Scenario: A listed business header is forwarded

- **GIVEN** `jmsHeaderAllowlist=customerId,region`
- **WHEN** a route sends `customerId=42` and `region=eu`
- **THEN** both headers SHALL reach the bridge and become JMS properties

#### Scenario: Reserved and framework-internal names are denied even when listed

- **GIVEN** `jmsHeaderAllowlist=JMSMessageID,JMSXDeliveryCount,CamelStop,x-camel-leader-epoch`
- **WHEN** a route sends headers with those names
- **THEN** the IPC payload SHALL omit all of them
- **AND** the bridge SHALL set no JMS property for them

#### Scenario: Credential variants are denied

- **GIVEN** `jmsHeaderAllowlist=api-key,API_KEY,api_key,Password,Authorization`
- **WHEN** a route sends headers with those names
- **THEN** the IPC payload SHALL omit all of them
- **AND** the credential set SHALL reuse `BROKER_SENSITIVE_KEY_SUBSTRINGS`

#### Scenario: Credentials never reach the wire

- **GIVEN** a route that sets `password`, `Authorization`, `api-key`, and a benign `traceId`, with a non-empty allowlist that names them
- **WHEN** the exchange is sent
- **THEN** a positive test SHALL find `traceId` on the wire
- **AND** a negative test SHALL find no credential name or value in the IPC payload or in the broker JMS properties

#### Scenario: Java applies the same rules

- **GIVEN** a shared corpus of header names
- **WHEN** the Rust producer and the Java `HeaderPolicy` classify them
- **THEN** both SHALL deny the same names and allow the same names

### Requirement: JMS bridge applies ActiveMQ jms.* URI options in native images

The native JMS bridge SHALL apply ActiveMQ Classic broker-URL options in the
`jms.*` namespace, including `jms.connectionIDPrefix`. The native-image
configuration SHALL register `org.apache.activemq.ActiveMQConnectionFactory` at
class level with invocation-capable public methods (`allPublicMethods` or an
explicit invocable setter set), not a query-only registration.

`JmsClientFactory` SHALL honor an explicit `jms.connectionIDPrefix` as the
effective connection-ID prefix base and SHALL append a per-process token so
connection IDs stay unique across processes. When no explicit prefix is set it
SHALL apply a random per-process token alone. The native proof SHALL observe the
effective connection identity from a live broker connection, not echoed input
configuration. Existing cross-process unique-ID regressions SHALL remain. No
package-level entry and no transport option outside `jms.*` SHALL be added.

#### Scenario: Invocation-capable class-level registration

- **GIVEN** the native-image `reflect-config.json`
- **WHEN** a reviewer inspects the ActiveMQ entry
- **THEN** `org.apache.activemq.ActiveMQConnectionFactory` SHALL be registered at class level
- **AND** its public methods SHALL be invocation-capable, not query-only
- **AND** no package-level ActiveMQ entry SHALL be added

#### Scenario: Explicit prefix survives with uniqueness

- **GIVEN** a broker URL carrying `jms.connectionIDPrefix=<sentinel>`
- **WHEN** `JmsClientFactory` builds the factory
- **THEN** the effective connection-ID prefix SHALL contain `<sentinel>`
- **AND** it SHALL carry a per-process unique token

#### Scenario: Live-broker connection observes the sentinel

- **GIVEN** a live ActiveMQ Classic broker and the 0.8.0 native bridge
- **WHEN** the proof runs two bridge processes
- **THEN** the health response SHALL report a connected broker
- **AND** each effective broker connection ID SHALL contain the sentinel
- **AND** the two connection IDs SHALL differ
- **AND** no option-application or reflection failure SHALL appear

#### Scenario: The native build still succeeds

- **GIVEN** the new reflection entry
- **WHEN** the native image builds
- **THEN** the build SHALL succeed and `cargo xtask lint-ignore` SHALL pass


## MODIFIED Requirements

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
and observe the recorded reservation count. The killed probe's bridge session
SHALL close — under the watchdog supervision topology the bridge process exits
when the probe dies — and the message SHALL remain unacknowledged across that
death: the kernel SHALL release the destination lock at bridge process exit.

The crash probe SHALL publish the bridge process id it owns
(`BridgeProcess::child_pid`) in a handoff artifact before it writes the
handoff marker, and the test SHALL identify the exact owner bridge by that
published pid — validated live, executable-canonical, supervisor-group
cross-checked, and equal to the destination lock owner — never by scanning the
probe's process group. The probe's death SHALL take the bridge down through
the liveness-pipe watchdog: the test SHALL prove the exact bridge pid exited,
the supervisor process group is empty, and the destination lock was released;
the proof SHALL be capable of FAILING if the watchdog fails to kill the
bridge. Parent-death binding between the probe process and the bridge it owns
is the watchdog mechanism; bridge-side release-on-stream-end semantics for a
bridge whose guard is absent are unchanged.

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

#### Scenario: The probe publishes an authoritative bridge pid

- **GIVEN** a probe that has accepted an exchange and written its handoff marker
- **WHEN** the test reads the handoff artifact the probe published
- **THEN** the published pid SHALL be alive, SHALL run the expected bridge
  binary, SHALL belong to the supervisor process group and not the probe's,
  and SHALL equal the destination lock owner
- **AND** a missing or stale artifact (publisher pid different from the
  spawned probe) SHALL fail the test with a diagnostic naming the probe group
  and the supervisor group

#### Scenario: Probe death takes the bridge down with it

- **GIVEN** a crash test that sent `SIGKILL` to the probe only
- **WHEN** the watchdog observes the liveness-pipe EOF
- **THEN** the exact bridge pid SHALL exit and the supervisor process group
  SHALL become empty
- **AND** the destination lock SHALL be released without any stream-end
  notification being required
- **AND** if the bridge survives the probe, the test SHALL FAIL

#### Scenario: The harness reaps the orphan bridge

- **GIVEN** a crash test that killed the probe
- **WHEN** the test finishes its assertions
- **THEN** the watchdog SHALL have reaped the probe's bridge with it
- **AND** it SHALL leave no process holding the destination lock

#### Scenario: Classic and Artemis coverage

- **GIVEN** the crash scenario
- **WHEN** the suite runs it on ActiveMQ Classic and on Artemis
- **THEN** both runs SHALL record the observed redelivery disposition
- **AND** a gap in broker semantics SHALL be reported, not passed

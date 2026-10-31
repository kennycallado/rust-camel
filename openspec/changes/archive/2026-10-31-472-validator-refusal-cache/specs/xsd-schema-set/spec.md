## ADDED Requirements

### Requirement: Deterministic bridge refusals are memoized per generation and bounded

The bridge backend SHALL memoize a deterministic schema refusal — a
registration response error of kind `COMPILATION_FAILED` mapped to
`ValidatorError::CompilationFailed` — in a negative cache keyed by the
stable schema-set identity (cache key) and the bridge generation, and
SHALL return the memoized typed error for subsequent registrations of the
identical set without hashing, encoding, transmitting or recompiling it.
The backend SHALL NOT memoize transient failures: transport and I/O
errors, timeouts, channel or generation loss, and response errors of any
other kind SHALL be retried on the next registration attempt. The
negative cache SHALL be cleared when the bridge restarts or its
generation changes, and a refusal response belonging to a retired
generation SHALL NOT enter it. The negative cache SHALL share the
`schema_cache_max_entries` budget with the success cache under a
documented combined eviction policy, and success-cache insertion and
reseed semantics SHALL remain unchanged.

#### Scenario: First refusal costs one RPC, repeats cost none

- **GIVEN** a schema set that the bridge deterministically refuses with
  kind `COMPILATION_FAILED`
- **WHEN** the set is registered once and then registered N more times
  against the same bridge generation
- **THEN** the first registration performs exactly one register RPC and
  every subsequent registration performs zero register RPCs while
  returning the same typed `CompilationFailed` error

#### Scenario: Transient transport failure is not memoized

- **GIVEN** a register RPC that fails with a transport error
- **WHEN** the registration is attempted again after the failure
- **THEN** the backend performs the register RPC again instead of
  returning a memoized error

#### Scenario: Non-refusal response error is not memoized

- **GIVEN** a register RPC that returns a response error of a kind other
  than `COMPILATION_FAILED` (for example `INTERNAL`)
- **WHEN** the same set is registered again after the failed attempt
- **THEN** the backend performs the register RPC again instead of
  returning a memoized error

#### Scenario: Restart clears the negative cache

- **GIVEN** a deterministic refusal memoized under generation G
- **WHEN** the bridge restarts and a new generation G+1 is published
- **THEN** the next registration of the same set performs the register
  RPC again against the new generation

#### Scenario: Stale-generation refusal is not memoized

- **GIVEN** a register RPC whose refusal response arrives after the
  bridge generation has advanced past the generation the request was
  sent on
- **WHEN** the same set is registered against the current generation
- **THEN** the backend performs the register RPC again because the stale
  refusal was not memoized

#### Scenario: Combined cache respects the configured bound

- **GIVEN** `schema_cache_max_entries` is set to N and the combined
  success and refusal cache holds N entries
- **WHEN** a new deterministic refusal is memoized
- **THEN** the refusal cache is cleared under the documented combined
  full-clear policy before the new entry is inserted, so the combined
  size stays within the bound and previously memoized refusals are
  re-attempted on their next registration

#### Scenario: Different schema identities do not collide

- **GIVEN** a deterministic refusal memoized for one schema-set identity
- **WHEN** a different schema set (different cache key) is registered
- **THEN** the new set is registered over the wire normally and the
  memoized refusal of the first set does not affect it

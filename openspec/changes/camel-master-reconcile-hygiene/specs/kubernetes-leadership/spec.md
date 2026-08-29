## ADDED Requirements

### Requirement: Lease timing validation enforces renewal slack

`KubernetesPlatformConfig::validate` SHALL reject configurations where
`lease_duration - renew_deadline < retry_period`: the gap between lease
expiry and the renewal deadline must leave at least one full retry window
of slack for clock skew and renew jitter.

#### Scenario: defaults pass

- **GIVEN** the default lease timings (`lease_duration` 15s,
  `renew_deadline` 10s, `retry_period` 2s)
- **WHEN** `validate` runs
- **THEN** validation succeeds (slack 5s ≥ 2s)

#### Scenario: insufficient slack is rejected

- **GIVEN** `lease_duration` 12s, `renew_deadline` 11s, `retry_period` 2s
  (slack 1s)
- **WHEN** `validate` runs
- **THEN** validation fails with a message naming `lease_duration`,
  `renew_deadline`, and `retry_period`

#### Scenario: slack equal to retry period passes

- **GIVEN** `lease_duration` 12s, `renew_deadline` 10s, `retry_period` 2s
  (slack exactly one retry window)
- **WHEN** `validate` runs
- **THEN** validation succeeds

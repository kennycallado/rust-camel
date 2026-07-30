## ADDED Requirements

### Requirement: Review-finding resolution gating

A review finding SHALL block advancement to the next task, phase, or workflow gate unless it identifies no real defect. A finding identifies a real defect when it demonstrates a violation of blessed specs, task acceptance criteria, or applicable project quality requirements. Findings classified critical, important, or legitimate-minor identify a real defect and SHALL be resolved and re-reviewed before advancement. Only an absurd-minor finding, which demonstrates none of those violations, MAY be discarded. Filing a deferred follow-up issue SHALL NOT satisfy this requirement for any in-scope finding that identifies a real defect.

#### Scenario: Legitimate finding blocks advancement

- **GIVEN** a per-task, inter-phase, or holistic review produced a finding that identifies a real defect
- **WHEN** the workflow considers advancing to the next task, phase, or workflow gate
- **THEN** the workflow SHALL NOT advance
- **AND** the finding SHALL be resolved and re-reviewed before advancement

#### Scenario: Absurd finding may be discarded

- **GIVEN** a review produced an absurd-minor finding that demonstrates no violation of blessed specs, task acceptance criteria, or applicable project quality requirements
- **WHEN** the workflow triages the finding
- **THEN** the finding MAY be discarded and SHALL NOT, by itself, block advancement

#### Scenario: Deferral does not satisfy resolution

- **GIVEN** an in-scope finding that identifies a real defect exists
- **WHEN** the workflow considers filing a deferred follow-up issue in place of resolving it
- **THEN** filing the follow-up SHALL NOT permit advancement
- **AND** an out-of-scope finding MAY be tracked in a deferred follow-up, but an in-scope finding remains blocking even when its fix exceeds the current task's scope

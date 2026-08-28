# kubernetes-leadership

## ADDED Requirements

### Requirement: Renewal-path epoch monotonicity

While leading, the leadership service SHALL keep the stored fencing epoch
monotonic: a renewal observing a leader-term lower than the stored epoch
SHALL NOT regress the stored value, and the ignored regression SHALL be
logged.

#### Scenario: regressed term is ignored

- **GIVEN** a leader with stored epoch 7 whose Lease was deleted and
  recreated at `camel.io/leader-term` 1 by an operator
- **WHEN** the next renewal succeeds and observes term 1
- **THEN** the stored epoch remains 7 and a warning is logged containing
  both values

#### Scenario: increased term is adopted

- **GIVEN** a leader with stored epoch 7
- **WHEN** a renewal observes term 9
- **THEN** the stored epoch becomes 9

#### Scenario: equal term is a no-op

- **GIVEN** a leader with stored epoch 7
- **WHEN** a renewal observes term 7
- **THEN** the stored epoch remains 7 and no epoch-update or
  epoch-regression log is emitted

#### Scenario: stripped annotation keeps local epoch

- **GIVEN** a leader with stored epoch 7
- **WHEN** a renewal succeeds but the server response carries no
  leader-term annotation
- **THEN** the stored epoch remains 7

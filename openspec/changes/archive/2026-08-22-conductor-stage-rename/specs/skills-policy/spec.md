# skills-policy Delta

## MODIFIED Requirements

### Requirement: ste-writing permitted during OpenSpec commands

The skills policy at `.opencode/instructions/openspec-skills.md` SHALL permit
`ste-writing` to run as a prose pass before a blessing during `/opsx:*`
commands. This exception SHALL NOT relax the existing rule that only
`self-grill-proposals` may load among the other skills. `ste-writing` SHALL
never replace or weaken a blessing gate.

#### Scenario: prose pass before spec blessing

- **GIVEN** a writer has drafted proposal, design, and specs artifacts during
  STAGE 1
- **WHEN** the writer prepares for the spec blessing
- **THEN** `ste-writing` MAY run as a prose pass on those artifacts, after
  which the blessing hash SHALL be computed over the resulting files

#### Scenario: policy amendment is explicit and minimal

- **GIVEN** the amended `openspec-skills.md`
- **WHEN** a reader inspects the OpenSpec-command skill exceptions
- **THEN** the file SHALL list `ste-writing` by name as the only additional
  permitted skill during `/opsx:*`, and SHALL keep the blessing hashes and
  gates unchanged

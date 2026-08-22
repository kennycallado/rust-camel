# skills-policy Specification

## Purpose
TBD - created by archiving change ste-writing-skill. Update Purpose after archive.
## Requirements
### Requirement: ste-writing skill with two modes

The system SHALL provide a project-local skill `ste-writing` at
`.opencode/skills/ste-writing/SKILL.md` that applies ASD-STE100 Simplified
Technical English as a writing aid to hand-authored durable prose. The skill
SHALL expose two modes: STE-flavored (default, overridable heuristics) and
strict (mandatory). The skill file SHALL enumerate the six mechanical rules
(split sentences over 20 words; replace semicolons with periods; expand
contractions; make passive voice active when the actor is known; replace
`-ing` main verbs, nominalizations, and phrasal verbs with plain verbs; one
name per concept) and the slop-marker set (banned verbs such as `leverage`,
`utilize`, `facilitate`, `ensure`, `prior to`; marketing adjectives such as
`seamless`, `robust`, `powerful`; modal hedges such as "it is important to
note"; em-dashes). In strict mode the rules are mandatory for prose. Code
spans, fenced blocks, identifiers, and verbatim commands are never rewritten
in either mode, and the lint rules SHALL NOT fire on them.

#### Scenario: flavored mode applies to argumentative prose

- **GIVEN** a writer is producing an ADR, a `CONTEXT.md`, a `CONTEXT-MAP.md`,
  a `README.md`, an OpenSpec proposal/design/spec/tasks file, a PR
  description, or a release note
- **WHEN** the `ste-writing` skill activates
- **THEN** it SHALL apply STE-flavored mode and treat the six self-lint rules
  as editorial heuristics that the writer may override when precision or
  argument requires

#### Scenario: strict mode applies to procedure-class prose

- **GIVEN** a writer is producing an operator runbook, a migration or
  remediation procedure, a safety/security instruction, or actionable
  error-message guidance
- **WHEN** the `ste-writing` skill activates
- **THEN** it SHALL apply strict mode and treat the self-lint rules as
  mandatory for prose; the lint rules SHALL NOT fire on code spans, fenced
  blocks, identifiers, or verbatim commands (which are never rewritten in
  either mode)

#### Scenario: code, identifiers, and syntax are never rewritten

- **GIVEN** a passage contains code, identifiers, or command syntax
- **WHEN** the skill processes the passage
- **THEN** it SHALL leave code spans, fenced blocks, identifiers, and command
  syntax unchanged

#### Scenario: the skill enumerates its rule set

- **GIVEN** the `ste-writing` skill file
- **WHEN** a worker reads it to apply the skill
- **THEN** the file SHALL list all six mechanical rules and the slop-marker
  set by name, so the worker does not invent skill behavior

### Requirement: document-function surface division

The system SHALL divide writing skills by document function. `caveman` SHALL
own conversational chat output. `caveman-commit` SHALL own git commit metadata.
`ste-writing` SHALL own durable explanatory, procedural, and operator-facing
prose. No two of these skills SHALL run over the same text at the same time.

#### Scenario: commit messages stay under caveman-commit

- **GIVEN** a commit message is being written
- **WHEN** a skill selection is made
- **THEN** `caveman-commit` SHALL govern the message and `ste-writing` SHALL
  NOT also run over it

#### Scenario: conversational chat stays under caveman

- **GIVEN** an interactive chat reply is being produced
- **WHEN** a skill selection is made
- **THEN** `caveman` SHALL govern the reply and `ste-writing` SHALL NOT also
  run over it

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

### Requirement: architectural voice preservation

In STE-flavored mode the skill SHALL preserve canonical terms, causal argument,
deliberate emphasis, and project-defining formulations. The skill SHALL be a
clarity pass, not a voice-removal pass.

#### Scenario: canonical formulation is preserved

- **GIVEN** a document contains the canonical formulation "Every processor and
  producer is a `Service<Exchange>`"
- **WHEN** `ste-writing` processes the document in flavored mode
- **THEN** the formulation SHALL remain intact, including the `Service<Exchange>`
  term and its argumentative force

#### Scenario: ADR reasoning is not fragmented

- **GIVEN** an ADR justifies a decision against named alternatives using causal
  sentences that exceed 20 words for precision
- **WHEN** `ste-writing` processes the ADR in flavored mode
- **THEN** it SHALL NOT force-split those sentences if doing so would weaken
  the argument, and the writer MAY keep the long form

### Requirement: no runtime or crate changes

The change SHALL NOT modify any Rust crate, `Cargo.toml`, `src/`, `xtask/`, the
data/control plane boundary, or any blessing gate.

#### Scenario: change is confined to tooling and docs

- **GIVEN** the change is implemented
- **WHEN** the diff is inspected
- **THEN** it SHALL touch only `.opencode/skills/ste-writing/`,
  `.opencode/instructions/openspec-skills.md`, and documentation; no `*.rs`
  file SHALL be modified


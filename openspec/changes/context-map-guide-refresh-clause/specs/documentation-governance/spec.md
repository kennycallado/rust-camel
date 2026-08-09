## ADDED Requirements

### Requirement: User-visible-contract guide refresh trigger

The project SHALL treat a user-visible contract change as a documentation-refresh trigger: when an architecture-shaping merge changes a user-visible contract, the change SHALL also refresh the affected mdBook guide section and its anchored `examples/` include, in the same change.

A user-visible contract change is one of: a new EIP builder method, a new component scheme, a DSL key rename, a lifecycle-state rename, or a public contract enum gaining a variant.

This rule extends the existing event-driven refresh contract (CONTEXT-MAP.md, "Refresh is event-driven", bullet 1). It does not apply to internal refactors that leave all user-visible contracts unchanged.

#### Scenario: new EIP builder method added

- **GIVEN** a merge adds a new EIP builder method to `RouteBuilder` (for example `.resequence(...)`)
- **WHEN** the change is prepared for merge
- **THEN** the mdBook guide section documenting EIP patterns and the `examples/` file whose anchor demonstrates the new method are both updated in the same change

#### Scenario: public contract enum gains a variant

- **GIVEN** a merge adds a variant to a public contract enum that is `#[non_exhaustive]` (per ADR-0049), such as `RuntimeCommand`
- **WHEN** the change is prepared for merge
- **THEN** the mdBook guide section that documents matching on that enum is updated in the same change, and the anchored `examples/` include that demonstrates the match is updated to match

#### Scenario: internal refactor with no user-visible change

- **GIVEN** a merge reorganizes internal module boundaries inside `camel-core` (per ADR-0045) without adding, renaming, or removing any user-visible contract
- **WHEN** the change is prepared for merge
- **THEN** the mdBook guide is NOT required to change (only CONTEXT-MAP Contexts/Relationships and touched CONTEXT.md files refresh, per the existing bullet)

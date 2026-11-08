# changelog-066

- Scope: rewrite `## Unreleased` in `CHANGELOG.md` so every user-visible change since `v0.65.0` has one entry under the existing headings (`### Added`, `### Changed`, `### Fixes`, `### Performance`); `CHANGELOG.md` is the only touched file — no version heading, no date, no other docs.
- Exclusion rule: internal-only work (CI/release plumbing, lint tooling, test-only commits, `chore(openspec)`, behaviour-preserving refactors, advisory-ignore bookkeeping already implied by the surrealdb 4.0 entry) gets no bullet and is accounted commit-by-commit in the park coverage table.

---
name: ste-writing
description: Rewrite hand-authored durable prose (ADRs, CONTEXT.md, CONTEXT-MAP.md, README.md, OpenSpec artifacts, PR descriptions, release notes) toward ASD-STE100 Simplified Technical English to remove AI slop. Does NOT apply to code, identifiers, command syntax, docs/ARCHITECT.md (code-derived), commit messages (owned by caveman-commit), or chat (owned by caveman).
---

# Two modes

**STE-flavored (default).** Overridable editorial heuristics. Applied to ADRs, CONTEXT, CONTEXT-MAP, README, OpenSpec artifacts, PR descriptions, release notes.

**Strict.** Mandatory. Apply strict mode only to procedure-class prose: operator runbooks, migration/remediation steps, safety/security instructions, actionable error-message guidance.

# Self-lint rules

Six mechanical rules:

(a) Split sentences over 20 words.
(b) Replace semicolons with periods.
(c) Expand contractions.
(d) Make passive voice active when the actor is known.
(e) Replace -ing main verbs, nominalizations ("perform an analysis" -> "analyze"), and phrasal verbs ("spin up" -> "start") with plain verbs.
(f) one name per concept.

In flavored mode these are overridable. In strict mode they are mandatory for prose.

# Slop markers

Flag these as AI slop:

- Banned verbs: "leverage", "utilize", "facilitate", "ensure", "prior to".
- Marketing adjectives: "seamless", "robust", "powerful".
- modal hedge: "it is important to note".
- em-dash (an LLM tell).

# Code protection

Code spans, fenced blocks, identifiers, and verbatim commands are never rewritten in either mode. Lint rules do not fire on them.

# Voice preservation

In flavored mode the skill preserves canonical terms, causal argument, deliberate emphasis, and project-defining formulations (e.g. "Every processor and producer is a `Service<Exchange>`"). STE clarifies prose. It does not strip voice.

# Surface division

`caveman` owns chat. `caveman-commit` owns commit metadata. `ste-writing` owns durable explanatory, procedural, and operator-facing prose. No two run over the same text at once.

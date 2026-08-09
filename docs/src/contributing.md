# Documentation workflow

Build the book locally, keep code examples honest with the include system,
follow the page template, apply the voice rules, and run the linters.

## Build the book locally

The guide is an mdBook. Build it from the repository root:

```console
nix shell nixpkgs#mdbook -c mdbook build docs
```

For a live preview while you edit, run the watcher and server:

```console
nix shell nixpkgs#mdbook -c mdbook serve docs --open
```

The rendered HTML lands in `docs/book`. That directory is disposable. The build
output is not committed.

## The include system

Code examples come from compiled example crates, not from hand-written
snippets. Each snippet is an mdBook include directive that pulls an anchored
region out of a file under `examples/`. The directive names the file (by path
relative to the page) and the anchor name.

Anchor a region in Rust with comments:

```rust,ignore
// ANCHOR: first-route
let route = RouteBuilder::from("timer:tick?period=1000")
    .to("log:info?level=info&showBody=true")
    .build();
// ANCHOR_END: first-route
```

In YAML, use `# ANCHOR` and `# ANCHOR_END`.

For a working directive, open
[`concepts/routes-pipelines.md`](concepts/routes-pipelines.md) and copy its
include line. It pulls the `first-route` anchor out of
`examples/hello-world/src/main.rs`. That one anchor backs several concept and
pattern pages. One source, many readers.

This is a drift contract. The include pulls real code from a compiled example.
When a Rust API changes, the example stops compiling. The guide cannot drift
from the code while the include resolves. To verify the contract, build the
book and check the example crate:

```console
nix shell nixpkgs#mdbook -c mdbook build docs
cargo check -p hello-world -p content-based-router
```

Never hand-write code that duplicates a compiled example. If no example exists
yet, write minimal inline code and mark the fence `rust,no_run` or `ignore`.

## Page template

Every Enterprise Integration Pattern page follows the same structure. The full
template lives in [`docs/AGENTS.md`](https://github.com/kennycallado/rust-camel/blob/main/docs/AGENTS.md). In short:

1. A `# <pattern name>` heading.
2. One sentence naming the pattern and its Hohpe and Woolf category.
3. An `{{#include}}` directive that pulls the route code from a compiled
   example.
4. Two to four paragraphs of prose.
5. A reference link to the governing crate `CONTEXT.md`.
6. An ADR citation where the page states architectural rationale.
7. A link to the example source on GitHub.

Section hub pages (`index.md`) are navigation aids. They hold one purpose
sentence and a list of child pages with one-line descriptions. No code, no deep
explanation.

## Voice and style

Write like a senior engineer talking to a peer. Short sentences, active voice,
concrete examples over abstractions. The full rules, including banned words and
the em-dash policy, are in [`VOICE.md`](https://github.com/kennycallado/rust-camel/blob/main/docs/VOICE.md). Read it before you write
prose.

## Linters

Two structural linters run over the guide. Run them before you commit:

- `cargo run -p xtask -- lint-adr-cite --deny docs/src/` verifies every ADR
  citation resolves to a file under `docs/adr/`.
- `cargo run -p xtask -- lint-glossary` verifies the glossary stays consistent
  with the Key Terms in [`CONTEXT-MAP.md`](https://github.com/kennycallado/rust-camel/blob/main/CONTEXT-MAP.md).

These are link and structure checks. They do not assess prose quality. Prose
quality depends on the `ste-writing` skill and human review.

## Two-source rule

Every durable claim in the guide cites a source it can defend. The two
acceptable sources are a crate `CONTEXT.md` (the crate authority) or an ADR in
`docs/adr/` (the decision record). A claim with no citation is speculation.

Define each domain term once on its canonical page. Link to it from everywhere
else. Do not re-explain. If two pages both explain the Exchange model, one of
them is wrong.

## Publishing

A GitHub Actions workflow publishes `docs/book` from `main`. An administrator
sets the Pages source to GitHub Actions once under Settings, then Pages.
Generated HTML is not committed. The output directory is disposable, so future
release-versioned books can stage without changing the source chapter URLs.

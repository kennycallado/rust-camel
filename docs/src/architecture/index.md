# Architecture

rust-camel separates a Tower-native data plane from a control plane that owns
route lifecycle and integrations. The repository's
[`CONTEXT-MAP.md`](https://github.com/kennycallado/rust-camel/blob/main/CONTEXT-MAP.md)
defines the bounded contexts and domain language; architecture decision
records under `docs/adr/` explain consequential choices.

The guide is organized by user tasks rather than workspace crates. This keeps
the narrative stable as internal crate boundaries evolve and leaves public
type-level contracts to Rustdoc.

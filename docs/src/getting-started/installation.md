# Installation

Install rust-camel to build integration routes in Rust. Two paths: embed the
crates in a Rust application, or run YAML routes with the CLI.

## Prerequisites

Install the Rust toolchain with [rustup](https://rustup.rs). rust-camel
requires Rust 1.89 or newer. The workspace pins edition 2024, and the Tokio
async runtime needs a current stable release.

Confirm your toolchain:

```console
rustc --version
```

## Path 1: Embed in a Rust project

Add the core crates to an existing Cargo project:

```console
cargo add camel-core camel-builder camel-api
```

Then add only the components your routes reference. A timer-to-log route needs
two endpoint crates:

```console
cargo add camel-component-timer camel-component-log
```

These commands resolve the latest published release from crates.io. The
resulting `[dependencies]` block looks like:

```toml
[dependencies]
camel-api = "0.29"
camel-core = "0.29"
camel-builder = "0.29"
camel-component-timer = "0.29"
camel-component-log = "0.29"
tokio = { version = "1", features = ["full"] }
tracing-subscriber = "0.3"
```

`camel-core` provides `CamelContext`, the runtime that starts and stops routes.
`camel-builder` provides the fluent `RouteBuilder` API. `camel-api` provides
the `Exchange`, `Message`, and `Value` types. Each component crate is optional.
Add only what a route references.

Write your first route next: [First route in Rust](rust-route.md).

## Path 2: Run YAML routes with the CLI

The `camel` CLI parses YAML route files and starts them without compiling Rust.
Install the binary from crates.io:

```console
cargo install camel-cli
```

This places `camel` on your `PATH`. Confirm the install:

```console
camel --version
```

Scaffold a project and run it:

```console
camel new my-integration
cd my-integration
camel run
```

`camel new` creates a `Camel.toml` config file and a `routes/` directory with a
starter route. `camel run` starts the Camel context from that config.

Write your first YAML route next: [First route in YAML](yaml-route.md).

## Run the examples from source

The repository ships compiled examples. Clone it and run one to verify your
toolchain:

```console
git clone https://github.com/kennycallado/rust-camel.git
cd rust-camel
cargo run -p hello-world
```

The `hello-world` example fires a timer five times and logs each tick. You
should see five log lines, then the process waits for Ctrl+C.

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `rustc 1.xx is unsupported` | Run `rustup update stable`. rust-camel needs 1.89 or newer. |
| `camel: command not found` | `cargo install` puts binaries in `~/.cargo/bin`. Add it to your `PATH`. |
| `error[E0658]` on edition 2024 features | Your rustc is too old. See the first row. |

## Next steps

- [First route in Rust](rust-route.md) or [First route in YAML](yaml-route.md)
- [CLI usage](cli.md) for every CLI command
- [Core concepts](../concepts/index.md) for the Exchange and CamelContext model

**Reference**: [CLI crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md)

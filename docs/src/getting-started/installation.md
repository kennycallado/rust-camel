# Installation

rust-camel requires Rust 1.89 or newer. To work with the repository examples:

```console
git clone https://github.com/kennycallado/rust-camel.git
cd rust-camel
cargo run -p hello-world
```

For declarative routes, install the command-line application:

```console
cargo install camel-cli
camel new my-integration
cd my-integration
camel run
```

When embedding rust-camel, add only the crates for the core, builder, and
components your application uses. The examples' `Cargo.toml` files are the
most reliable dependency templates while the project remains pre-release.

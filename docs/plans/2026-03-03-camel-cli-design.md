# camel-cli Design Document

**Date:** 2026-03-03
**Status:** Approved
**Goal:** Reserve the `camel-cli` package name on crates.io with a minimal functional CLI

## Overview

Create a minimal CLI tool for Apache Camel in Rust that can be published to crates.io to reserve the package name. The CLI will initially support only `--help` and `--version` commands, with extensibility for future features.

## Objectives

1. **Primary:** Reserve the `camel-cli` package name on crates.io
2. **Secondary:** Establish an extensible architecture for future CLI development
3. **Future:** Support execution of Camel routes from YAML/JSON configuration files

## Architecture

### Crate Structure

Single binary crate within the existing workspace:

```
crates/camel-cli/
├── Cargo.toml          # Package configuration
├── README.md           # Basic documentation
└── src/
    └── main.rs         # CLI implementation
```

### Workspace Integration

- Add `"crates/camel-cli"` to `workspace.members` in root `Cargo.toml`
- Do NOT add to `workspace.dependencies` (binary crate, not a library)

### Package Naming Convention

- **Package name:** `camel-cli` (for crates.io)
- **Binary name:** `camel` (for user execution)
- Users install with: `cargo install camel-cli`
- Users run with: `camel --help`

## Implementation

### Cargo.toml

```toml
[package]
name = "camel-cli"
version = "0.1.0"
edition = "2024"
license = "Apache-2.0"
description = "Command-line interface for Apache Camel in Rust"
repository = "https://github.com/kennycallado/rust-camel"
readme = "README.md"
keywords = ["camel", "cli", "integration", "messaging"]
categories = ["command-line-utilities", "development-tools"]

[[bin]]
name = "camel"
path = "src/main.rs"

[dependencies]
clap = { version = "4", features = ["derive"] }
```

### main.rs

```rust
use clap::{ArgMatches, Command};

fn main() {
    let matches = Command::new("camel")
        .version("0.1.0")
        .about("Command-line interface for Apache Camel in Rust")
        .subcommand_required(false)
        .arg_required_else_help(true)
        .get_matches();

    handle_commands(&matches);
}

fn handle_commands(_matches: &ArgMatches) {
    // Future: Add run, generate, manage commands here
}
```

### README.md

```markdown
# camel-cli

Command-line interface for Apache Camel in Rust.

## Installation

```bash
cargo install camel-cli
```

## Usage

```bash
camel --help
camel --version
```

## Status

🚧 **Early Development** - This CLI is currently in active development.

Future features will include:
- Execute Camel routes from YAML/JSON files
- Generate Camel projects and components
- Manage and monitor Camel instances

## License

Apache-2.0
```

## Design Decisions

### Why clap from the start?

- Industry standard for Rust CLIs
- Provides excellent help generation and argument parsing
- Easy to extend with subcommands later
- Derive macro for clean, maintainable code

### Why single crate instead of lib/bin split?

- YAGNI principle - no need for separation yet
- Single crate is simpler to maintain
- Can refactor later if testing or reusability needs arise
- Current goal is just name reservation with minimal functionality

### Why version 0.1.0?

- Indicates early development stage
- Allows breaking changes in future versions
- Follows semantic versioning conventions

## Future Extensibility

The architecture supports easy addition of:

1. **Subcommands:** Using clap's subcommand system
   - `camel run <config.yaml>` - Execute routes from config
   - `camel generate <template>` - Generate projects/components
   - `camel manage` - Manage running instances

2. **Configuration support:** Add `camel-cli-config` crate if needed
3. **DSL support:** Integrate with `camel-dsl` crate for YAML/JSON parsing
4. **Runtime support:** Add `camel-cli-runtime` for execution engine

## Publication Checklist

- [ ] Create `crates/camel-cli/` directory structure
- [ ] Implement minimal CLI with clap
- [ ] Write README.md
- [ ] Add to workspace members
- [ ] Test locally: `cargo build -p camel-cli`
- [ ] Test binary: `./target/debug/camel --help`
- [ ] Publish to crates.io: `cargo publish -p camel-cli`

## Success Criteria

- ✅ Package `camel-cli` published on crates.io
- ✅ Binary `camel` executable with help and version
- ✅ Extensible architecture for future development
- ✅ Clear documentation in README.md

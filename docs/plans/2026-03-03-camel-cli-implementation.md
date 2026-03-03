# camel-cli Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create a minimal CLI tool to reserve the `camel-cli` package name on crates.io with help/version functionality

**Architecture:** Single binary crate using clap 4 for argument parsing. The package name is `camel-cli` but the binary is named `camel` for user convenience.

**Tech Stack:** Rust 2024 edition, clap 4 with derive feature

---

## Task 1: Create crate directory structure

**Files:**
- Create: `crates/camel-cli/`
- Create: `crates/camel-cli/src/`

**Step 1: Create directory structure**

```bash
mkdir -p crates/camel-cli/src
```

**Step 2: Verify directories exist**

Run: `ls -la crates/camel-cli/`
Expected: See `src/` directory

---

## Task 2: Create Cargo.toml

**Files:**
- Create: `crates/camel-cli/Cargo.toml`

**Step 1: Write Cargo.toml**

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

**Step 2: Verify file exists**

Run: `cat crates/camel-cli/Cargo.toml`
Expected: See full Cargo.toml content

---

## Task 3: Create main.rs with clap

**Files:**
- Create: `crates/camel-cli/src/main.rs`

**Step 1: Write main.rs**

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

**Step 2: Verify file exists**

Run: `cat crates/camel-cli/src/main.rs`
Expected: See main.rs content

---

## Task 4: Create README.md

**Files:**
- Create: `crates/camel-cli/README.md`

**Step 1: Write README.md**

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

**Step 2: Verify file exists**

Run: `cat crates/camel-cli/README.md`
Expected: See README content

---

## Task 5: Add camel-cli to workspace

**Files:**
- Modify: `Cargo.toml:3-43`

**Step 1: Update workspace members**

Add `"crates/camel-cli",` to the workspace.members array after line 22 (after `"crates/components/camel-controlbus",`):

```toml
members = [
    "crates/camel-api",
    "crates/camel-core",
    "crates/camel-util",
    "crates/camel-support",
    "crates/camel-processor",
    "crates/camel-health",
    "crates/camel-dsl",
    "crates/camel-builder",
    "crates/camel-component",
    "crates/camel-endpoint",
    "crates/camel-test",
    "crates/components/camel-timer",
    "crates/components/camel-log",
    "crates/components/camel-file",
    "crates/components/camel-direct",
    "crates/components/camel-mock",
    "crates/components/camel-http",
    "crates/components/camel-redis",
    "crates/components/camel-controlbus",
    "crates/camel-cli",
    "examples/hello-world",
    # ... rest of examples
]
```

**Step 2: Verify workspace update**

Run: `cargo metadata --no-deps --format-version 1 | grep -o '"name":"camel-cli"'`
Expected: See `"name":"camel-cli"`

---

## Task 6: Build the CLI

**Files:**
- None (build artifact)

**Step 1: Build in debug mode**

Run: `cargo build -p camel-cli`
Expected: Compilation succeeds without errors

**Step 2: Verify binary exists**

Run: `ls -lh target/debug/camel`
Expected: See binary file with executable permissions

---

## Task 7: Test CLI functionality

**Files:**
- None (testing binary)

**Step 1: Test --version**

Run: `./target/debug/camel --version`
Expected: `camel 0.1.0`

**Step 2: Test --help**

Run: `./target/debug/camel --help`
Expected: See help output with description and options

**Step 3: Test no arguments (should show help)**

Run: `./target/debug/camel`
Expected: See help output (due to `arg_required_else_help(true)`)

---

## Task 8: Build release version

**Files:**
- None (build artifact)

**Step 1: Build in release mode**

Run: `cargo build --release -p camel-cli`
Expected: Compilation succeeds with optimizations

**Step 2: Verify release binary**

Run: `./target/release/camel --version`
Expected: `camel 0.1.0`

---

## Task 9: Commit changes

**Files:**
- Multiple files created

**Step 1: Stage all new files**

```bash
git add crates/camel-cli/
git add Cargo.toml
```

**Step 2: Commit with descriptive message**

```bash
git commit -m "feat: add camel-cli minimal CLI to reserve package name

- Create camel-cli crate with clap for argument parsing
- Package name: camel-cli, binary name: camel
- Supports --help and --version commands
- Ready for future extension with run/generate/manage commands"
```

**Step 3: Verify commit**

Run: `git log -1 --oneline`
Expected: See commit message

---

## Task 10: Publication preparation checklist

**Files:**
- None (pre-publication checks)

**Step 1: Verify package metadata**

Run: `cargo metadata --no-deps --format-version 1 -p camel-cli`
Expected: See complete metadata with license, description, etc.

**Step 2: Dry-run package creation**

Run: `cargo publish -p camel-cli --dry-run`
Expected: See "Verify" step succeed without errors

**Step 3: Check README rendering**

Run: `cargo readme -p camel-cli` (if cargo-readme installed)
Expected: README renders correctly

---

## Publication (Manual Step)

**After completing all tasks above:**

```bash
cargo publish -p camel-cli
```

This will publish `camel-cli` to crates.io, reserving the package name.

---

## Success Criteria

- ✅ `crates/camel-cli/` exists with proper structure
- ✅ `cargo build -p camel-cli` succeeds
- ✅ `./target/debug/camel --version` outputs `camel 0.1.0`
- ✅ `./target/debug/camel --help` shows help text
- ✅ Changes committed to git
- ✅ Ready for `cargo publish -p camel-cli`

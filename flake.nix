{
  description = "rust-camel";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
    nixpkgs-unstable.url = "github:nixos/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      nixpkgs-unstable,
      crane,
      flake-utils,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        pkgsUnstable = import nixpkgs-unstable {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
          ];
          targets = [
            "wasm32-wasip2"
          ];
        };

        # Nightly is for cargo-fuzz instrumented builds of the
        # workspace-excluded fuzz/ crate ONLY. The project itself stays on
        # stable (rustToolchain above); no gate or crate build uses nightly.
        # NOTE: nixpkgs cargo-fuzz is 0.13.1; CI pins 0.13.2 — the
        # 0.13.1→0.13.2 changelog shows no tmin/minimize changes.
        fuzzNightly = pkgs.rust-bin.nightly.latest.minimal;

        # `cargo +nightly` needs a rustup-style proxy; rust-overlay toolchains
        # are real toolchains. The shim dispatches +nightly to fuzzNightly and
        # everything else to the stable toolchain. The PATH prepend inside the
        # nightly branch also routes cargo-fuzz's nested bare `cargo build`
        # and `rustc` (spawned via PATH, no toolchain arg) to nightly.
        # Wired via shellHook PATH prepend: inside mkShell the packages PATH
        # order is input order, and rustToolchain would shadow the shim.
        fuzzCargoShim = pkgs.runCommandLocal "fuzz-cargo-shim" { } ''
          mkdir -p $out/bin
          cat > $out/bin/cargo <<'EOF'
          #!/usr/bin/env bash
          if [ "$1" = "+nightly" ]; then
            shift
            export PATH="${fuzzNightly}/bin:$PATH"
            exec "${fuzzNightly}/bin/cargo" "$@"
          fi
          exec "${rustToolchain}/bin/cargo" "$@"
          EOF
          chmod +x $out/bin/cargo
        '';

        src = craneLib.cleanCargoSource ./.;

        commonArgs = {
          inherit src;
          pname = "rust-camel";
          strictDeps = true;
          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ libxml2 ];
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
          BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include";
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        rust-camel = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
          }
        );
      in
      {
        checks = {
          inherit rust-camel;
          rust-camel-clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            }
          );
          rust-camel-doc = craneLib.cargoDoc (
            commonArgs
            // {
              inherit cargoArtifacts;
            }
          );
          rust-camel-fmt = craneLib.cargoFmt {
            inherit src;
          };
          rust-camel-nextest = craneLib.cargoNextest (
            commonArgs
            // {
              inherit cargoArtifacts;
              partitions = 1;
              partitionType = "count";
            }
          );
        };

        packages = {
          default = rust-camel;
          opencode = pkgsUnstable.opencode;
        };

        apps.default = flake-utils.lib.mkApp {
          drv = rust-camel;
        };

        devShells.default = craneLib.devShell {
          inputsFrom = [ rust-camel ];
          packages = with pkgs; [
            rustToolchain
            cargo-audit
            cargo-watch
            cargo-edit
            bacon
            cargo-llvm-cov
            llvm
            sccache
            patchelf
            nix-ld
            pkg-config
            libxml2
            libclang
            mdbook
            python3
            pkgsUnstable.beads # agents memory
            pkgsUnstable.opencode
            pkgsUnstable.rdkafka
            (writeShellScriptBin "openspec" ''
              exec npx @fission-ai/openspec@1.7.0 "$@"
            '')
            # Fuzz tooling: cargo-fuzz + nightly for instrumented builds of
            # the workspace-excluded fuzz/ crate only. The cargo shim itself
            # is activated by the shellHook PATH prepend below (see
            # fuzzCargoShim for why it cannot live in this list).
            pkgs.cargo-fuzz
            fuzzNightly
            # Mutation tooling: cargo-mutants from the locked nixpkgs
            # (27.1.0) == the version the mutants wrapper pins (schema and
            # exit codes match). Stable toolchain, no cargo shim needed
            # (unlike cargo-fuzz above).
            pkgs.cargo-mutants
          ];
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          LLVM_COV = "${pkgs.llvm}/bin/llvm-cov";
          LLVM_PROFDATA = "${pkgs.llvm}/bin/llvm-profdata";
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
          BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include";
          shellHook = ''
            export PATH="${fuzzCargoShim}/bin:$PATH"
            export RUSTC_WRAPPER=sccache
            if [ -d "/home/shared" ] && [ -w "/home/shared" ]; then
              # Per-checkout lock isolation on the big partition:
              # worktrees use their own $WT/target; the main checkout
              # resolves ./target (symlink to the shared dir).
              # CARGO_TARGET_DIR stays deliberately unset here.
              # Export BEFORE start-server: the server caches its dir.
              export SCCACHE_DIR="/home/shared/sccache"
              export SCCACHE_CACHE_SIZE="40G"
            else
              export CARGO_TARGET_DIR="$HOME/.cache/rust-camel-target"
            fi
            sccache --stop-server 2>/dev/null || true
            sccache --start-server

            # JMS bridge: auto-detect native binary
            BRIDGE_BIN="$PWD/bridges/jms/build/native/jms-bridge"
            if [ -x "$BRIDGE_BIN" ]; then
              export CAMEL_JMS_BRIDGE_BINARY_PATH="$BRIDGE_BIN"
            fi

            # XML bridge: auto-detect native binary
            XML_BRIDGE_BIN="$PWD/bridges/xml/build/native/xml-bridge"
            if [ -x "$XML_BRIDGE_BIN" ]; then
              export CAMEL_XML_BRIDGE_BINARY_PATH="$XML_BRIDGE_BIN"
            fi

            # CXF bridge: auto-detect native binary
            CXF_BRIDGE_BIN="$PWD/bridges/cxf/build/native/cxf-bridge"
            if [ -x "$CXF_BRIDGE_BIN" ]; then
              export CAMEL_CXF_BRIDGE_BINARY_PATH="$CXF_BRIDGE_BIN"
            fi

            echo ""
            echo "  rust-camel dev shell"
            echo ""
            if [ -x "$BRIDGE_BIN" ]; then
              echo "  JMS bridge: ready"
            else
              echo "  JMS bridge: not built"
              echo "    run: cargo xtask build-jms-bridge"
            fi
            if [ -x "$XML_BRIDGE_BIN" ]; then
              echo "  XML bridge: ready"
            else
              echo "  XML bridge: not built"
              echo "    run: cargo xtask build-xml-bridge"
            fi
            if [ -x "$CXF_BRIDGE_BIN" ]; then
              echo "  CXF bridge: ready"
            else
              echo "  CXF bridge: not built"
              echo "    run: cargo xtask build-cxf-bridge"
            fi
            echo ""
          '';
        };
      }
    );
}

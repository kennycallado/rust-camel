{
  description = "rust-camel";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.11";
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

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
          ];
        };

        src = craneLib.cleanCargoSource ./.;

        commonArgs = {
          inherit src;
          pname = "rust-camel";
          strictDeps = true;
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

        packages.default = rust-camel;

        apps.default = flake-utils.lib.mkApp {
          drv = rust-camel;
        };

        devShells.default = craneLib.devShell {
          inputsFrom = [ rust-camel ];
          packages = with pkgs; [
            rustToolchain
            cargo-watch
            cargo-edit
            bacon
            cargo-llvm-cov
            llvm
            sccache
            patchelf
            nix-ld
          ];
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          LLVM_COV = "${pkgs.llvm}/bin/llvm-cov";
          LLVM_PROFDATA = "${pkgs.llvm}/bin/llvm-profdata";
          shellHook = ''
            export RUSTC_WRAPPER=sccache
            sccache --stop-server 2>/dev/null || true
            sccache --start-server
            export CARGO_TARGET_DIR="$HOME/.cache/rust-camel-target"

            # JMS bridge: auto-detect native binary
            BRIDGE_BIN="$PWD/bridges/jms/build/native/jms-bridge"
            if [ -x "$BRIDGE_BIN" ]; then
              export CAMEL_JMS_BRIDGE_BINARY_PATH="$BRIDGE_BIN"
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
            echo ""
          '';
        };
      }
    );
}

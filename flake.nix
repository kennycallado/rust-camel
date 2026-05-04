{
  description = "rust-camel";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.11";
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
        };

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
            pkg-config
            libxml2
            libclang
            pkgsUnstable.rdkafka
          ];
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          LLVM_COV = "${pkgs.llvm}/bin/llvm-cov";
          LLVM_PROFDATA = "${pkgs.llvm}/bin/llvm-profdata";
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
          BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include";
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

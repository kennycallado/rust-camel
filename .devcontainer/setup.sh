#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if [ ! -d "$HOME/.config/nvim" ]; then
  mkdir -p "$HOME/.config"
  cp -r .devcontainer/nvim "$HOME/.config/nvim"
fi

if ! command -v nvim >/dev/null 2>&1; then
  case "$(uname -m)" in
    x86_64) nvim_arch=x86_64 ;;
    aarch64 | arm64) nvim_arch=arm64 ;;
    *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
  esac
  nvim_url="https://github.com/neovim/neovim/releases/download/v0.12.4/nvim-linux-${nvim_arch}.tar.gz"
  curl -fsSL "$nvim_url" -o /tmp/nvim.tar.gz
  sudo tar -xzf /tmp/nvim.tar.gz -C /usr/local --strip-components=1
  rm /tmp/nvim.tar.gz
fi

if ! command -v rg >/dev/null 2>&1; then
  sudo apt-get update
  sudo apt-get install -y ripgrep
fi

if ! command -v opencode >/dev/null 2>&1; then
  nix profile install .#opencode
fi

nix develop --command bash -c 'nvim --headless "+Lazy! sync" "+TSInstallSync rust toml lua vim vimdoc query bash markdown markdown_inline json yaml" +qa' >/dev/null 2>&1 || true

nix develop --command true

# GitHub Codespaces

Create a codespace from the repository as usual. The container installs Nix,
Neovim, OpenCode, and an SSH server, then evaluates the project's pinned
development shell once so its dependencies are cached.

Enter the development shell before building:

```sh
nix develop
```

OpenCode is installed in the user profile and is available directly:

```console
opencode
```

A Neovim config is copied to `~/.config/nvim` on first setup. It is built on
`mini.nvim` with LazyVim-style keybindings (pinned via `lazy-lock.json`,
pre-warmed during setup). `rg` is installed for `mini.pick` grep.

Finders/explorer (`mini.pick` / `mini.files`):
- `<leader><space>` / `<leader>ff` find files, `<leader>fg` live grep
- `<leader>fb` / `<leader>,` buffers, `<leader>:` command history
- `<leader>e` file explorer

Terminals (`toggleterm`) float centered at 80% of the editor: `<C-1>`..`<C-9>`
(or `<leader>t1`..`<leader>t9`) toggle terminals 1-9; `<C-\>` / `<leader>tt`
toggle terminal 1.

GitHub's authenticated SSH path does not require exposing a port:

```sh
gh codespace ssh --codespace <name>
```

The SSH daemon feature is also present for clients that need a conventional
daemon. Prefer `gh codespace ssh`, which uses Codespaces authentication and
does not require managing a public SSH port.

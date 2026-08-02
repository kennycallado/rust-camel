# GitHub Codespaces

Create a codespace from the repository as usual. The container installs Nix,
OpenCode, and an SSH server, then evaluates the project's pinned development
shell once so its dependencies are cached.

Enter the development shell before building:

```console
nix develop
```

OpenCode is installed in the user profile and is available directly:

```console
opencode
```

GitHub's authenticated SSH path does not require exposing a port:

```console
gh codespace ssh --codespace <name>
```

The SSH daemon feature is also present for clients that need a conventional
daemon. Prefer `gh codespace ssh`, which uses Codespaces authentication and
does not require managing a public SSH port.

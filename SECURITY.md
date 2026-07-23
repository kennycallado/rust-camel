# Security Policy

## Supported versions

rust-camel is pre-release software. Security fixes target the
latest `main` only; there are no backports to older lines yet.

| Version | Supported |
| ------- | --------- |
| `main` / latest release | ✅ |
| Older releases | ❌ |

## Reporting a vulnerability

**Please do not open a public GitHub issue for security problems.**

Use [GitHub's private vulnerability reporting](https://github.com/kennycallado/rust-camel/security/advisories/new).
Include:

- A description of the issue and its impact.
- Steps to reproduce (proof of concept, affected component/route).
- Suggested fix, if you have one.

You will receive an acknowledgement within a few days. Please allow
reasonable time for a fix before public disclosure.

## Built-in hardening

rust-camel ships several built-in security safeguards (see the README for
usage details):

- **SSRF protection** in the HTTP component — private IP ranges blocked by
  default; configurable host blocklists.
- **Path traversal protection** in the File component — resolved paths are
  validated against the configured base directory.
- **Timeouts** on file and HTTP I/O, with safe defaults.
- **Memory limits** on the aggregator (`max_buckets`, `bucket_ttl`).
- **Fail-closed command execution** in the `exec` component — profile-pinned
  binaries, argument policy, environment sanitization, and cwd confinement.

## Scope

In scope: vulnerabilities in rust-camel crates and components, the DSL
parser, and the route runtime. Out of scope: vulnerabilities in third-party
dependencies (report those upstream) and issues in example code that do not
affect the framework.

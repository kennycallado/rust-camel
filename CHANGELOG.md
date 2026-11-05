# Changelog

## Unreleased

### Fixes

- fix(languages/rhai): a JSON integer above i64::MAX in a header, property, or body no longer fails every later rhai script step — it binds as a json number carrying the exact decimal token, and fails only later, at the operation, if used in a refused comparison or converted outbound as a non-image token (bd rc-141om).
- fix(components/mail): with one account and no `?account=`, an endpoint no longer logs in to an unlisted host — it needs the URI host in the account's `allowedHosts` (case-insensitive), fails at endpoint creation when the list is empty, the host does not match or the URI has no host, and never carries the host in the error; migrate with `?account=<name>` or `allowedHosts` (bd rc-bcstf).

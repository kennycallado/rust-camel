# Changelog

## Unreleased

### Fixes

- fix(languages/rhai): a JSON integer above i64::MAX in a header, property, or body no longer fails every later rhai script step — it binds as a json number carrying the exact decimal token, and fails only later, at the operation, if used in a refused comparison or converted outbound as a non-image token (bd rc-141om).

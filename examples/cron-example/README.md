# cron-example

Demonstrates the `cron:` component — fires an Exchange every minute using a
Unix 5-field cron expression.

## Run

```bash
cargo run -p cron-example
```

An exchange fires at the top of each minute (second 0). Watch the logs for
`cron-result` output with body `cron-fired` and header `source=cron`.

## URI format

```
cron:<name>?schedule=<5-field-expr>[&timeZone=<IANA>]
```

- `schedule` — Unix 5-field cron: `min hour dom month dow`. Use `+` as space
  separator (e.g. `0+2+*+*+*` for "2am daily"). Percent-encoding (`%20`) also
  works.
- `timeZone` — IANA timezone (default: `UTC`)

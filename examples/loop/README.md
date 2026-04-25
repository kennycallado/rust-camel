# Loop Example

Demonstrates the Loop EIP pattern: iterating a sub-pipeline a fixed number of times.

## What it does

A timer triggers every 3 seconds (3 times total). Each exchange starts with body "hello" and passes through a count-mode loop that appends "!" three times:

```
timer → set_body("hello") → loop(3 × append "!") → log
```

Expected output body: `hello!!!`

## Run

```sh
cargo run -p loop-example
```

## Code

- `src/main.rs` — Programmatic DSL with `loop_count(3)` and a `process` step inside the loop scope.

## YAML equivalent

```yaml
routes:
  - id: "loop-yaml"
    from: "timer:tick?period=3000&repeatCount=3"
    steps:
      - set_body:
          value: "hello"
      - loop:
          count: 3
          steps:
            - process:
                language: "rhai"
                source: "let b = body().as_text().unwrap_or(\"\").to_string(); set_body(b + \"!\")"
      - to: "log:loop-result?showBody=true"
```

## While-mode example (programmatic)

```rust
builder.loop_while(|ex| {
    let n: u64 = ex.input.body.as_text().unwrap_or("0").parse().unwrap_or(0);
    n < 5
})
.process(|mut ex| async move {
    let n: u64 = ex.input.body.as_text().unwrap_or("0").parse().unwrap_or(0);
    ex.input.body = Body::Text(format!("{}", n + 1));
    Ok(ex)
})
.end_loop()
```

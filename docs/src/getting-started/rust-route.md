# First route in Rust

The runnable [`hello-world`](https://github.com/kennycallado/rust-camel/tree/main/examples/hello-world)
example registers timer and log components, builds a route, and starts its
context. The code below is included from that compiled workspace example:

```rust,ignore
{{#include ../../../examples/hello-world/src/main.rs:first-route}}
```

`timer:tick` is the consumer endpoint. Each tick creates an exchange, the
route adds a header, and `log:info` receives the exchange as the producer
endpoint. Run it with `cargo run -p hello-world`.

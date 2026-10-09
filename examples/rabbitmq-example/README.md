# rabbitmq-example

Timer-driven producer and log consumer against RabbitMQ over native AMQP 0-9-1.

- Consumer: `rabbitmq:default?queue=demo&autoDeclare=true` → `log:info?showHeaders=true`.
  `autoDeclare=true` declares the `demo` queue at consumer start.
- Producer: `timer:tick?period=1000` → `set_body` JSON → `rabbitmq:default?queue=demo`.

The default exchange routes by queue name, so the producer's routing key falls back to `demo`.

## Run it

Start a local RabbitMQ fixture on loopback with explicit demo credentials:

```bash
docker run -d --rm --name rmq-example \
  -p 127.0.0.1:5672:5672 \
  -e RABBITMQ_DEFAULT_USER=rmq \
  -e RABBITMQ_DEFAULT_PASS=rmq \
  rabbitmq:3.13-alpine
```

Then run the example:

```bash
cargo run -p rabbitmq-example
```

The example reads `RABBITMQ_URL`, defaulting to `amqp://rmq:rmq@127.0.0.1:5672/%2f`. Set it to point at another broker:

```bash
RABBITMQ_URL=amqp://user:pass@broker.internal:5672/%2f cargo run -p rabbitmq-example
```

The `rmq`/`rmq` credentials belong to the ephemeral local fixture. They are not a production secret.

Press Ctrl+C to stop.

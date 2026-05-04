# cxf-example

A minimal CXF example project for `camel run` with:
- SOAP producer route (`soap-producer.yaml`) that invokes `sayHello` every 10 seconds
- SOAP consumer route (`soap-consumer.yaml`) that exposes a SOAP endpoint and returns a fixed response

## Prerequisites

Build the CLI from workspace root:

```bash
cargo build -p camel-cli
```

## Run

```bash
cd examples/cxf-example
../../target/debug/camel run
```

## Expected output

- Consumer route listens on `http://0.0.0.0:9090/hello`
- Producer route sends SOAP request payloads periodically
- Logs show received SOAP requests and SOAP responses

# RabbitMQ TLS test fixtures

Test-only material. Never use these keys or certificates outside tests.

All fixtures share the same fixed validity window (`2020-01-01` ..
`2120-01-01`), so they stay valid regardless of the machine clock.

- `test-ca.pem` — self-signed CA trust anchor for the local harness and the
  gated `amqps` e2e tests.
- `server-cert.pem` / `server-key.pem` — leaf certificate and private key for
  `localhost` (SAN `DNS:localhost,IP:127.0.0.1`), signed by `test-ca.pem`. The
  local harness runs a `tokio-rustls` acceptor with this pair; the committed
  key is intentionally public test material.
- `wrong-ca.pem` — a second, unrelated self-signed CA used to prove that a
  client configured with the wrong trust anchor rejects the broker.

## Regeneration

Run from this directory. `test-ca.pem` was regenerated in task 1.4 because the
CA private key is not committed; issuing the leaf requires it. The CA subject,
extensions and validity window are unchanged.

```bash
# CA key + trust anchor (key stays temporary)
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout /tmp/rabbitmq-test-ca-key.pem \
  -out test-ca.pem \
  -not_before 20200101000000Z -not_after 21200101000000Z \
  -subj "/CN=rust-camel rabbitmq test CA" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign"

# Leaf key + CSR
openssl req -new -newkey rsa:2048 -nodes \
  -keyout server-key.pem \
  -out /tmp/rabbitmq-test-server.csr \
  -subj "/CN=localhost"

# Leaf extensions (SANs the harness connects to)
cat > /tmp/rabbitmq-test-server.ext <<'EOF'
subjectAltName=DNS:localhost,IP:127.0.0.1
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
EOF

# Sign the leaf with the test CA
openssl x509 -req \
  -in /tmp/rabbitmq-test-server.csr \
  -CA test-ca.pem \
  -CAkey /tmp/rabbitmq-test-ca-key.pem \
  -CAcreateserial \
  -out server-cert.pem \
  -not_before 20200101000000Z -not_after 21200101000000Z \
  -extfile /tmp/rabbitmq-test-server.ext

# Unrelated CA for the reject-untrusted-CA test
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout /tmp/rabbitmq-test-wrong-ca-key.pem \
  -out wrong-ca.pem \
  -not_before 20200101000000Z -not_after 21200101000000Z \
  -subj "/CN=rust-camel rabbitmq unrelated test CA" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign"

rm -f /tmp/rabbitmq-test-ca-key.pem \
      /tmp/rabbitmq-test-wrong-ca-key.pem \
      /tmp/rabbitmq-test-server.csr \
      /tmp/rabbitmq-test-server.ext
rm -f test-ca.srl
```

The `-not_before` / `-not_after` options require OpenSSL 3.4 or newer. The
private CA keys are intentionally not committed: `test-ca.pem` and
`wrong-ca.pem` are trust anchors only.

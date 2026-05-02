FROM rust:alpine AS builder

RUN apk add --no-cache \
    build-base \
    ca-certificates \
    pkgconf \
    openssl-dev \
    openssl-libs-static \
    libxml2-dev

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY scripts scripts
COPY examples examples

RUN cargo build -p camel-cli --release

FROM scratch AS production
WORKDIR /app
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /app/target/release/camel /usr/local/bin/camel
ENTRYPOINT ["camel"]

FROM alpine:3.21 AS alpine
WORKDIR /app
RUN apk add --no-cache ca-certificates
COPY --from=builder /app/target/release/camel /usr/local/bin/camel
ENTRYPOINT ["camel"]

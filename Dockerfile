FROM rust:alpine AS builder

RUN apk add --no-cache \
    gcc-g++ \
    ca-certificates \
    cmake \
    make \
    pkgconf \
    openssl-dev \
    openssl-libs-static \
    curl-dev \
    cyrus-sasl-dev \
    zlib-dev \
    zstd-dev \
    libxml2-dev \
    musl-dev

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY scripts scripts
COPY examples examples

RUN cargo build -p camel-cli --release --features kafka-static

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

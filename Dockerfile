ARG TARGETARCH=amd64

FROM scratch AS production
ARG TARGETARCH
COPY camel-${TARGETARCH} /usr/local/bin/camel
COPY ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
ENTRYPOINT ["camel"]

FROM alpine:3.21 AS alpine
ARG TARGETARCH
RUN apk add --no-cache ca-certificates
COPY camel-${TARGETARCH} /usr/local/bin/camel
ENTRYPOINT ["camel"]

# gnu variant (distroless, glibc binaries).
#
# Allocator note: the glibc default (per-thread arenas, up to 8x cores) shows a
# slow RSS drift under sustained multithreaded churn — measured 107->139 MB
# VmRSS over 10 identical load rounds, no plateau (demo-team soak, bd rc-9cwi).
# Operators running long-lived high-churn workloads can set MALLOC_ARENA_MAX=2
# (or 4 to reduce arena-lock contention): with 2, the same soak converged flat
# at ~72 MB. Deliberately NOT baked in as an image default: one env fits no
# every workload. musl variants (production/alpine) ship jemalloc instead.
# Switching this image to the jemalloc cargo feature is tracked in bd rc-9cwi.
FROM gcr.io/distroless/cc-debian13 AS gnu
ARG TARGETARCH
COPY camel-${TARGETARCH} /usr/local/bin/camel
ENTRYPOINT ["camel"]

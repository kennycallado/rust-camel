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

# Benchmark runner — single Temurin-based image for the entire
# v3 benchmark pipeline (build + measure). See plan
# docs/superpowers/plans/2026-07-20-benchmark-container-runner.md
#
# Base: Eclipse Temurin JDK 21 — the same vendor/channel as the
# validated smoke toolchain (rust-toolchain.toml `stable` for Rust,
# Temurin 21 for Java, Gradle 8.10.x, Maven 3.9.x), mirroring the
# nix-shell environment the validated smokes ran on. The earlier
# Mandrel base was dropped for exactly this reason: the runner must
# mirror the validated toolchain, not a same-VM-family lookalike.
#
# Native-image builds do NOT use an in-image GraalVM: the harness
# builds native artifacts through $QUARKUS_NATIVE_BUILDER_IMAGE
# (BENCH_NATIVE_MODE=docker), exactly as the validated smokes did
# with a plain Temurin host JDK.
#
# The floating base tag is made reproducible one level up: pin.sh
# records the built image by digest (DIGEST), which transitively
# freezes base, toolchains, and utilities.

FROM eclipse-temurin:21-jdk-jammy

# Benchmark utilities first (the gradle install below needs unzip):
# GNU time (peak RSS), jq (manifest parsing), iproute2 (ss for
# port-availability probes), procps (pgrep/pkill for bridge
# subprocess cleanup), findutils, unzip (gradle install below),
# util-linux (setsid for rust-camel-cli wrapper — reparents child
# process per rc-w1u9 handshake).
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        time \
        jq \
        iproute2 \
        procps \
        findutils \
        unzip \
        util-linux \
    && rm -rf /var/lib/apt/lists/*

# Rust toolchain (rustup → `stable`, matching the workspace
# rust-toolchain.toml channel; the image digest freezes the exact
# version at build time).
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal \
    && chmod -R a+rwx /usr/local/rustup /usr/local/cargo

# Maven (camel-standalone builds). 3.9.x, matching the
# maven:3.9-eclipse-temurin-21 image the smokes used for maven builds.
ENV MAVEN_VERSION=3.9.9
ENV MAVEN_HOME=/opt/maven
ENV PATH=$MAVEN_HOME/bin:$PATH
RUN mkdir -p $MAVEN_HOME \
    && curl -fsSL https://archive.apache.org/dist/maven/maven-3/$MAVEN_VERSION/binaries/apache-maven-$MAVEN_VERSION-bin.tar.gz \
       | tar -xzC $MAVEN_HOME --strip-components=1

# Gradle 8.10 (camel-quarkus native builds), matching the 8.10.x
# wrapper the validated smokes ran. The image does NOT ship Gradle;
# each scenario's `gradlew` wrapper could in principle handle it, but
# pre-installing avoids the per-scenario download.
ENV GRADLE_VERSION=8.10.2
ENV GRADLE_HOME=/opt/gradle
ENV PATH=$GRADLE_HOME/bin:$PATH
ENV GRADLE_BIN=/opt/gradle/bin/gradle
RUN mkdir -p $GRADLE_HOME \
    && curl -fsSL https://services.gradle.org/distributions/gradle-$GRADLE_VERSION-bin.zip -o /tmp/gradle.zip \
    && unzip -q /tmp/gradle.zip -d /opt \
    && mv /opt/gradle-$GRADLE_VERSION/* $GRADLE_HOME/ \
    && rmdir /opt/gradle-$GRADLE_VERSION \
    && rm /tmp/gradle.zip

# Node.js 22.x LTS (node contenders, OpenSpec change bench-node).
# ARG defaults mirror the pin.sh literals — pin.sh is the single
# source of truth; the defaults exist so a bare `docker build` still
# works. `.tar.gz` (not .tar.xz) because xz-utils is absent from the
# image. The tarball is SHA256-verified BEFORE extraction — a
# mismatch fails the build (fail closed).
ARG NODE_VERSION=22.14.0
ARG NODE_SHA256=9d942932535988091034dc94cc5f42b6dc8784d6366df3a36c4c9ccb3996f0c2
ENV NODE_HOME=/opt/node
ENV PATH=$NODE_HOME/bin:$PATH
RUN curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.gz" -o /tmp/node.tar.gz \
    && echo "${NODE_SHA256}  /tmp/node.tar.gz" | sha256sum -c - \
    && mkdir -p $NODE_HOME \
    && tar -xzf /tmp/node.tar.gz -C $NODE_HOME --strip-components=1 \
    && rm /tmp/node.tar.gz \
    && ln -s $NODE_HOME/bin/node /usr/local/bin/node \
    && ln -s $NODE_HOME/bin/npm /usr/local/bin/npm

# Cache volume mount points: pre-create with world-writable perms so
# named volumes (maven-cache, gradle-cache) attached at runtime are
# writable by the host UID (container runs as --user $(id -u):$(id -g)).
# 1777 = sticky + rwx, same as /tmp.
RUN mkdir -p /root/.m2 /root/.gradle \
    && chmod -R 1777 /root/.m2 /root/.gradle

# Verify the toolchain is wired correctly (image-build-time gate).
# No native-image here by design — native builds go through
# $QUARKUS_NATIVE_BUILDER_IMAGE (see header).
RUN java -version && cargo --version && mvn -version && gradle -version && node --version && /usr/bin/time --version | head -1

# Default workdir; overridden by -w /work at run time.
WORKDIR /work

# Clear any inherited ENTRYPOINT — we run bash + cargo + gradle + mvn
# as siblings, not via an entrypoint. Without this, `docker run
# benchmark-runner bash benchmarks/harness/run.sh ...` could resolve
# to `<entrypoint> bash ...` if the base ever grows one.
ENTRYPOINT []

# The harness runs as the host UID/GID (passed via --user). Cache
# volumes attached at /root/.m2 + /root/.gradle are world-writable
# (chmod 1777 above) so non-root container user can write deps.

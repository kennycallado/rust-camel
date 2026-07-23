# Benchmark runner — single Mandrel-based image for the entire
# v3 benchmark pipeline (build + measure). See plan
# docs/superpowers/plans/2026-07-20-benchmark-container-runner.md
#
# Base: Quarkus Mandrel builder image (GraalVM JDK 21 + native-image).
# Adds Rust toolchain (for rust-camel-lib + bridge + loadgen) and
# Maven (for camel-standalone jars) and benchmark utilities.
#
# Note: GraalVM CE was tested for PGO support but PGO is EE-only
# in ALL distributions (Mandrel CE and GraalVM CE). Mandrel is
# kept for its newer JDK (21.0.11 vs 21.0.2) and Quarkus tuning.
# The bridges (scripts/xtask) use GraalVM CE — the inconsistency
# is cosmetic (same Substrate VM, same GC options, same feature set).

FROM quay.io/quarkus/ubi-quarkus-mandrel-builder-image:jdk-21

# Switch to root for tool installation (base image defaults to user 1000).
USER root

# Rust toolchain (rustup → stable toolchain).
# Pin to a known-good stable version for reproducibility.
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.82.0 --profile minimal \
    && chmod -R a+rwx /usr/local/rustup /usr/local/cargo

# Maven (camel-standalone builds).
ENV MAVEN_VERSION=3.9.9
ENV MAVEN_HOME=/opt/maven
ENV PATH=$MAVEN_HOME/bin:$PATH
RUN mkdir -p $MAVEN_HOME \
    && curl -fsSL https://archive.apache.org/dist/maven/maven-3/$MAVEN_VERSION/binaries/apache-maven-$MAVEN_VERSION-bin.tar.gz \
       | tar -xzC $MAVEN_HOME --strip-components=1

# Gradle 8.10 (camel-quarkus native builds). The base image does NOT
# ship Gradle; each scenario's `gradlew` wrapper could in principle
# handle it, but pre-installing avoids the per-scenario download.
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

# Benchmark utilities: GNU time (peak RSS), jq (manifest parsing),
# iproute2 (ss for port-availability probes), procps-ng (pgrep/pkill
# for bridge subprocess cleanup), unzip (for gradle install above),
# util-linux (setsid for rust-camel-cli wrapper — reparents child
# process per rc-w1u9 handshake).
RUN microdnf install -y \
        time \
        jq \
        iproute \
        procps-ng \
        findutils \
        unzip \
        util-linux \
    && microdnf clean all

# Cache volume mount points: pre-create with world-writable perms so
# named volumes (maven-cache, gradle-cache) attached at runtime are
# writable by the host UID (container runs as --user $(id -u):$(id -g)).
# 1777 = sticky + rwx, same as /tmp.
RUN mkdir -p /root/.m2 /root/.gradle \
    && chmod -R 1777 /root/.m2 /root/.gradle

# Verify the toolchain is wired correctly (image-build-time gate).
RUN java -version && native-image --version && cargo --version && mvn -version && gradle -version && /usr/bin/time --version | head -1

# Default workdir; overridden by -w /work at run time.
WORKDIR /work

# Clear the base image's ENTRYPOINT [native-image] — we run
# bash + cargo + gradle + mvn + native-image as siblings, not via
# the entrypoint. Without this, `docker run benchmark-runner bash
# benchmarks/harness/run.sh ...` becomes `native-image bash ...`.
ENTRYPOINT []

# The harness runs as the host UID/GID (passed via --user). Cache
# volumes attached at /root/.m2 + /root/.gradle are world-writable
# (chmod 1777 above) so non-root container user can write deps.

FROM eclipse-temurin:21-jdk AS builder
WORKDIR /build
COPY . .
RUN apt-get update && apt-get install -y gradle && rm -rf /var/lib/apt/lists/*
RUN gradle quarkusBuild -Dquarkus.package.type=uber-jar --no-daemon

FROM eclipse-temurin:21-jre
WORKDIR /app
COPY --from=builder /build/build/*-runner.jar /app/jms-bridge.jar
ENV BRIDGE_BROKER_URL=tcp://localhost:61616
ENV BRIDGE_BROKER_TYPE=activemq
CMD ["java", "-jar", "/app/jms-bridge.jar"]

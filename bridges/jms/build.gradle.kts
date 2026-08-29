plugins {
    java
    id("io.quarkus") version "3.39.1"
    id("com.diffplug.spotless") version "6.25.0"
}

version = project.findProperty("version")?.toString() ?: "0.6.1"

repositories {
    mavenCentral()
}

val quarkusVersion = "3.39.1"

dependencies {
    implementation(enforcedPlatform("io.quarkus.platform:quarkus-bom:$quarkusVersion"))
    implementation("io.quarkus:quarkus-grpc")
    implementation("io.quarkus:quarkus-arc")
    implementation("io.quarkus:quarkus-config-yaml")

    // ActiveMQ Classic (OpenWire)
    implementation("org.apache.activemq:activemq-client:5.19.10")
    implementation("org.apache.activemq:activemq-pool:5.19.10")

    // Artemis AMQP/JMS client
    implementation("org.apache.activemq:artemis-jms-client-all:2.36.0")

    // Generic JMS connection pooling (used for Artemis)
    implementation("org.messaginghub:pooled-jms:2.0.8")

    // log4j-api needed at native-image build time (Log4J2LogImpl in artemis-jms-client-all)
    runtimeOnly("org.apache.logging.log4j:log4j-api:2.26.1")

    testImplementation("io.quarkus:quarkus-junit5")
    testImplementation("org.mockito:mockito-core:5.12.0")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

spotless {
    java {
        target("src/**/*.java")
        googleJavaFormat("1.34.1")
    }
}

tasks.wrapper {
    gradleVersion = "8.10"
    distributionType = org.gradle.api.tasks.wrapper.Wrapper.DistributionType.BIN
}

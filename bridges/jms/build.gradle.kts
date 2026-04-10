plugins {
    java
    id("io.quarkus") version "3.20.0"
    id("com.diffplug.spotless") version "6.25.0"
}

version = project.findProperty("version")?.toString() ?: "0.2.0"

repositories {
    mavenCentral()
}

val quarkusVersion = "3.20.0"

dependencies {
    implementation(enforcedPlatform("io.quarkus.platform:quarkus-bom:$quarkusVersion"))
    implementation("io.quarkus:quarkus-grpc")
    implementation("io.quarkus:quarkus-arc")

    // ActiveMQ Classic (OpenWire)
    implementation("org.apache.activemq:activemq-client:5.18.3")
    implementation("org.apache.activemq:activemq-pool:5.18.3")

    // Artemis AMQP/JMS client
    implementation("org.apache.activemq:artemis-jms-client-all:2.36.0")

    // Generic JMS connection pooling (used for Artemis)
    implementation("org.messaginghub:pooled-jms:2.0.8")

    // log4j-api needed at native-image build time (Log4J2LogImpl in artemis-jms-client-all)
    runtimeOnly("org.apache.logging.log4j:log4j-api:2.24.3")

    testImplementation("io.quarkus:quarkus-junit5")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

spotless {
    java {
        googleJavaFormat("1.24.0")
    }
}

tasks.wrapper {
    gradleVersion = "8.10"
    distributionType = org.gradle.api.tasks.wrapper.Wrapper.DistributionType.BIN
}

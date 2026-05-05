plugins {
    java
    id("io.quarkus") version "3.20.0"
    id("com.diffplug.spotless") version "6.25.0"
}

version = project.findProperty("version")?.toString() ?: "0.1.0"

repositories {
    mavenCentral()
    maven {
        url = uri("https://build.shibboleth.net/nexus/content/groups/public")
    }
}

val quarkusVersion = "3.20.0"
val cxfVersion = "4.1.1"

dependencies {
    implementation(enforcedPlatform("io.quarkus.platform:quarkus-bom:$quarkusVersion"))
    implementation("io.quarkus:quarkus-grpc")
    implementation("io.quarkus:quarkus-arc")

    // Apache CXF
    implementation("org.apache.cxf:cxf-core:$cxfVersion")
    implementation("org.apache.cxf:cxf-rt-frontend-simple:$cxfVersion")
    implementation("org.apache.cxf:cxf-rt-frontend-jaxws:$cxfVersion")
    implementation("org.apache.cxf:cxf-rt-transports-http:$cxfVersion")
    implementation("org.apache.cxf:cxf-rt-ws-security:$cxfVersion")
    implementation("org.apache.cxf:cxf-rt-bindings-soap:$cxfVersion")
    implementation("wsdl4j:wsdl4j:1.6.3")
    implementation("org.osgi:org.osgi.core:6.0.0")
    implementation("jakarta.servlet:jakarta.servlet-api:6.1.0")
    implementation("com.ibm.icu:icu4j:75.1")
    implementation("com.sun.xml.fastinfoset:FastInfoset:2.1.1")

    testImplementation("io.quarkus:quarkus-junit5")
    testImplementation("io.grpc:grpc-testing")
    testImplementation("org.mockito:mockito-core:5.12.0")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

spotless {
    java {
        target("src/**/*.java")
        googleJavaFormat("1.24.0")
    }
}

tasks.wrapper {
    gradleVersion = "8.10"
    distributionType = org.gradle.api.tasks.wrapper.Wrapper.DistributionType.BIN
}

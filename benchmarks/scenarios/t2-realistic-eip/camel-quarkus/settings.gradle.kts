rootProject.name = "t2-realistic-eip-camel-quarkus"
// v3.5: camel-quarkus-dsl + camel-quarkus-yaml (JVM mode) dropped — redundant
// with camel-standalone-* JVM baseline. Directories retained as source
// holders for the -native variants (sourceSets srcDir pattern).
include("camel-quarkus-dsl-native", "camel-quarkus-yaml-native")

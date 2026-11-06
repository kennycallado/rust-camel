import java.util.concurrent.TimeUnit

plugins {
    java
    id("io.quarkus") version "3.39.1"
    id("com.diffplug.spotless") version "6.25.0"
}

version = providers.gradleProperty("version").orNull ?: run {
    val bridgeRegistry = rootDir.resolve("../versions.toml")
    check(bridgeRegistry.isFile) {
        "bridge version registry not found: ${bridgeRegistry.absolutePath}"
    }
    val bridgeKey = "jms"
    Regex("""(?m)^\s*${Regex.escape(bridgeKey)}\s*=\s*"([^"]+)"\s*$""")
        .find(bridgeRegistry.readText())
        ?.groupValues
        ?.get(1)
        ?: error("bridge version key '$bridgeKey' not found in ${bridgeRegistry.absolutePath}")
}

repositories {
    mavenCentral()
}

val quarkusVersion = "3.39.1"

dependencies {
    implementation(enforcedPlatform("io.quarkus.platform:quarkus-bom:$quarkusVersion"))
    implementation("io.quarkus:quarkus-grpc")
    implementation("io.quarkus:quarkus-arc")
    implementation("io.quarkus:quarkus-config-yaml")
    implementation("io.quarkus:quarkus-logging-json")
    // OpenTelemetry: own pipeline with OTLP export. Egress is fail-closed
    // (see application.yml + OtlpEgressGuard) — no dial-out defaults.
    implementation("io.quarkus:quarkus-opentelemetry")

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
    // Embedded Classic broker (vm://) for durable-subscription tests; pinned to the client version.
    testImplementation("org.apache.activemq:activemq-broker:5.19.10")
    // In-memory SpanExporter for otel wiring tests (no collector, no egress)
    testImplementation("io.opentelemetry:opentelemetry-sdk-testing")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

// The full JVM suite (embedded TLS brokers + per-test consumers + Quarkus test model) exceeds
// Gradle's default 512m test-worker heap and dies with cascading OutOfMemoryError failures.
tasks.test {
    maxHeapSize = "3g"
    // CI logs must carry assertion messages and full exception chains for failed tests —
    // platform-specific failures (e.g. Windows fixture races) are triaged from the log alone.
    testLogging {
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
        events("failed")
    }
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

// ---------------------------------------------------------------------------------------------
// Pool evictor verifier (native-evictor-verification).
//
// Proves the connection-pool idle evictor on both bridges branches (ActiveMQ
// Classic and the messaginghub pool used for Artemis). The JVM run is the local
// gate; the native-image run is CI-only. `--no-fallback` is hard-coded: a
// fallback image starts on the JVM and would fake the native proof.
// ---------------------------------------------------------------------------------------------

val evictorVerifierClass = "org.rustcamel.jms.PoolEvictorVerifier"
val evictorVerifyDir = layout.buildDirectory.dir("evictor-verify")
val evictorVerifierClasspath = sourceSets.test.get().runtimeClasspath

val evictorClassicPassLine = "PASS branch=classic proofs=START,RETAINED,EVICT,SHUTDOWN"
val evictorArtemisPassLine = "PASS branch=artemis proofs=START,RETAINED,EVICT,SHUTDOWN"

/**
 * Shared CI gate for verifier output. Returns null only when the process exited
 * 0 AND both branch PASS lines are present; otherwise returns the rejection
 * reason. `evictorVerifyNative` and `evictorVerifierFaults` use this same
 * function, so the CI gate and the fault regressions exercise identical logic.
 */
fun validateVerifierOutput(exitCode: Int, stdout: String): String? {
    if (exitCode != 0) {
        return "verifier exited with code $exitCode (expected 0)"
    }
    val missing = listOf(evictorClassicPassLine, evictorArtemisPassLine)
        .filterNot { stdout.contains(it) }
    if (missing.isNotEmpty()) {
        return "verifier output is missing PASS line(s): ${missing.joinToString(" | ")}"
    }
    return null
}

fun isWindowsHost(): Boolean = org.gradle.internal.os.OperatingSystem.current().isWindows

/**
 * Reads NATIVE_BUILD_ARGS_NEUTRAL from native-build-args.env with the same
 * contract as scripts/xtask `read_neutral_native_build_args`: strip surrounding
 * double quotes. Fail-closed: a missing file or empty value fails the build, no
 * silent default.
 */
fun readNeutralNativeBuildArgs(): String {
    val argsFile = layout.projectDirectory.file("native-build-args.env").asFile
    check(argsFile.isFile) {
        "native build args file not found: ${argsFile.absolutePath}"
    }
    val prefix = "NATIVE_BUILD_ARGS_NEUTRAL="
    val raw = argsFile.readLines()
        .map { it.trim() }
        .firstOrNull { it.startsWith(prefix) }
        ?.removePrefix(prefix)
        ?: error("NATIVE_BUILD_ARGS_NEUTRAL not defined in ${argsFile.absolutePath}")
    val value =
        if (raw.length >= 2 && raw.startsWith("\"") && raw.endsWith("\"")) {
            raw.substring(1, raw.length - 1)
        } else {
            raw
        }
    check(value.isNotEmpty()) {
        "NATIVE_BUILD_ARGS_NEUTRAL is empty in ${argsFile.absolutePath}"
    }
    return value
}

// GraalVM's argfile parser interprets backslash escapes inside a quoted token,
// so a raw Windows classpath (C:\Users\...) must be escaped on write and
// decoded on the round-trip check.
fun escapeArgfileToken(value: String): String =
    value.replace("\\", "\\\\").replace("\"", "\\\"")

// Mirrors the launcher's tokenization for the subset written here: one token per
// newline, surrounding double quotes stripped, \\ and \" escapes decoded.
fun decodeArgfileToken(line: String): String {
    val trimmed = line.trim()
    val body =
        if (trimmed.length >= 2 && trimmed.startsWith("\"") && trimmed.endsWith("\"")) {
            trimmed.substring(1, trimmed.length - 1)
        } else {
            trimmed
        }
    val decoded = StringBuilder(body.length)
    var index = 0
    while (index < body.length) {
        val current = body[index]
        if (current == '\\' && index + 1 < body.length) {
            val next = body[index + 1]
            if (next == '\\' || next == '"') {
                decoded.append(next)
                index += 2
            } else {
                decoded.append(current)
                index += 1
            }
        } else {
            decoded.append(current)
            index += 1
        }
    }
    return decoded.toString()
}

fun resolveJavaBinary(): String {
    val executable = if (isWindowsHost()) "java.exe" else "java"
    val javaHome = providers.environmentVariable("JAVA_HOME").orNull
    if (!javaHome.isNullOrBlank()) {
        val candidate = File(javaHome, "bin/$executable")
        if (candidate.isFile) {
            return candidate.absolutePath
        }
    }
    return executable
}

/** Outcome of one fault-regression subprocess. */
data class EvictorFaultOutcome(
    val exitCode: Int,
    val timedOut: Boolean,
    val stdout: String,
    val stderr: String,
)

/**
 * Runs the verifier directly through ProcessBuilder: these cases EXPECT nonzero
 * exits and a deadline kill, which Gradle's exec helpers do not support. The
 * classpath travels in the CLASSPATH environment variable (no command-line
 * limit, no quoting pitfalls). stdout/stderr are drained on separate daemon
 * threads so a full pipe buffer cannot deadlock the child.
 */
fun runEvictorFaultCase(
    javaBinary: String,
    classpathValue: String,
    label: String,
    caseEnv: Map<String, String>,
    deadlineSeconds: Long,
): EvictorFaultOutcome {
    val builder = ProcessBuilder(javaBinary, evictorVerifierClass)
    val environment = builder.environment()
    environment.remove("POOL_EVICTOR_FAULT")
    environment.remove("POOL_EVICTOR_BUDGET_MS")
    environment["CLASSPATH"] = classpathValue
    environment.putAll(caseEnv)
    val process = builder.start()
    var stdout = ""
    var stderr = ""
    val stdoutDrain =
        Thread(
            { stdout = process.inputStream.bufferedReader().use { it.readText() } },
            "evictor-fault-$label-stdout",
        )
    val stderrDrain =
        Thread(
            { stderr = process.errorStream.bufferedReader().use { it.readText() } },
            "evictor-fault-$label-stderr",
        )
    stdoutDrain.isDaemon = true
    stderrDrain.isDaemon = true
    stdoutDrain.start()
    stderrDrain.start()
    val finished = process.waitFor(deadlineSeconds, TimeUnit.SECONDS)
    if (!finished) {
        process.destroyForcibly()
    }
    process.waitFor()
    stdoutDrain.join(5_000L)
    stderrDrain.join(5_000L)
    return EvictorFaultOutcome(process.exitValue(), !finished, stdout, stderr)
}

val evictorVerifyJvm =
    tasks.register<JavaExec>("evictorVerifyJvm") {
        group = "verification"
        description = "Runs the pool evictor verifier on the JVM (both branches)."
        dependsOn(tasks.compileTestJava)
        mainClass.set(evictorVerifierClass)
        classpath = evictorVerifierClasspath
        maxHeapSize = "1g"
    }

val evictorVerifyNativeImage =
    tasks.register("evictorVerifyNativeImage") {
        group = "verification"
        description = "Builds the verifier with native-image (argfile, --no-fallback)."
        dependsOn(tasks.compileTestJava)
        val verifierClasspath = evictorVerifierClasspath
        val verifyDir = evictorVerifyDir
        doLast {
            val neutralArgs = readNeutralNativeBuildArgs()
            val directory = verifyDir.get().asFile
            directory.mkdirs()
            val launcher = if (isWindowsHost()) "native-image.cmd" else "native-image"
            val classpathValue = verifierClasspath.asPath
            val tokens = mutableListOf("--no-fallback", "-Ob")
            tokens.addAll(neutralArgs.split(",").map { it.trim() }.filter { it.isNotEmpty() })
            tokens.add("-cp")
            tokens.add("\"" + escapeArgfileToken(classpathValue) + "\"")
            tokens.add(evictorVerifierClass)
            tokens.add("evictor-verifier")
            val argfile = File(directory, "native-image.args")
            argfile.writeText(tokens.joinToString("\n", postfix = "\n"))

            // Executable round-trip: re-read the argfile exactly as the launcher
            // will and prove the classpath survives quoting/escaping intact.
            val decoded = argfile.readLines().filter { it.isNotBlank() }.map { decodeArgfileToken(it) }
            val cpIndex = decoded.indexOf("-cp")
            check(cpIndex >= 0 && cpIndex + 1 < decoded.size) {
                "argfile $argfile has no -cp value"
            }
            check(decoded[cpIndex + 1] == classpathValue) {
                "argfile classpath round-trip mismatch: decoded length=${decoded[cpIndex + 1].length} " +
                    "expected length=${classpathValue.length}"
            }

            val execOutput =
                providers.exec {
                    commandLine(launcher, "@" + argfile.name)
                    workingDir = directory
                    // Consume the exit code explicitly: the default would throw
                    // ProcessExecutionException on the first provider query and
                    // hide the captured diagnostics below.
                    isIgnoreExitValue = true
                }
            val stdout = execOutput.standardOutput.asText.get()
            val stderr = execOutput.standardError.asText.get()
            val exitCode = execOutput.result.get().exitValue
            check(exitCode == 0) {
                "native-image failed with exit code $exitCode\n--- stdout ---\n$stdout\n--- stderr ---\n$stderr"
            }
        }
    }

val evictorVerifyNative =
    tasks.register("evictorVerifyNative") {
        group = "verification"
        description = "Runs the native verifier image and applies the shared CI gate."
        dependsOn(evictorVerifyNativeImage)
        val verifyDir = evictorVerifyDir
        doLast {
            val binaryName = if (isWindowsHost()) "evictor-verifier.exe" else "evictor-verifier"
            val binary = File(verifyDir.get().asFile, binaryName)
            check(binary.isFile) { "native verifier binary not found: ${binary.absolutePath}" }
            val execOutput =
                providers.exec {
                    commandLine(binary.absolutePath)
                    workingDir = binary.parentFile
                    // Consume the exit code explicitly so the shared validator
                    // runs for nonzero exits instead of Gradle throwing first.
                    isIgnoreExitValue = true
                }
            val stdout = execOutput.standardOutput.asText.get()
            val stderr = execOutput.standardError.asText.get()
            val exitCode = execOutput.result.get().exitValue
            val rejection = validateVerifierOutput(exitCode, stdout)
            if (rejection != null) {
                logger.error("native verifier stdout:\n$stdout")
                logger.error("native verifier stderr:\n$stderr")
                error("native verifier rejected: $rejection")
            }
        }
    }

val evictorVerifierFaults =
    tasks.register("evictorVerifierFaults") {
        group = "verification"
        description = "Proves the watchdog halts and the shared gate rejects bad verifier runs."
        dependsOn(tasks.compileTestJava)
        val verifierClasspath = evictorVerifierClasspath
        doLast {
            val javaBinary = resolveJavaBinary()
            val classpathValue = verifierClasspath.asPath
            val deadline = 60L
            logger.lifecycle("evictorVerifierFaults: java=$javaBinary classpathEntries=${verifierClasspath.files.size}")

            val blockCheckin =
                runEvictorFaultCase(
                    javaBinary,
                    classpathValue,
                    "a-block-checkin",
                    mapOf("POOL_EVICTOR_FAULT" to "block-checkin", "POOL_EVICTOR_BUDGET_MS" to "3000"),
                    deadline,
                )
            val blockHook =
                runEvictorFaultCase(
                    javaBinary,
                    classpathValue,
                    "b-block-hook",
                    mapOf("POOL_EVICTOR_FAULT" to "block-hook", "POOL_EVICTOR_BUDGET_MS" to "15000"),
                    deadline,
                )
            val failArtemis =
                runEvictorFaultCase(
                    javaBinary,
                    classpathValue,
                    "c-fail-artemis",
                    mapOf("POOL_EVICTOR_FAULT" to "fail-artemis"),
                    deadline,
                )
            val skipArtemis =
                runEvictorFaultCase(
                    javaBinary,
                    classpathValue,
                    "d-skip-artemis",
                    mapOf("POOL_EVICTOR_FAULT" to "skip-artemis"),
                    deadline,
                )

            val problems = mutableListOf<String>()
            fun reject(label: String, outcome: EvictorFaultOutcome, reason: String?) {
                if (reason == null) {
                    logger.lifecycle("evictorVerifierFaults[$label] exit=${outcome.exitCode} OK")
                    return
                }
                logger.error("evictorVerifierFaults[$label] rejected: $reason")
                problems.add(
                    "[$label] $reason\n--- stdout ---\n${outcome.stdout}\n--- stderr ---\n${outcome.stderr}"
                )
            }

            fun timeoutReason(outcome: EvictorFaultOutcome): String? =
                if (outcome.timedOut) "process exceeded the ${deadline}s deadline and was killed" else null

            // Surfaces the captured evidence lines in the build log even on success.
            fun echoEvidence(label: String, outcome: EvictorFaultOutcome, needles: List<String>) {
                val lines = outcome.stdout.lines()
                for (needle in needles) {
                    lines.filter { it.contains(needle) }
                        .take(2)
                        .forEach { logger.lifecycle("evictorVerifierFaults[$label] evidence: $it") }
                }
            }

            reject("a-block-checkin", blockCheckin, timeoutReason(blockCheckin) ?: run {
                when {
                    blockCheckin.exitCode != 2 ->
                        "exit=${blockCheckin.exitCode} (expected 2 from the watchdog halt)"
                    !blockCheckin.stdout.contains("FAULT entered block-checkin") ->
                        "stdout is missing 'FAULT entered block-checkin'"
                    !blockCheckin.stdout.contains("WATCHDOG budget=") ->
                        "stdout is missing a watchdog diagnostic"
                    blockCheckin.stdout.contains("PASS both") ->
                        "stdout must not contain 'PASS both' for a blocked check-in"
                    else -> null
                }
            })
            echoEvidence(
                "a-block-checkin",
                blockCheckin,
                listOf("FAULT entered block-checkin", "WATCHDOG budget="),
            )

            reject("b-block-hook", blockHook, timeoutReason(blockHook) ?: run {
                when {
                    blockHook.exitCode != 2 ->
                        "exit=${blockHook.exitCode} (expected 2: the halt must beat the blocked shutdown hook)"
                    !blockHook.stdout.contains("FAULT entered block-hook") ->
                        "stdout is missing 'FAULT entered block-hook'"
                    else -> null
                }
            })
            echoEvidence("b-block-hook", blockHook, listOf("FAULT entered block-hook"))

            reject("c-fail-artemis", failArtemis, timeoutReason(failArtemis) ?: run {
                when {
                    failArtemis.exitCode != 1 ->
                        "exit=${failArtemis.exitCode} (expected 1)"
                    !failArtemis.stdout.contains("FAIL branch=artemis") ->
                        "stdout is missing 'FAIL branch=artemis'"
                    failArtemis.stdout.contains("PASS both") ->
                        "stdout must not contain 'PASS both'"
                    else -> null
                }
            })
            echoEvidence("c-fail-artemis", failArtemis, listOf("FAIL branch=artemis"))

            val skipRejection = validateVerifierOutput(skipArtemis.exitCode, skipArtemis.stdout)
            reject("d-skip-artemis", skipArtemis, timeoutReason(skipArtemis) ?: run {
                when {
                    skipArtemis.exitCode != 0 ->
                        "exit=${skipArtemis.exitCode} (expected 0: skip-artemis returns success)"
                    !skipArtemis.stdout.contains(evictorClassicPassLine) ->
                        "stdout is missing the classic PASS line"
                    skipArtemis.stdout.contains(evictorArtemisPassLine) ->
                        "stdout must not contain the artemis PASS line"
                    skipRejection == null ->
                        "shared validator accepted an output with no artemis PASS line"
                    else -> null
                }
            })
            echoEvidence("d-skip-artemis", skipArtemis, listOf("FAULT entered skip-artemis", evictorClassicPassLine))
            logger.lifecycle("evictorVerifierFaults[d-skip-artemis] shared validator rejected exit=0 output: $skipRejection")

            // (e) Replay the nonzero case through the shared gate: a classic PASS
            // line is present, so only the exit-code branch can reject it.
            val replayRejection = validateVerifierOutput(1, failArtemis.stdout)
            when {
                !failArtemis.stdout.contains(evictorClassicPassLine) ->
                    problems.add(
                        "[e-replay] precondition failed: the fail-artemis output has no classic PASS line\n" +
                            "--- stdout ---\n${failArtemis.stdout}"
                    )
                replayRejection == null ->
                    problems.add("[e-replay] shared validator accepted a nonzero exit with a PASS line present")
                else ->
                    logger.lifecycle("evictorVerifierFaults[e-replay] validator rejected replayed output: $replayRejection")
            }

            if (problems.isNotEmpty()) {
                error("evictorVerifierFaults failed:\n\n" + problems.joinToString("\n\n"))
            }
        }
    }

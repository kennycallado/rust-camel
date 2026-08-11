mod changelog;
mod lint_context_citations;
mod lint_single_source;

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, Subcommand};

const GRAALVM_IMAGE: &str = "quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-21";
const EXPECTED_BINARY: &str = "build/native/jms-bridge";
const EXPECTED_BINARY_XML: &str = "build/native/xml-bridge";
const EXPECTED_BINARY_CXF: &str = "build/native/cxf-bridge";

#[derive(Parser)]
#[command(name = "xtask", about = "rust-camel build tasks")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[allow(clippy::enum_variant_names)]
enum Commands {
    /// Build the JMS bridge native binary using Docker (GraalVM CE)
    BuildJmsBridge {
        /// Version tag to pass to build-native.sh (e.g. 0.2.0)
        #[arg(long)]
        version: Option<String>,
        /// Clear Gradle cache before building
        #[arg(long)]
        no_cache: bool,
    },
    /// Build the XML bridge native binary using Docker (GraalVM CE)
    BuildXmlBridge {
        /// Version tag to pass to build-native.sh (e.g. 0.2.0)
        #[arg(long)]
        version: Option<String>,
        /// Clear Gradle cache before building
        #[arg(long)]
        no_cache: bool,
    },
    /// Build the CXF bridge native binary using Docker (GraalVM CE)
    BuildCxfBridge {
        /// Version tag to pass to build-native.sh (e.g. 0.2.0)
        #[arg(long)]
        version: Option<String>,
        /// Clear Gradle cache before building
        #[arg(long)]
        no_cache: bool,
    },
    /// Build a bridge native binary directly (no Docker) for macOS/Windows CI
    BuildBridgeNative {
        /// Bridge name: jms, xml, or cxf
        #[arg(long)]
        bridge: String,
        /// Version tag
        #[arg(long)]
        version: Option<String>,
        /// Target platform: macos-x86_64, macos-aarch64, windows-x86_64
        #[arg(long)]
        target: String,
    },
    /// Generate canonical route spec artifacts (JSON Schema, TypeScript types)
    Schema {
        /// Check on-disk schemas match freshly-regenerated ones. Exit non-zero on drift.
        #[arg(long)]
        check: bool,
    },
    /// Scan production source files for .unwrap() and .expect( calls.
    /// Exits non-zero if any violations are found.
    /// Escape hatch: append `// allow-unwrap` to the line.
    LintUnwrap,
    /// Scan source files for potential credential leakage in format macros
    /// and tracing macro structured fields.
    /// Exits non-zero if any violations are found.
    /// Escape hatch: append `// allow-secret` to the line.
    LintSecrets,
    /// Scan source files for error!() calls without a required
    /// `// log-policy:` annotation on the preceding line.
    /// See ADR-0012 for the convention.
    /// Escape hatches: append `// allow-log-levels` on the same line,
    /// or list `<relative path>:<line>` in
    /// `scripts/xtask/allowlist-log-levels.txt`.
    LintLogLevels,
    /// Enforce ADR-0049: pub enums in the contract crates must be
    /// #[non_exhaustive] or carry a `/// exhaustive-by-contract: <rationale>`
    /// rustdoc note. Exits non-zero on violations.
    LintNonExhaustive,
    /// Enforce ADR-0054: every #[ignore] must carry a reason string from a
    /// closed vocabulary. Exits non-zero on violations.
    LintIgnore,
    /// Validate CONTEXT.md and CONTEXT-MAP.md citation hygiene: path
    /// existence, anchor resolution, and (later) symbol validation against
    /// the workspace's own crate definitions. Exits non-zero on violations.
    LintContextCitations,
    /// Scan component crate source for `UriOption::new` calls outside
    /// `#[cfg(test)]` modules. Enforces the single-source-of-truth
    /// invariant: metadata MUST be macro-derived, not hand-written.
    LintSingleSource,
    /// Enforce publish-topology invariants: no cyclic dev/build-dependencies
    /// on publishable crates, and no publishable crate depends on camel-test
    /// (the publish-order leaf sink).
    LintPublishCycles,
    /// Compute the correct publish order for workspace crates by performing
    /// a topological sort over normal (non-dev) internal dependencies.
    /// Outputs shell commands suitable for publish-crates.sh.
    PublishOrder {
        /// Output as publish_crate lines for scripts/publish-crates.sh
        #[arg(long)]
        shell: bool,
    },
    /// Publish all workspace crates to crates.io in topological order.
    /// Skips crates already published and those with publish = false.
    Publish {
        /// Don't actually publish, just show what would be done
        #[arg(long)]
        dry_run: bool,
        /// Print the no_verify set and broken weak edges from the
        /// SCC-accurate cycle detector without publishing anything
        /// or mutating any Cargo.toml.
        #[arg(long)]
        show_cycles: bool,
    },
    /// Print the artifact hash for an OpenSpec change directory.
    /// Used by /bless to compute the hash shown to the expert, and by
    /// /apply to detect drift between blessed and current artifacts.
    HashArtifacts {
        /// Directory of the OpenSpec change.
        #[arg(long)]
        change_dir: String,
    },
    /// Generate categorized release notes from Conventional Commits.
    ///
    /// Writes Markdown to stdout (for the GitHub Release body) and
    /// diagnostics (warnings + SemVer recommendation) to stderr.
    /// Defaults to `<latest vX.Y.Z tag>..HEAD`.
    Changelog {
        /// Starting tag or SHA (exclusive). Defaults to the latest core tag.
        #[arg(long)]
        from: Option<String>,
        /// Ending ref. Defaults to HEAD.
        #[arg(long)]
        to: Option<String>,
        /// CI gate mode: exit non-zero if any commit mentions breaking
        /// without the `!:` subject marker. Used by `lint-commits` gate.
        #[arg(long)]
        check: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::BuildJmsBridge { version, no_cache } => {
            if let Err(e) = build_jms_bridge(version, no_cache) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::BuildXmlBridge { version, no_cache } => {
            if let Err(e) = build_xml_bridge(version, no_cache) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::BuildCxfBridge { version, no_cache } => {
            if let Err(e) = build_cxf_bridge(version, no_cache) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::BuildBridgeNative {
            bridge,
            version,
            target,
        } => {
            if let Err(e) = build_bridge_native(&bridge, version.as_deref(), &target) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Schema { check } => {
            if let Err(e) = run_schema_generation(check) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Commands::LintUnwrap => {
            let workspace_root = workspace_root_or_exit();
            match lint_unwrap(&workspace_root) {
                Ok(violations) if violations.is_empty() => {
                    println!("lint-unwrap: OK (no violations)");
                }
                Ok(violations) => {
                    println!("UNWRAP VIOLATIONS ({} found):", violations.len());
                    for v in &violations {
                        println!("  {}:{}  {}", v.file, v.line, v.snippet.trim());
                    }
                    eprintln!("\nlint-unwrap: FAILED");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("lint-unwrap error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::LintSecrets => {
            let workspace_root = workspace_root_or_exit();
            match lint_secrets(&workspace_root) {
                Ok(violations) if violations.is_empty() => {
                    println!("lint-secrets: OK (no violations)");
                }
                Ok(violations) => {
                    println!("SECRET LEAKAGE VIOLATIONS ({} found):", violations.len()); // allow-secret
                    for v in &violations {
                        println!("  {}:{}  {}", v.file, v.line, v.snippet.trim());
                        println!("    rule: {}", v.rule);
                    }
                    eprintln!("\nlint-secrets: FAILED");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("lint-secrets error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::LintLogLevels => {
            let workspace_root = workspace_root_or_exit();
            match lint_log_levels(&workspace_root) {
                Ok(violations) if violations.is_empty() => {
                    println!("lint-log-levels: OK (strict mode — 0 violations)");
                }
                Ok(violations) => {
                    println!("LOG-LEVEL VIOLATIONS ({} found):", violations.len());
                    for v in &violations {
                        println!("  {}:{}  {}", v.file, v.line, v.snippet.trim());
                        println!(
                            "    remedy: add one of `// log-policy: system-broken | outside-contract | handler-owned`"
                        );
                        println!("            on the preceding line. See ADR-0012.");
                    }
                    eprintln!("\nlint-log-levels: FAILED");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("lint-log-levels error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::LintContextCitations => {
            let workspace_root = workspace_root_or_exit();
            match lint_context_citations::lint_context_citations(&workspace_root) {
                Ok(violations) if violations.is_empty() => {
                    println!("lint-context-citations: OK (0 violations)");
                }
                Ok(violations) => {
                    println!("CONTEXT-CITATION VIOLATIONS ({} found):", violations.len());
                    for v in &violations {
                        println!("  {}:{}  {}", v.file, v.line, v.snippet.trim());
                    }
                    eprintln!("\nlint-context-citations: FAILED");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("lint-context-citations error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::LintSingleSource => {
            let workspace_root = workspace_root_or_exit();
            match lint_single_source::lint_single_source(&workspace_root) {
                Ok(violations) if violations.is_empty() => {
                    println!("lint-single-source: OK (no violations)");
                }
                Ok(violations) => {
                    println!("SINGLE-SOURCE VIOLATIONS ({} found):", violations.len());
                    for v in &violations {
                        println!("  {}:{}  {}", v.file, v.line, v.snippet.trim());
                    }
                    eprintln!("\nlint-single-source: FAILED");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("lint-single-source error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::LintPublishCycles => {
            let workspace_root = workspace_root_or_exit();
            if let Err(e) = lint_publish_cycles(&workspace_root) {
                eprintln!("lint-publish-cycles error: {e}");
                std::process::exit(1);
            }
            println!("lint-publish-cycles: OK (0 violations)");
        }
        Commands::LintNonExhaustive => {
            let workspace_root = workspace_root_or_exit();
            match lint_non_exhaustive(&workspace_root) {
                Ok(violations) if violations.is_empty() => {
                    println!("lint-non-exhaustive: OK (no violations)");
                }
                Ok(violations) => {
                    println!("NON-EXHAUSTIVE VIOLATIONS ({} found):", violations.len());
                    for v in &violations {
                        println!("  {}:{}  {}", v.file, v.line, v.snippet.trim());
                    }
                    eprintln!("\nlint-non-exhaustive: FAILED");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("lint-non-exhaustive error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::LintIgnore => {
            let workspace_root = workspace_root_or_exit();
            match lint_ignore(&workspace_root) {
                Ok(violations) if violations.is_empty() => {
                    println!("lint-ignore: OK (no violations)");
                }
                Ok(violations) => {
                    println!("IGNORE-POLICY VIOLATIONS ({} found):", violations.len());
                    for v in &violations {
                        println!("  {}:{}  {}", v.file, v.line, v.snippet.trim());
                    }
                    eprintln!("\nlint-ignore: FAILED");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("lint-ignore error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::PublishOrder { shell } => {
            let workspace_root = workspace_root_or_exit();
            if let Err(e) = publish_order(&workspace_root, shell) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Publish {
            dry_run,
            show_cycles: show_cycles_flag,
        } => {
            let workspace_root = workspace_root_or_exit();
            if show_cycles_flag {
                if let Err(e) = show_cycles(&workspace_root, &mut std::io::stdout()) {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            if let Err(e) = publish_crates(&workspace_root, dry_run) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::HashArtifacts { change_dir } => match artifact_hash::compute(&change_dir) {
            Ok(hash) => println!("{hash}"),
            Err(e) => {
                eprintln!("hash-artifacts: FAIL — {e}");
                std::process::exit(1);
            }
        },
        Commands::Changelog { from, to, check } => {
            if let Err(e) = changelog::run(from, to, check) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn validate_version(v: &str) -> Result<(), String> {
    let re = regex::Regex::new(r"^(dev|[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?)$").unwrap(); // allow-unwrap
    if !re.is_match(v) {
        return Err(format!(
            "Invalid version '{v}' — must be 'dev' or semver pattern MAJOR.MINOR.PATCH[-PRERELEASE]"
        ));
    }
    Ok(())
}

fn build_jms_bridge(version: Option<String>, no_cache: bool) -> Result<(), String> {
    build_bridge("JMS", "jms", EXPECTED_BINARY, version, no_cache)
}

fn build_xml_bridge(version: Option<String>, no_cache: bool) -> Result<(), String> {
    build_bridge("XML", "xml", EXPECTED_BINARY_XML, version, no_cache)
}

fn build_cxf_bridge(version: Option<String>, no_cache: bool) -> Result<(), String> {
    build_bridge("CXF", "cxf", EXPECTED_BINARY_CXF, version, no_cache)
}

fn build_bridge(
    bridge_name: &str,
    bridge_dir_name: &str,
    expected_binary: &str,
    version: Option<String>,
    no_cache: bool,
) -> Result<(), String> {
    // Validate version early to prevent path traversal or malformed filenames
    if let Some(ref v) = version {
        validate_version(v)?;
    }

    // 1. Locate workspace root
    let workspace_root = workspace_root()?;

    let bridge_dir = workspace_root.join("bridges").join(bridge_dir_name);

    // 2. Check Docker
    let docker_ok = Command::new("docker")
        .args(["info"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !docker_ok {
        return Err("Docker is required but not running. Start Docker and retry.".to_string());
    }

    // 3. Optional: clear Gradle cache
    if no_cache {
        let cache_dir = bridge_dir.join(".gradle-docker-cache");
        if cache_dir.exists() {
            std::fs::remove_dir_all(&cache_dir)
                .map_err(|e| format!("Failed to clear Gradle cache: {e}"))?;
            println!("Cleared Gradle cache at {}", cache_dir.display());
        }
    }

    // 4. Ensure the Gradle cache dir and build dir exist.
    let cache_dir = bridge_dir.join(".gradle-docker-cache");
    if !cache_dir.exists() {
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create Gradle cache dir: {e}"))?;
    }
    #[cfg(unix)]
    {
        let build_dir = bridge_dir.join("build");
        if !build_dir.exists() {
            std::fs::create_dir_all(&build_dir)
                .map_err(|e| format!("Failed to create build dir: {e}"))?;
        }
        use std::os::unix::fs::PermissionsExt;
        for dir in &[&build_dir, &cache_dir] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o777))
                .map_err(|e| format!("Failed to chmod {}: {e}", dir.display()))?;
        }
    }

    // 5. Build docker run args
    // Run as root — the GraalVM CE image sets USER 1001 but we need /lib
    // write access for the musl loader symlink. cleanup_permissions trap
    // in build-native.sh fixes ownership on exit.
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--user=0:0".to_string(),
        "--network=host".to_string(),
        format!("--volume={}:/project:z", bridge_dir.display()),
        "--workdir=/project".to_string(),
        "--env=GRADLE_USER_HOME=/project/.gradle-docker-cache".to_string(),
        // Native Image compiles and executes C helper probes in /tmp.
        // Keep /tmp executable on hosted runners with restrictive defaults.
        "--tmpfs=/tmp:rw,exec".to_string(),
        "--entrypoint".to_string(),
        "bash".to_string(),
    ];

    args.push(GRAALVM_IMAGE.to_string());

    // build-native.sh args
    args.push("./build-native.sh".to_string());
    args.push("--in-container".to_string());
    if let Some(ref v) = version {
        args.push("--version".to_string());
        args.push(v.clone());
    }

    println!("Building {bridge_name} bridge native image...");
    println!("  Image:     {GRAALVM_IMAGE}");
    println!("  Source:    {}", bridge_dir.display());
    if let Some(ref v) = version {
        println!("  Version:   {v}");
    }
    println!();

    let status = Command::new("docker")
        .args(&args)
        .status()
        .map_err(|e| format!("Failed to start docker: {e}"))?;

    if !status.success() {
        return Err(format!(
            "Docker build failed with exit code: {}",
            status.code().unwrap_or(-1)
        ));
    }

    // 5. Verify binary exists
    let binary_path = bridge_dir.join(expected_binary);
    if !binary_path.exists() {
        return Err(format!(
            "Build succeeded but binary not found at expected path: {}",
            binary_path.display()
        ));
    }

    // 6. On NixOS, patch the glibc-linked binary so it uses the Nix-store
    //    linker and libraries. This makes the binary runnable without
    //    enabling nix-ld at the system level.
    //    Skip for statically linked binaries (no dynamic interpreter to patch).
    #[cfg(target_os = "linux")]
    {
        if is_static_binary(&binary_path) {
            println!("Binary is statically linked — skipping NixOS patchelf.");
        } else {
            patchelf_for_nixos(&binary_path)?;
        }
    }

    // 7. Print summary
    let metadata =
        std::fs::metadata(&binary_path).map_err(|e| format!("Cannot stat binary: {e}"))?;
    let size_mb = metadata.len() as f64 / 1_048_576.0;

    let bytes =
        std::fs::read(&binary_path).map_err(|e| format!("Cannot read binary for SHA256: {e}"))?;
    let sha256 = sha256_hex(&bytes);

    println!("Build complete!");
    println!("  Path:   {}", binary_path.display());
    println!("  Size:   {:.1} MB", size_mb);
    println!("  SHA256: {sha256}");

    Ok(())
}

fn build_bridge_native(bridge: &str, version: Option<&str>, target: &str) -> Result<(), String> {
    let (bridge_name, bridge_dir, binary_name, extra_gradle_args) = match bridge {
        "jms" => ("JMS", "jms", "jms-bridge", ""),
        "xml" => ("XML", "xml", "xml-bridge", ""),
        "cxf" => (
            "CXF",
            "cxf",
            "cxf-bridge",
            "-x spotlessJavaCheck -x spotlessCheck",
        ),
        other => return Err(format!("Unknown bridge: {other}. Use jms, xml, or cxf.")),
    };

    let ver = version.unwrap_or("dev");
    validate_version(ver)?;

    // Validate target matches host OS/arch to prevent mislabeled artifacts
    let host_os = std::env::consts::OS;
    let host_arch = std::env::consts::ARCH;
    let target_os = if target.contains("linux") {
        "linux"
    } else if target.contains("macos") {
        "macos"
    } else if target.contains("windows") {
        "windows"
    } else {
        return Err(format!("Unknown target OS in: {target}"));
    };
    let target_arch = if target.contains("x86_64") {
        "x86_64"
    } else if target.contains("aarch64") {
        "aarch64"
    } else {
        return Err(format!("Unknown target arch in: {target}"));
    };
    if target_os != host_os || target_arch != host_arch {
        return Err(format!(
            "Target '{target}' does not match host '{host_os}-{host_arch}'. Cross-compilation is not supported."
        ));
    }

    let workspace_root = workspace_root()?;

    let bridge_path = workspace_root.join("bridges").join(bridge_dir);

    println!("Building {bridge_name} bridge native image (native, no Docker)...");
    println!("  Bridge:  {bridge}");
    println!("  Target:  {target}");
    println!("  Version: {ver}");
    println!();

    // Use gradlew script if available, otherwise invoke java with wrapper jar
    let gradle_cmd = if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    };
    let gradle_script = bridge_path.join(gradle_cmd);

    let (cmd, initial_args) = if gradle_script.exists() {
        (gradle_script, Vec::new())
    } else {
        let jar_path = bridge_path
            .join("gradle")
            .join("wrapper")
            .join("gradle-wrapper.jar");
        if !jar_path.exists() {
            return Err(format!(
                "Gradle wrapper not found (tried {} and {})",
                gradle_script.display(),
                jar_path.display()
            ));
        }
        let args = vec![
            "-cp".to_string(),
            jar_path
                .to_str()
                .ok_or("Non-UTF-8 path to gradle-wrapper.jar")?
                .to_string(),
            "org.gradle.wrapper.GradleWrapperMain".to_string(),
        ];
        (PathBuf::from("java"), args)
    };

    let mut args = initial_args;
    args.extend([
        "build".to_string(),
        "-Dquarkus.package.jar.enabled=false".to_string(),
        "-Dquarkus.native.enabled=true".to_string(),
        format!("-Pversion={ver}"),
        "--no-daemon".to_string(),
    ]);

    if !extra_gradle_args.is_empty() {
        args.extend(extra_gradle_args.split_whitespace().map(String::from));
    }

    // Native-image args: windows/macos get ONLY the platform-neutral list from
    // native-build-args.env (the same single source of truth the linux/docker
    // build-native.sh sources, where it additionally appends --static/--libc=musl).
    // musl/static args are linux-only and must NOT reach windows/macos.
    //
    // We pass them via the env var (comma form) rather than application.yml so
    // there is exactly ONE source: setting additional-build-args in YAML *and*
    // the env var makes SmallRye prioritize the indexed YAML form and silently
    // drop these args.
    let neutral_args = read_neutral_native_build_args(&bridge_path)?;

    let status = Command::new(&cmd)
        .args(&args)
        .current_dir(&bridge_path)
        .env("GRADLE_USER_HOME", bridge_path.join(".gradle-local-cache"))
        .env("QUARKUS_NATIVE_ADDITIONAL_BUILD_ARGS", &neutral_args)
        .status()
        .map_err(|e| format!("Failed to run Gradle: {e}"))?;

    if !status.success() {
        return Err(format!(
            "Gradle build failed with exit code: {}",
            status.code().unwrap_or(-1)
        ));
    }

    let runner = locate_native_runner(&bridge_path, binary_name, ver)?;

    let final_binary = bridge_path.join("build").join("native").join(binary_name);
    if runner != final_binary {
        let parent = final_binary
            .parent()
            .ok_or_else(|| format!("Cannot resolve parent of {}", final_binary.display()))?;
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create native dir: {e}"))?;
        std::fs::copy(&runner, &final_binary).map_err(|e| format!("Cannot copy binary: {e}"))?;
    }

    let metadata =
        std::fs::metadata(&final_binary).map_err(|e| format!("Cannot stat binary: {e}"))?;
    let size_mb = metadata.len() as f64 / 1_048_576.0;

    let bytes = std::fs::read(&final_binary).map_err(|e| format!("Cannot read binary: {e}"))?;
    let sha256 = sha256_hex(&bytes);

    println!("Build complete!");
    println!("  Path:   {}", final_binary.display());
    println!("  Size:   {:.1} MB", size_mb);
    println!("  SHA256: {sha256}");

    package_release(&final_binary, binary_name, ver, target, &bridge_path)?;

    Ok(())
}

/// Read the platform-neutral native-image build args from a bridge's
/// `native-build-args.env` file (the single source of truth shared with
/// `build-native.sh`).
///
/// The file defines a single shell-style assignment:
/// `NATIVE_BUILD_ARGS_NEUTRAL="arg1,arg2,..."`. We extract the quoted value and
/// return it verbatim for use as `QUARKUS_NATIVE_ADDITIONAL_BUILD_ARGS`.
fn read_neutral_native_build_args(bridge_path: &Path) -> Result<String, String> {
    let file = bridge_path.join("native-build-args.env");
    let contents = std::fs::read_to_string(&file)
        .map_err(|e| format!("Cannot read {}: {e}", file.display()))?;

    for line in contents.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("NATIVE_BUILD_ARGS_NEUTRAL=") {
            // Strip surrounding double quotes if present.
            let value = rest
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(rest);
            if value.is_empty() {
                return Err(format!(
                    "NATIVE_BUILD_ARGS_NEUTRAL is empty in {}",
                    file.display()
                ));
            }
            return Ok(value.to_string());
        }
    }

    Err(format!(
        "NATIVE_BUILD_ARGS_NEUTRAL not defined in {}",
        file.display()
    ))
}

fn locate_native_runner(
    bridge_path: &Path,
    binary_name: &str,
    version: &str,
) -> Result<PathBuf, String> {
    let build_dir = bridge_path.join("build");

    let canonical = build_dir.join("native").join(binary_name);
    if canonical.is_file() {
        return Ok(canonical);
    }

    let runner_name = format!("{binary_name}-{version}-runner");
    if let Ok(entries) = std::fs::read_dir(&build_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.contains(&runner_name)
                && !name_str.ends_with(".jar")
                && entry.path().is_file()
            {
                return Ok(entry.path());
            }
        }
    }

    let source_jar_dir = build_dir.join(format!("{binary_name}-{version}-native-image-source-jar"));
    let runner_in_source = source_jar_dir.join(format!("{binary_name}-{version}-runner"));
    if runner_in_source.is_file() {
        return Ok(runner_in_source);
    }

    Err(format!(
        "Native runner not found. Searched:\n  {}\n  build/*{runner_name}*\n  {}",
        canonical.display(),
        runner_in_source.display()
    ))
}

fn package_release(
    binary_path: &Path,
    binary_name: &str,
    version: &str,
    target: &str,
    bridge_dir: &Path,
) -> Result<(), String> {
    let is_windows = target.contains("windows");
    let dist_name = format!("{binary_name}-{version}-{target}");
    let build_dir = bridge_dir.join("build").join("release");
    let bin_dir = build_dir.join(&dist_name).join("bin");

    std::fs::create_dir_all(&bin_dir).map_err(|e| format!("Cannot create release dir: {e}"))?;

    let dest_binary = if is_windows {
        bin_dir.join(format!("{binary_name}.exe"))
    } else {
        bin_dir.join(binary_name)
    };

    std::fs::copy(binary_path, &dest_binary)
        .map_err(|e| format!("Cannot copy binary to release dir: {e}"))?;

    if is_windows {
        let archive_path = build_dir.join(format!("{dist_name}.zip"));
        let file =
            std::fs::File::create(&archive_path).map_err(|e| format!("Cannot create zip: {e}"))?;
        let mut zip_writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for entry in walkdir::WalkDir::new(build_dir.join(&dist_name))
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let rel = entry
                .path()
                .strip_prefix(&build_dir)
                .map_err(|e| format!("strip_prefix: {e}"))?;
            let rel_str = rel
                .to_str()
                .ok_or_else(|| format!("Non-UTF-8 path: {}", rel.display()))?;
            zip_writer
                .start_file(rel_str, options)
                .map_err(|e| format!("zip start_file: {e}"))?;
            let mut f = std::fs::File::open(entry.path())
                .map_err(|e| format!("open {}: {e}", entry.path().display()))?;
            std::io::copy(&mut f, &mut zip_writer).map_err(|e| format!("zip write: {e}"))?;
        }
        zip_writer
            .finish()
            .map_err(|e| format!("zip finish: {e}"))?;
        let sha = sha256_hex(&std::fs::read(&archive_path).map_err(|e| format!("read zip: {e}"))?);
        println!("Archive: {}", archive_path.display());
        println!("SHA256:  {sha}");
    } else {
        let archive_path = build_dir.join(format!("{dist_name}.tar.gz"));
        let status = Command::new("tar")
            .args([
                "-czf",
                archive_path.to_str().ok_or("Non-UTF-8 archive path")?,
                "-C",
                build_dir.to_str().ok_or("Non-UTF-8 build dir")?,
                &dist_name,
            ])
            .status()
            .map_err(|e| format!("tar failed: {e}"))?;
        if !status.success() {
            return Err("tar command failed".to_string());
        }
        let sha =
            sha256_hex(&std::fs::read(&archive_path).map_err(|e| format!("read tarball: {e}"))?);
        println!("Tarball: {}", archive_path.display());
        println!("SHA256:  {sha}");
    }

    Ok(())
}

/// Check if a binary is statically linked by looking for the absence of
/// a dynamic interpreter (`Requesting program interpreter`) in its ELF
/// program headers. Static binaries have no interpreter segment.
#[cfg(target_os = "linux")]
fn is_static_binary(binary: &Path) -> bool {
    let output = Command::new("readelf")
        .args(["-l", binary.to_str().unwrap_or_default()])
        .output();
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            !stdout.contains("Requesting program interpreter")
        }
        Err(_) => {
            eprintln!("Warning: readelf failed, assuming dynamic binary");
            false
        }
    }
}

/// On NixOS, the native binary is linked against glibc with
/// interpreter `/lib64/ld-linux-x86-64.so.2`, which does not exist
/// unless `nix-ld` is enabled at the system level.
///
/// This function detects NixOS, resolves the glibc store path via
/// `nix eval`, and calls `patchelf` to rewrite the interpreter and
/// rpath so the binary can run directly in a `nix develop` shell.
///
/// On non-NixOS Linux, this is a no-op. Errors are non-fatal warnings.
#[cfg(target_os = "linux")]
fn patchelf_for_nixos(binary: &Path) -> Result<(), String> {
    // Detect NixOS by reading /etc/os-release
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let is_nixos = os_release.lines().any(|l| l == "ID=nixos");
    if !is_nixos {
        return Ok(());
    }

    println!("NixOS detected — patching binary ELF interpreter and rpath...");

    // Check patchelf is available
    let has_patchelf = Command::new("patchelf")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_patchelf {
        println!(
            "  warning: patchelf not found — binary may not run on this NixOS host.\n\
             Add patchelf to your devShell packages or install it with `nix-env -iA nixpkgs.patchelf`."
        );
        return Ok(());
    }

    // Resolve glibc store path: `nix eval --raw nixpkgs#glibc.outPath`
    let glibc_out = Command::new("nix")
        .args(["eval", "--raw", "nixpkgs#glibc.outPath"])
        .output()
        .map_err(|e| format!("Failed to run `nix eval`: {e}"))?;
    if !glibc_out.status.success() {
        println!(
            "  warning: could not resolve glibc via `nix eval` — skipping ELF patch.\n\
             Binary may not run on this NixOS host unless nix-ld is enabled."
        );
        return Ok(());
    }
    let glibc_store = String::from_utf8_lossy(&glibc_out.stdout)
        .trim()
        .to_string();

    // Resolve zlib store path: `nix eval --raw nixpkgs#zlib.outPath`
    let zlib_out = Command::new("nix")
        .args(["eval", "--raw", "nixpkgs#zlib.outPath"])
        .output()
        .map_err(|e| format!("Failed to run `nix eval` for zlib: {e}"))?;
    let zlib_rpath = if zlib_out.status.success() {
        let zlib_store = String::from_utf8_lossy(&zlib_out.stdout).trim().to_string();
        format!(":{zlib_store}/lib")
    } else {
        String::new() // zlib missing from nix store — skip
    };

    let interpreter = format!("{glibc_store}/lib/ld-linux-x86-64.so.2");
    let rpath = format!("{glibc_store}/lib{zlib_rpath}");

    // Ensure binary is writable. The file is often owned by the Docker
    // container uid (e.g. quarkus/1001). Use sudo chmod if needed.
    let is_writable = std::fs::OpenOptions::new().write(true).open(binary).is_ok();
    if !is_writable {
        let status = Command::new("sudo")
            .args(["chmod", "a+w", binary.to_str().unwrap()]) // allow-unwrap
            .status()
            .map_err(|e| format!("sudo chmod failed: {e}"))?;
        if !status.success() {
            return Err(
                "sudo chmod a+w failed — cannot make binary writable for patchelf".to_string(),
            );
        }
    }

    // patchelf --set-interpreter
    let status = Command::new("patchelf")
        .args(["--set-interpreter", &interpreter, binary.to_str().unwrap()]) // allow-unwrap
        .status()
        .map_err(|e| format!("patchelf --set-interpreter failed: {e}"))?;
    if !status.success() {
        return Err(format!(
            "patchelf --set-interpreter exited with code {}",
            status.code().unwrap_or(-1)
        ));
    }

    // patchelf --set-rpath (so libz.so.1 and libc.so.6 are found)
    let status = Command::new("patchelf")
        .args(["--set-rpath", &rpath, binary.to_str().unwrap()]) // allow-unwrap
        .status()
        .map_err(|e| format!("patchelf --set-rpath failed: {e}"))?;
    if !status.success() {
        return Err(format!(
            "patchelf --set-rpath exited with code {}",
            status.code().unwrap_or(-1)
        ));
    }

    println!("  Interpreter: {interpreter}");
    println!("  Rpath:       {rpath}");

    Ok(())
}

/// Walk up from `start` looking for a `Cargo.toml` containing `[workspace]`
/// with a `bridges/` directory as sentinel. Returns the workspace root.
pub fn find_workspace_root_from(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    for _ in 0..10 {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists()
            && std::fs::read_to_string(&cargo_toml)
                .map(|contents| contents.contains("[workspace]"))
                .unwrap_or(false)
            && current.join("bridges").exists()
        {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

/// Locate the workspace root using the runtime CWD, not the compile-time
/// `CARGO_MANIFEST_DIR`. The compile-time macro is baked into the binary
/// and goes stale when a shared target dir reuses an xtask binary compiled
/// in a different worktree.
fn workspace_root() -> Result<PathBuf, String> {
    let start =
        std::env::current_dir().map_err(|e| format!("Cannot determine current directory: {e}"))?;
    find_workspace_root_from(&start).ok_or_else(|| {
        "Cannot locate workspace root with bridges/ — are you running from the rust-camel workspace?".to_string()
    })
}

/// Same as `workspace_root()` but prints the error and exits on failure.
fn workspace_root_or_exit() -> PathBuf {
    workspace_root().unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    })
}

fn run_schema_generation(check: bool) -> Result<(), String> {
    let workspace_root = workspace_root()?;
    let schemas_dir = workspace_root.join("schemas");
    let dsl_dir = schemas_dir.join("dsl");
    let ts_dir = schemas_dir.join("ts");
    // Directory creation is DEFERRED to the `!check` branch — `--check` mode
    // must be non-mutating (no create_dir_all, no writes anywhere).

    // Pure content-producing fns (no IO).
    let canonical_schema = generate_canonical_schema_content()?;
    let dsl_schema = generate_dsl_schema()?;
    let component_metadata_schema = generate_component_metadata_schema()?;

    // JSON schema artifacts: (path, content).
    let schema_artifacts: Vec<(std::path::PathBuf, String)> = vec![
        (
            schemas_dir.join("canonical-route-spec.json"),
            canonical_schema,
        ),
        (dsl_dir.join("route-schema.json"), dsl_schema),
        (
            schemas_dir.join("component-metadata.json"),
            component_metadata_schema,
        ),
    ];

    // TS artifacts: ALWAYS export to a tempdir first (never write to ts_dir
    // before the drift check; otherwise --check mode would overwrite disk
    // and the subsequent diff would always succeed).
    let temp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let ts_config = ts_rs::Config::new().with_out_dir(temp.path());
    export_ts_types(&ts_config)?;
    let mut temp_ts_files: Vec<(String, String)> = collect_ts_files(temp.path())?;
    // Append missing dependency files that ts_rs references but doesn't auto-generate.
    // serde_json::Value fields trigger `import type { JsonValue } from "./serde_json/JsonValue"`.
    temp_ts_files.push((
        "serde_json/JsonValue.ts".to_string(),
        "// unknown is safer than any — callers must narrow before use.\n// This file is hand-maintained because ts-rs references the import\n// path but does not auto-generate the file.\nexport type JsonValue = unknown;\n".to_string(),
    ));

    if check {
        // NON-MUTATING: no create_dir_all, no writes anywhere. Only reads.
        let mut drift: Vec<String> = Vec::new();
        for (path, expected) in &schema_artifacts {
            let actual = std::fs::read_to_string(path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            if &actual != expected {
                drift.push(path.display().to_string());
            }
        }
        if let Err(ts_err) = check_ts_drift(&ts_dir, &temp_ts_files) {
            drift.push(ts_err);
        }
        // Verify camel-lint embedded schema matches the canonical generated copy.
        let lint_schema = workspace_root.join("crates/camel-lint/schema/route-schema.json");
        let dsl_schema_path = dsl_dir.join("route-schema.json");
        let lint_content = std::fs::read(&lint_schema)
            .map_err(|e| format!("read {}: {e}", lint_schema.display()))?;
        let dsl_content = std::fs::read(&dsl_schema_path)
            .map_err(|e| format!("read {}: {e}", dsl_schema_path.display()))?;
        if lint_content != dsl_content {
            drift.push(format!(
                "{} differs from {} ({} vs {} bytes)",
                lint_schema.display(),
                dsl_schema_path.display(),
                lint_content.len(),
                dsl_content.len()
            ));
        }

        if !drift.is_empty() {
            return Err(format!(
                "Schema drift detected. Re-run `cargo xtask schema` (regenerates \
                 schemas/dsl/route-schema.json only) and also copy: \
                 cp schemas/dsl/route-schema.json crates/camel-lint/schema/route-schema.json\n  {}",
                drift.join("\n  ")
            ));
        }
        println!("OK: all schemas and TS types match.");
    } else {
        // MUTATING: create all dirs lazily, then write everything.
        std::fs::create_dir_all(&schemas_dir).map_err(|e| format!("create schemas/: {e}"))?;
        std::fs::create_dir_all(&dsl_dir).map_err(|e| format!("create schemas/dsl/: {e}"))?;
        std::fs::create_dir_all(&ts_dir).map_err(|e| format!("create schemas/ts/: {e}"))?;
        // Write JSON schema artifacts.
        for (path, content) in &schema_artifacts {
            std::fs::write(path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
            println!("Generated: {}", path.display());
        }
        // Write TS files from tempdir to ts_dir.
        for (fname, content) in &temp_ts_files {
            let target = ts_dir.join(fname);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create {}: {e}", parent.display()))?;
            }
            std::fs::write(&target, content)
                .map_err(|e| format!("write {}: {e}", target.display()))?;
        }
        // Remove stale TS files on disk that no longer get generated.
        let on_disk_files: Vec<String> = std::fs::read_dir(&ts_dir)
            .map_err(|e| format!("read ts_dir: {e}"))?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".ts"))
            .collect();
        let generated: Vec<&String> = temp_ts_files.iter().map(|(n, _)| n).collect();
        for fname in on_disk_files {
            if !generated.contains(&&fname) {
                let stale = ts_dir.join(&fname);
                std::fs::remove_file(&stale).ok();
                println!("Removed stale: {}", stale.display());
            }
        }
        println!(
            "Generated {} TS files in: {}",
            temp_ts_files.len(),
            ts_dir.display()
        );
        println!("Done! Run `quicktype` manually for Go/Python types.");
    }

    Ok(())
}

fn generate_canonical_schema_content() -> Result<String, String> {
    let schema = schemars::schema_for!(camel_api::CanonicalRouteSpec);
    serde_json::to_string_pretty(&schema).map_err(|e| format!("serialize canonical: {e}"))
}

fn generate_component_metadata_schema() -> Result<String, String> {
    let schema = schemars::schema_for!(camel_api::component_metadata::ComponentMetadata);
    serde_json::to_string_pretty(&schema).map_err(|e| format!("serialize component-metadata: {e}"))
}

fn export_ts_types(ts_config: &ts_rs::Config) -> Result<(), String> {
    // Helper macro to reduce boilerplate
    macro_rules! ts_export {
        ($config:ident, $ty:ty) => {
            <$ty as ts_rs::TS>::export($config)
                .map_err(|e| format!("TS {}: {e}", stringify!($ty)))?;
        };
    }

    // -----------------------------------------------------------------------
    // camel_api types — CanonicalRouteSpec and its transitive dependencies
    // -----------------------------------------------------------------------
    ts_export!(ts_config, camel_api::CanonicalRouteSpec);
    ts_export!(ts_config, camel_api::runtime::CanonicalStepSpec);
    ts_export!(ts_config, camel_api::runtime::CanonicalWhenSpec);
    ts_export!(ts_config, camel_api::runtime::CanonicalCircuitBreakerSpec);
    ts_export!(ts_config, camel_api::runtime::CanonicalSplitExpressionSpec);
    ts_export!(ts_config, camel_api::runtime::CanonicalSplitAggregationSpec);
    ts_export!(
        ts_config,
        camel_api::runtime::CanonicalAggregateStrategySpec
    );
    ts_export!(ts_config, camel_api::runtime::CanonicalAggregateSpec);
    ts_export!(ts_config, camel_api::declarative::LanguageExpressionDef);
    // ExceptionDisposition — referenced by CatchClauseData in the DSL
    ts_export!(ts_config, camel_api::error_handler::ExceptionDisposition);
    // CanonicalConcurrencySpec — referenced by CanonicalRouteSpec
    ts_export!(ts_config, camel_api::runtime::CanonicalConcurrencySpec);
    // StreamSplitConfig + StreamSplitFormat — referenced by CanonicalSplitExpressionSpec
    ts_export!(ts_config, camel_api::splitter::StreamSplitConfig);
    ts_export!(ts_config, camel_api::splitter::StreamSplitFormat);

    // -----------------------------------------------------------------------
    // camel_dsl::route_ast DSL types — every type with the cfg_attr derive
    // -----------------------------------------------------------------------
    // Route-level config / error handling / security
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslRoute);
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslStep);
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslSecurityPolicy);
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslPermissionPolicy);
    ts_export!(
        ts_config,
        camel_dsl::route_ast::RouteDslPermissionValueSource
    );
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslPermissionContext);
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslErrorHandler);
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslOnException);
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslRedeliveryPolicy);
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslCircuitBreaker);

    // Step wrappers
    ts_export!(ts_config, camel_dsl::route_ast::AggregateStep);
    ts_export!(ts_config, camel_dsl::route_ast::BeanStep);
    ts_export!(ts_config, camel_dsl::route_ast::ChoiceStep);
    ts_export!(ts_config, camel_dsl::route_ast::ConvertBodyToStep);
    ts_export!(ts_config, camel_dsl::route_ast::DelayStep);
    ts_export!(ts_config, camel_dsl::route_ast::DoTryStep);
    ts_export!(ts_config, camel_dsl::route_ast::DynamicRouterStep);
    ts_export!(ts_config, camel_dsl::route_ast::EnrichStep);
    ts_export!(ts_config, camel_dsl::route_ast::FilterStep);
    ts_export!(ts_config, camel_dsl::route_ast::FunctionStep);
    ts_export!(ts_config, camel_dsl::route_ast::LoadBalanceStep);
    ts_export!(ts_config, camel_dsl::route_ast::LogStep);
    ts_export!(ts_config, camel_dsl::route_ast::LoopStep);
    ts_export!(ts_config, camel_dsl::route_ast::MarshalStep);
    ts_export!(ts_config, camel_dsl::route_ast::MulticastStep);
    ts_export!(ts_config, camel_dsl::route_ast::PollEnrichStep);
    ts_export!(ts_config, camel_dsl::route_ast::RecipientListStep);
    ts_export!(ts_config, camel_dsl::route_ast::RoutingSlipStep);
    ts_export!(ts_config, camel_dsl::route_ast::ScriptStep);
    ts_export!(ts_config, camel_dsl::route_ast::SetBodyStep);
    ts_export!(ts_config, camel_dsl::route_ast::SetHeaderStep);
    ts_export!(ts_config, camel_dsl::route_ast::SetPropertyStep);
    ts_export!(ts_config, camel_dsl::route_ast::SplitStep);
    ts_export!(ts_config, camel_dsl::route_ast::StopStep);
    ts_export!(ts_config, camel_dsl::route_ast::StreamCacheStep);
    ts_export!(ts_config, camel_dsl::route_ast::ThrottleStep);
    ts_export!(ts_config, camel_dsl::route_ast::ToStep);
    ts_export!(ts_config, camel_dsl::route_ast::TransformStep);
    ts_export!(ts_config, camel_dsl::route_ast::UnmarshalStep);
    ts_export!(ts_config, camel_dsl::route_ast::ValidateStep);
    ts_export!(ts_config, camel_dsl::route_ast::WireTapStep);

    // Step data types
    ts_export!(ts_config, camel_dsl::route_ast::AggregateData);
    ts_export!(ts_config, camel_dsl::route_ast::BeanStepData);
    ts_export!(ts_config, camel_dsl::route_ast::CatchClauseData);
    ts_export!(ts_config, camel_dsl::route_ast::ChoiceData);
    ts_export!(ts_config, camel_dsl::route_ast::DelayFullConfig);
    ts_export!(ts_config, camel_dsl::route_ast::DoTryData);
    ts_export!(ts_config, camel_dsl::route_ast::DynamicRouterData);
    ts_export!(ts_config, camel_dsl::route_ast::EnrichConfig);
    ts_export!(ts_config, camel_dsl::route_ast::FinallyData);
    ts_export!(ts_config, camel_dsl::route_ast::FunctionData);
    ts_export!(ts_config, camel_dsl::route_ast::LoadBalanceData);
    ts_export!(ts_config, camel_dsl::route_ast::LogConfig);
    ts_export!(ts_config, camel_dsl::route_ast::LogMessageExpr);
    ts_export!(ts_config, camel_dsl::route_ast::MulticastData);
    ts_export!(ts_config, camel_dsl::route_ast::PredicateBlock);
    ts_export!(ts_config, camel_dsl::route_ast::RecipientListData);
    ts_export!(ts_config, camel_dsl::route_ast::RoutingSlipData);
    ts_export!(ts_config, camel_dsl::route_ast::ScriptData);
    ts_export!(ts_config, camel_dsl::route_ast::SetBodyConfig);
    ts_export!(ts_config, camel_dsl::route_ast::SetHeaderData);
    ts_export!(ts_config, camel_dsl::route_ast::SetPropertyData);
    ts_export!(ts_config, camel_dsl::route_ast::SplitData);
    ts_export!(ts_config, camel_dsl::route_ast::SplitExpressionConfig);
    ts_export!(ts_config, camel_dsl::route_ast::StreamConfigYaml);
    ts_export!(ts_config, camel_dsl::route_ast::ThrottleData);
    ts_export!(ts_config, camel_dsl::route_ast::LoopFullConfig);
    ts_export!(ts_config, camel_dsl::route_ast::LoopWhileExpr);
    ts_export!(ts_config, camel_dsl::route_ast::EnrichBody);
    ts_export!(ts_config, camel_dsl::route_ast::StreamCacheConfig);

    // Untagged enums (body/data unions)
    ts_export!(ts_config, camel_dsl::route_ast::DelayBody);
    ts_export!(ts_config, camel_dsl::route_ast::LogBody);
    ts_export!(ts_config, camel_dsl::route_ast::LogMessageData);
    ts_export!(ts_config, camel_dsl::route_ast::LoopData);
    ts_export!(ts_config, camel_dsl::route_ast::SetBodyData);
    ts_export!(ts_config, camel_dsl::route_ast::SplitExpressionYaml);
    ts_export!(ts_config, camel_dsl::route_ast::StreamCacheBody);

    // Template types
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslTemplateParameter);
    ts_export!(ts_config, camel_dsl::route_ast::RouteDslTemplatedRoute);

    Ok(())
}

fn collect_ts_files(dir: &std::path::Path) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("ts") {
            continue;
        }
        let fname = entry.file_name().to_string_lossy().into_owned();
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        out.push((fname, content));
    }
    Ok(out)
}

fn check_ts_drift(ts_dir: &std::path::Path, temp_files: &[(String, String)]) -> Result<(), String> {
    let mut drift = Vec::new();

    for (fname, expected) in temp_files {
        let disk_path = ts_dir.join(fname);
        match std::fs::read_to_string(&disk_path) {
            Ok(actual) if &actual == expected => { /* match */ }
            Ok(_) => drift.push(format!("{}: content differs", fname)),
            Err(_) => drift.push(format!("{}: missing on disk", fname)),
        }
    }

    if let Ok(entries) = std::fs::read_dir(ts_dir) {
        for entry in entries.filter_map(Result::ok) {
            let fname = entry.file_name().to_string_lossy().into_owned();
            if fname.ends_with(".ts") && !temp_files.iter().any(|(n, _)| n == &fname) {
                drift.push(format!("{}: stale on disk (no longer generated)", fname));
            }
        }
    }

    if drift.is_empty() {
        Ok(())
    } else {
        Err(format!("schemas/ts/ drift: {}", drift.join("; ")))
    }
}

/// Schema envelope for the public DSL surface.
///
/// Excludes templates (which use `noyalib::compat::serde_yaml::Value` —
/// a type that does not implement JsonSchema). Templates are internal
/// machinery; the public SDK schema is `{routes: [...]}` only.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct RouteDslSchemaEnvelope {
    /// Optional JSON Schema URL (mirrors RouteDslRoutes.schema_url — added in Task 9).
    #[serde(rename = "$schema", default, skip_serializing)]
    schema_url: Option<String>,
    /// Route definitions.
    #[serde(default)]
    routes: Vec<camel_dsl::RouteDslRoute>,
}

const DSL_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/kennycallado/rust-camel/main/schemas/dsl/route-schema.json";

fn generate_dsl_schema() -> Result<String, String> {
    let schema = schemars::schema_for!(RouteDslSchemaEnvelope);
    let mut value =
        serde_json::to_value(&schema).map_err(|e| format!("serialize DSL schema: {e}"))?;
    if let Some(obj) = value.as_object_mut() {
        // $id self-identifies the schema (per JSON Schema 2020-12 spec).
        // Tools like ajv use $id for $ref resolution.
        obj.insert(
            "$id".to_string(),
            serde_json::Value::String(DSL_SCHEMA_URL.to_string()),
        );
    }
    serde_json::to_string_pretty(&value).map_err(|e| format!("re-serialize DSL schema: {e}"))
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(data))
}

/// A single lint violation: file path, 1-based line number, line content.
#[derive(Debug, PartialEq)]
pub struct Violation {
    pub file: String,
    pub line: usize,
    pub snippet: String,
}

/// Returns true if the file path looks like a test file that should be excluded
/// from the unwrap scan.
fn is_test_file(path: &std::path::Path) -> bool {
    use std::path::Component;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    path.components()
        .any(|c| c == Component::Normal("tests".as_ref()))
        || name.starts_with("test_")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || name == "tests.rs"
        || name == "build.rs"
}

/// Core scanner: scan source `src` (a single `.rs` file) for `.unwrap()` / `.expect(` calls
/// that are NOT in test scope, attribute, or comment lines, and NOT marked with `// allow-unwrap`.
///
/// This function is extracted from [`lint_unwrap`] for unit-testability.
/// It uses a character-level state machine to correctly ignore braces inside
/// string/char literals, raw strings, line comments, and block comments,
/// preventing false test-scope-exit when unbalanced braces appear in literals (rc-4fs).
fn lint_unwrap_src(src: &str, file_path: &str) -> Vec<Violation> {
    use regex::Regex;

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum ScanState {
        Normal,
        StringLit,
        CharLit,
        LineComment,
        BlockComment,
        RawStr(usize),
    }

    let unwrap_re = Regex::new(r"\.(unwrap\(\)|expect\()").expect("valid regex"); // allow-unwrap
    let lines: Vec<&str> = src.lines().collect();

    let mut current_state = ScanState::Normal;
    let mut pending_test_attr = false;
    let mut test_scope_entry_depth: Option<i32> = None;
    let mut brace_depth: i32 = 0;
    let mut violations = Vec::new();

    for (line_idx, raw_line) in lines.iter().enumerate() {
        let trimmed = raw_line.trim();

        // Detect test attributes only when not already inside a test scope.
        if test_scope_entry_depth.is_none()
            && (trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[test]"))
        {
            pending_test_attr = true;
        }

        let entering_test_scope = pending_test_attr && test_scope_entry_depth.is_none();

        // State-machine brace counting — persists across lines.
        let mut chars = trimmed.chars().peekable();
        while let Some(ch) = chars.next() {
            match current_state {
                ScanState::Normal => match ch {
                    '{' => {
                        brace_depth += 1;
                        if pending_test_attr && test_scope_entry_depth.is_none() {
                            test_scope_entry_depth = Some(brace_depth - 1);
                            pending_test_attr = false;
                        }
                    }
                    '}' => {
                        brace_depth -= 1;
                        if let Some(entry) = test_scope_entry_depth
                            && brace_depth <= entry
                        {
                            test_scope_entry_depth = None;
                        }
                    }
                    '/' if chars.peek() == Some(&'/') => {
                        current_state = ScanState::LineComment;
                        chars.next();
                    }
                    '/' if chars.peek() == Some(&'*') => {
                        current_state = ScanState::BlockComment;
                        chars.next();
                    }
                    '"' => {
                        current_state = ScanState::StringLit;
                    }
                    '\'' => {
                        // Distinguish char literal ('a', '\n') from lifetime ('a, 'static, '_).
                        let next = chars.peek().copied();
                        let is_lifetime = next
                            .map(|c| c.is_ascii_alphanumeric() || c == '_')
                            .unwrap_or(false)
                            && {
                                let mut tmp = chars.clone();
                                tmp.next();
                                !matches!(tmp.peek(), Some('\'') | Some('\\'))
                            };
                        if !is_lifetime {
                            current_state = ScanState::CharLit;
                        }
                    }
                    'r' => {
                        // Potential raw string: r"..." or r#"..."# etc.
                        let mut hash_count: usize = 0;
                        let mut lookahead = chars.clone();
                        while lookahead.peek() == Some(&'#') {
                            hash_count += 1;
                            lookahead.next();
                        }
                        if lookahead.peek() == Some(&'"') {
                            current_state = ScanState::RawStr(hash_count);
                            for _ in 0..hash_count {
                                chars.next();
                            }
                            chars.next();
                        }
                    }
                    _ => {}
                },
                ScanState::StringLit => match ch {
                    '\\' => {
                        chars.next();
                    }
                    '"' => {
                        current_state = ScanState::Normal;
                    }
                    _ => {}
                },
                ScanState::CharLit => match ch {
                    '\\' => {
                        chars.next();
                    }
                    '\'' => {
                        current_state = ScanState::Normal;
                    }
                    _ => {}
                },
                ScanState::LineComment => {
                    // Consume remaining chars; state resets at EOL below.
                }
                ScanState::BlockComment => {
                    if ch == '*' && chars.peek() == Some(&'/') {
                        current_state = ScanState::Normal;
                        chars.next();
                    }
                }
                ScanState::RawStr(n) => {
                    if ch == '"' {
                        let mut count = 0;
                        let mut lookahead = chars.clone();
                        while lookahead.peek() == Some(&'#') {
                            count += 1;
                            lookahead.next();
                        }
                        if count >= n {
                            current_state = ScanState::Normal;
                            for _ in 0..count {
                                chars.next();
                            }
                        }
                    }
                }
            }
        }

        // Line comments end at the newline boundary.
        if current_state == ScanState::LineComment {
            current_state = ScanState::Normal;
        }

        // Clear pending_test_attr if no brace was opened on a semicolon line.
        if pending_test_attr && test_scope_entry_depth.is_none() && trimmed.contains(';') {
            pending_test_attr = false;
        }

        // Skip: the attribute line itself, the line that opens a test scope,
        // and all lines inside a test scope.
        if pending_test_attr || entering_test_scope || test_scope_entry_depth.is_some() {
            continue;
        }

        // Skip pure comment lines (only when not mid-block-comment).
        if current_state == ScanState::Normal && trimmed.starts_with("//") {
            continue;
        }

        // Skip lines with the escape hatch — also check the next line because
        // rustfmt sometimes moves `// allow-unwrap` onto the line after `expect(`
        // when the call opens a block.
        let next_line_allow = lines
            .get(line_idx + 1)
            .map(|l| l.trim() == "// allow-unwrap")
            .unwrap_or(false);
        if raw_line.contains("// allow-unwrap") || next_line_allow {
            continue;
        }

        if unwrap_re.is_match(raw_line) {
            violations.push(Violation {
                file: file_path.to_string(),
                line: line_idx + 1,
                snippet: raw_line.to_string(),
            });
        }
    }

    violations
}

/// Scan all workspace `src/**/*.rs` files (excluding test files and build.rs)
/// for `.unwrap()` and `.expect(` calls not marked with `// allow-unwrap`.
///
/// NOTE: This is a lexical scanner. UFCS forms like `Option::unwrap(x)` are
/// not caught. They are rare in this codebase; add `// allow-unwrap` if needed.
pub fn lint_unwrap(workspace_root: &Path) -> Result<Vec<Violation>, String> {
    use std::path::Component;
    use walkdir::WalkDir;

    let mut violations = Vec::new();

    for entry in WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if is_test_file(path) {
            continue;
        }
        // Only process files under a `src` component (portable, no string slicing)
        if !path
            .components()
            .any(|c| c == Component::Normal("src".as_ref()))
        {
            continue;
        }
        // Skip target and worktree directories
        if path.components().any(|c| {
            c == Component::Normal("target".as_ref())
                || c == Component::Normal(".worktrees".as_ref())
        }) {
            continue;
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;

        violations.extend(lint_unwrap_src(&content, &path.to_string_lossy()));
    }

    Ok(violations)
}

/// Core scanner: scan source `src` (a single `.rs` file) for `pub enum`
/// declarations that violate ADR-0049: a contract-crate pub enum must carry
/// `#[non_exhaustive]` OR a directly attached
/// `/// exhaustive-by-contract: <non-empty rationale>` rustdoc note.
/// A plain `//` comment or an empty rationale does NOT satisfy the rule.
///
/// The scanner is purely lexical (no I/O, no parser) so it can be unit-tested
/// in isolation via `lint_non_exhaustive_src`. The walker that drives it is
/// [`lint_non_exhaustive`].
pub fn lint_non_exhaustive_src(src: &str, file_path: &str) -> Vec<Violation> {
    use regex::Regex;

    let enum_re = Regex::new(r"^\s*pub\s+enum\s+(\w+)").expect("valid regex"); // allow-unwrap
    let note_re = Regex::new(r"^///\s*exhaustive-by-contract:\s*(\S.*)$").expect("valid regex"); // allow-unwrap

    let lines: Vec<&str> = src.lines().collect();
    let mut violations = Vec::new();

    for (idx, raw_line) in lines.iter().enumerate() {
        if !enum_re.is_match(raw_line) {
            continue;
        }

        // Walk backwards over the attached attribute/doc/comment region.
        // The region terminates at the first blank line, at the first non-
        // attribute/doc/comment line, or at the start of the file.
        let mut has_non_exhaustive = false;
        let mut has_valid_note = false;

        let mut i = idx;
        while i > 0 {
            i -= 1;
            let trimmed = lines[i].trim();

            if trimmed.is_empty() {
                // Blank line: rustdoc above is detached, does not satisfy.
                break;
            }
            if trimmed.starts_with("#[") {
                if trimmed.contains("non_exhaustive") {
                    has_non_exhaustive = true;
                }
                continue;
            }
            if trimmed.starts_with("///") {
                if !has_valid_note && note_re.is_match(trimmed) {
                    has_valid_note = true;
                }
                continue;
            }
            if trimmed.starts_with("//") {
                // Plain comment: included in the region but does NOT satisfy
                // the rule (only `///` rustdoc counts).
                continue;
            }
            // Anything else (use, struct, code, …): region ends.
            break;
        }

        if !has_non_exhaustive && !has_valid_note {
            violations.push(Violation {
                file: file_path.to_string(),
                line: idx + 1,
                snippet: raw_line.to_string(),
            });
        }
    }

    violations
}

/// Scan the three contract-crate source roots
/// (`crates/camel-api/src`, `crates/components/camel-component-api/src`,
/// `crates/languages/camel-language-api/src`) for `pub enum` declarations
/// that violate ADR-0049. See [`lint_non_exhaustive_src`] for the rule.
///
/// Test files are excluded by [`is_test_file`] so a pub enum inside
/// `#[cfg(test)] mod tests { ... }` is not released contract surface.
/// Returns true if `path` contains a `.worktrees/` component that is a
/// STRICT SUBDIRECTORY of `workspace_root` (i.e. a nested worktree).
///
/// The current worktree's own path is NOT considered nested — when the
/// workspace itself lives under `.worktrees/<branch>/`, every path under the
/// workspace root would otherwise be incorrectly excluded.
fn is_nested_worktree(path: &Path, workspace_root: &Path) -> bool {
    use std::path::Component;
    let rel = match path.strip_prefix(workspace_root) {
        Ok(r) => r,
        Err(_) => return false,
    };
    rel.components()
        .any(|c| c == Component::Normal(".worktrees".as_ref()))
}

pub fn lint_non_exhaustive(workspace_root: &Path) -> Result<Vec<Violation>, String> {
    use std::path::Component;
    use walkdir::WalkDir;

    let roots = [
        workspace_root.join("crates/camel-api/src"),
        workspace_root.join("crates/components/camel-component-api/src"),
        workspace_root.join("crates/languages/camel-language-api/src"),
    ];

    let mut violations = Vec::new();

    for root in roots.iter() {
        if !root.exists() {
            // Be permissive: a missing root is not a hard error (e.g. when
            // the crate is filtered out of a partial checkout). The other
            // roots are still scanned.
            continue;
        }

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            if is_test_file(path) {
                continue;
            }
            if path
                .components()
                .any(|c| c == Component::Normal("target".as_ref()))
            {
                continue;
            }
            if is_nested_worktree(path, workspace_root) {
                continue;
            }

            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;

            violations.extend(lint_non_exhaustive_src(&content, &path.to_string_lossy()));
        }
    }

    Ok(violations)
}

/// Read the ignore-policy allowlist from `scripts/xtask/allowlist-ignore.txt`.
///
/// Same format as `allowlist-log-levels.txt`: one relative path per line,
/// `#` for comments, blank lines ignored. Returns an empty set if the file
/// does not exist (caller treats empty set as "no allowlist entries").
pub fn load_ignore_allowlist(workspace_root: &Path) -> std::collections::HashSet<String> {
    let allowlist_path = workspace_root
        .join("scripts")
        .join("xtask")
        .join("allowlist-ignore.txt");
    std::fs::read_to_string(&allowlist_path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .map(|l| l.trim().to_string())
        .collect()
}

/// Classified prefix on a `#[ignore = "..."]` reason string. The `Err` arm
/// carries a short rule code that becomes the prefix of the violation snippet
/// (e.g. `ignore:invalid-prefix:`, `ignore:empty-detail:`).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum IgnoreReasonKind {
    PreBuilt,
    SlowTest,
}

/// Validate a reason string against the closed vocabulary from ADR-0054.
///
/// Returns `Ok(kind)` for a well-formed reason, or `Err(code)` where
/// `code` is one of `"invalid-prefix"` / `"empty-detail"`. The caller
/// emits the rule code as `ignore:<code>:` in the Violation snippet.
fn validate_ignore_reason(reason: &str) -> Result<IgnoreReasonKind, &'static str> {
    if let Some(detail) = reason.strip_prefix("requires pre-built ") {
        if detail.trim().is_empty() {
            return Err("empty-detail");
        }
        return Ok(IgnoreReasonKind::PreBuilt);
    }
    // `requires live` was removed from the closed vocabulary per ADR-0054 rev.
    // Emit a migration error instead of a generic invalid-prefix.
    // Only match when followed by space or colon (not when `live` is part of
    // a longer word like `livewire`).
    if reason.starts_with("requires live ") || reason.starts_with("requires live:") {
        return Err("migration-error");
    }
    if let Some(detail) = reason.strip_prefix("slow test: ") {
        if detail.trim().is_empty() {
            return Err("empty-detail");
        }
        return Ok(IgnoreReasonKind::SlowTest);
    }
    Err("invalid-prefix")
}

/// Sentinel `file` value for allowlist reverse-check violations that have no
/// single source line (see ADR-0054 allowlist coupling rules).
const IGNORE_ALLOWLIST_SENTINEL_FILE: &str = "<allowlist>";

/// Prefix path under which allowlist entries must be a direct child.
const IGNORE_ALLOWLIST_PREFIX: &str = "crates/components/camel-component-wasm/tests/";

/// Scan all workspace `crates/**/*.rs` and `examples/**/*.rs` files for
/// `#[ignore]` attributes that violate ADR-0054.
///
/// Rules enforced:
///   - Every `#[ignore]` must carry a reason string from the closed
///     vocabulary: `requires pre-built <detail>` | `slow test: <detail>`.
///   - Bare `#[ignore]` is rejected.
///   - The allowlist `scripts/xtask/allowlist-ignore.txt` is checked
///     bidirectionally (forward + reverse + mixed-reason). See ADR-0054.
///
/// Exclusion rules:
///   - Files under `target/`, `.worktrees/`, `scripts/`, `bridges/` are skipped.
///   - Files NOT under a `crates/` or `examples/` subdirectory are skipped.
///   - Test files are NOT skipped (unlike `lint_log_levels`) — `#[ignore]`
///     semantics in `tests/` are exactly the surface the lint must police.
pub fn lint_ignore(workspace_root: &Path) -> Result<Vec<Violation>, String> {
    use regex::Regex;
    use std::collections::HashMap;
    use std::path::Component;
    use walkdir::WalkDir;

    let allowlist = load_ignore_allowlist(workspace_root);
    // Use `r#"..."#` (not `r"..."`) because the regex body contains `"`.
    // The regex itself: `#[ignore]` optionally followed by ` = "..."`.
    let ignore_re = Regex::new(r#"#\[ignore\s*(?:=\s*"([^"]*)")?\]"#).expect("valid regex"); // allow-unwrap

    // Per-file state: (has_any_pre_built, has_any_non_pre_built)
    // Drives the forward / mixed-reason checks.
    let mut file_state: HashMap<String, (bool, bool)> = HashMap::new();

    let mut violations = Vec::new();

    for entry in WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }

        // Compute the path relative to the workspace root so the
        // component-based filters see the in-workspace layout (e.g. when
        // the workspace itself lives under `.worktrees/<branch>/`).
        let rel = path.strip_prefix(workspace_root).unwrap_or(path);
        let rel_components: Vec<_> = rel.components().collect();

        // Skip meta-tooling / build directories.
        if rel.components().any(|c| {
            c == Component::Normal("target".as_ref())
                || c == Component::Normal(".worktrees".as_ref())
                || c == Component::Normal("scripts".as_ref())
                || c == Component::Normal("bridges".as_ref())
        }) {
            continue;
        }

        // Only scan files under a `crates/` or `examples/` subdirectory.
        if rel_components.is_empty() {
            continue;
        }
        let first = rel_components[0];
        if first != Component::Normal("crates".as_ref())
            && first != Component::Normal("examples".as_ref())
        {
            continue;
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;

        // Normalize the relative path to forward slashes for consistent
        // allowlist comparison (Windows paths use `\`).
        let file_rel = rel.to_string_lossy().replace('\\', "/");

        let mut has_pre_built = false;
        let mut has_non_pre_built = false;

        for (line_idx, raw_line) in content.lines().enumerate() {
            let trimmed = raw_line.trim();

            // Skip comment lines (single-line `//` and inner-doc `//!`).
            // This prevents false positives on lines like
            // `// REDIS-009: ... (#[ignore] by default)` and
            // `//! All tests ... #[ignore] ...`.
            if trimmed.starts_with("//") {
                continue;
            }

            // A line may carry multiple `#[ignore]` attributes (unusual but
            // legal). Find all matches on this line.
            for cap in ignore_re.captures_iter(raw_line) {
                let line_no = line_idx + 1;
                if let Some(reason_match) = cap.get(1) {
                    let reason = reason_match.as_str();
                    match validate_ignore_reason(reason) {
                        Ok(IgnoreReasonKind::PreBuilt) => {
                            has_pre_built = true;
                        }
                        Ok(IgnoreReasonKind::SlowTest) => {
                            has_non_pre_built = true;
                        }
                        Err(code) => {
                            violations.push(Violation {
                                file: file_rel.clone(),
                                line: line_no,
                                snippet: format!(
                                    "ignore:{}: reason {:?} must use one of the closed-vocabulary prefixes: \
                                     `requires pre-built <detail>` | `slow test: <detail>` \
                                     (see ADR-0054) — {}",
                                    code, reason, raw_line.trim()
                                ),
                            });
                        }
                    }
                } else {
                    // Bare `#[ignore]` — no reason string supplied.
                    violations.push(Violation {
                        file: file_rel.clone(),
                        line: line_no,
                        snippet: format!(
                            "ignore:missing-reason: bare #[ignore] — add a reason string from the \
                             closed vocabulary (see ADR-0054): {}",
                            raw_line.trim()
                        ),
                    });
                }
            }
        }

        file_state.insert(file_rel, (has_pre_built, has_non_pre_built));
    }

    // -------- Forward check --------
    // Every file containing a `requires pre-built` test must be in the
    // allowlist (which the CI job consumes to run those tests).
    for (file_rel, (has_pre_built, _)) in &file_state {
        if *has_pre_built && !allowlist.contains(file_rel) {
            violations.push(Violation {
                file: file_rel.clone(),
                line: 0,
                snippet: format!(
                    "ignore:pre-built-not-in-allowlist: file contains `requires pre-built` tests \
                     but is not listed in scripts/xtask/allowlist-ignore.txt (see ADR-0054): \
                     {}",
                    file_rel
                ),
            });
        }
    }

    // -------- Reverse check --------
    // For each allowlist entry, verify (a) the path is in-scope, (b) the
    // file exists, (c) the file contains at least one pre-built test.
    for entry in &allowlist {
        // (a) In-scope: direct-child `.rs` file under the WASM tests dir.
        let after_prefix = match entry.strip_prefix(IGNORE_ALLOWLIST_PREFIX) {
            Some(s) => s,
            None => {
                violations.push(Violation {
                    file: IGNORE_ALLOWLIST_SENTINEL_FILE.to_string(),
                    line: 0,
                    snippet: format!(
                        "ignore:allowlist-out-of-scope: allowlist entry must be a direct-child \
                         `.rs` file under `crates/components/camel-component-wasm/tests/` \
                         (see ADR-0054): {}",
                        entry
                    ),
                });
                continue;
            }
        };
        if after_prefix.is_empty() || after_prefix.contains('/') || !after_prefix.ends_with(".rs") {
            violations.push(Violation {
                file: IGNORE_ALLOWLIST_SENTINEL_FILE.to_string(),
                line: 0,
                snippet: format!(
                    "ignore:allowlist-out-of-scope: allowlist entry must be a direct-child \
                     `.rs` file under `crates/components/camel-component-wasm/tests/` \
                     (see ADR-0054): {}",
                    entry
                ),
            });
            continue;
        }

        // (b) Exists on disk.
        let full_path = workspace_root.join(entry);
        if !full_path.is_file() {
            violations.push(Violation {
                file: IGNORE_ALLOWLIST_SENTINEL_FILE.to_string(),
                line: 0,
                snippet: format!(
                    "ignore:allowlist-stale: allowlist entry points to a non-existent file \
                     (see ADR-0054): {}",
                    entry
                ),
            });
            continue;
        }

        // (c) Contains at least one `requires pre-built` test.
        let has_pre_built = file_state.get(entry).map(|(pb, _)| *pb).unwrap_or(false);
        if !has_pre_built {
            violations.push(Violation {
                file: IGNORE_ALLOWLIST_SENTINEL_FILE.to_string(),
                line: 0,
                snippet: format!(
                    "ignore:allowlist-no-pre-built-test: allowlist entry must contain at least \
                     one `requires pre-built` test (see ADR-0054): {}",
                    entry
                ),
            });
        }
    }

    // -------- Mixed-reason check --------
    // For each allowlisted file, every `#[ignore]` reason must be
    // `requires pre-built`. A file with mixed reasons would cause the CI
    // job to incorrectly execute live-service tests.
    for entry in &allowlist {
        if let Some((_, has_non_pre_built)) = file_state.get(entry)
            && *has_non_pre_built
        {
            violations.push(Violation {
                file: entry.clone(),
                line: 0,
                snippet: format!(
                    "ignore:allowlist-mixed-reasons: allowlisted file must contain ONLY \
                     `requires pre-built` tests (the wasm-integration CI job would otherwise \
                     run live-service tests — see ADR-0054): {}",
                    entry
                ),
            });
        }
    }

    Ok(violations)
}

/// Scan all workspace `src/**/*.rs` files for `error!()` calls not annotated
/// with one of:
///   // log-policy: system-broken
///   // log-policy: outside-contract
///   // log-policy: handler-owned   (forbids error! — must be warn!/debug!)
///
/// Exclusion rules:
///   - Test files (`tests/`, `*_test.rs`, `*_tests.rs`) and `build.rs` skipped by name.
///   - Inside production files, `#[cfg(test)] mod ...` and `#[test] fn ...`
///     blocks are tracked and excluded (ported from lint_unwrap's
///     `pending_test_attr` / `test_scope_entry_depth` logic).
///
/// See ADR-0012 for the convention.

#[derive(Debug, PartialEq)]
enum LogPolicyKind {
    SystemBroken,
    OutsideContract,
    HandlerOwned,
    Unknown(String),
}

fn parse_log_policy(line: &str) -> Option<LogPolicyKind> {
    let trimmed = line.trim();
    if !trimmed.starts_with("//") {
        return None;
    }
    let payload = trimmed.trim_start_matches('/').trim();
    if !payload.starts_with("log-policy:") {
        return None;
    }
    let kind = payload.trim_start_matches("log-policy:").trim();
    Some(match kind {
        "system-broken" => LogPolicyKind::SystemBroken,
        "outside-contract" => LogPolicyKind::OutsideContract,
        "handler-owned" => LogPolicyKind::HandlerOwned,
        other => LogPolicyKind::Unknown(other.to_string()),
    })
}

/// Returns true if the function enclosing `line_idx` contains at least one of:
///   - `increment_errors(` (metric call)
///   - `force_unhealthy_for_route(` (health pin)
///   - an `if !bridged { ... }` guard wrapping the error! call.
///
/// Lexical approximation:
///   - The enclosing function is found by walking backwards to the nearest `fn `.
///   - The function body is bounded by brace-balanced scanning from the `fn`.
///   - Guard detection walks backwards counting braces; if we hit `if !bridged`
///     before exiting the enclosing scope, we're inside the guard.
///
/// Limitations: brace-counting is purely lexical; braces inside string literals
/// or comments can affect counts. Unusual cases can be suppressed with
/// `// allow-log-levels`.
fn has_replacement_signal(lines: &[&str], error_line_idx: usize) -> bool {
    let fn_start = (0..=error_line_idx)
        .rev()
        .find(|&i| lines.get(i).map(|l| l.contains("fn ")).unwrap_or(false))
        .unwrap_or(0);
    let mut depth: i32 = 0;
    let mut seen_open = false;
    let mut fn_end = error_line_idx;
    for (i, line) in lines.iter().enumerate().skip(fn_start) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    seen_open = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if seen_open && depth <= 0 {
            fn_end = i;
            break;
        }
    }
    let body_text: String = lines[fn_start..=fn_end.min(lines.len().saturating_sub(1))].join("\n");
    if body_text.contains("increment_errors(") {
        return true;
    }
    if body_text.contains("force_unhealthy_for_route(") {
        return true;
    }
    let mut d: i32 = 0;
    for (_idx, line) in lines[..=error_line_idx].iter().enumerate().rev() {
        for ch in line.chars() {
            match ch {
                '}' => d += 1,
                '{' => d -= 1,
                _ => {}
            }
        }
        if d < 0 && line.contains("if !bridged") {
            return true;
        }
    }
    false
}

const LABEL_REGEX: &str = r"^(b-prime|e|g):[a-z][a-z0-9-]*:[a-z][a-z0-9-]+$";
const BD_ID_REGEX: &str = r"bd\s+[a-z0-9][a-z0-9-]*";
const TODO_MARKER_REGEX: &str = r"TODO\(ADR-0012-[a-z-]+\)";

/// Extract the string-literal second argument of `increment_errors(...)` if
/// present on this line. Returns None if the call doesn't appear or the
/// argument can't be extracted as a string literal.
fn extract_increment_errors_label(line: &str) -> Option<&str> {
    let idx = line.find("increment_errors(")?;
    let after = &line[idx + "increment_errors(".len()..];
    let comma = after.find(',')?;
    let rest = after[comma + 1..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Walks the enclosing function body and checks every
/// `increment_errors(route_id, "label")` call. Returns a Violation for the
/// first label that doesn't match LABEL_REGEX. Returns None if all labels
/// match or there are no `increment_errors` calls.
fn check_labels_in_function(lines: &[&str], error_line_idx: usize) -> Option<Violation> {
    use regex::Regex;
    let label_re = Regex::new(LABEL_REGEX).expect("valid label regex"); // allow-unwrap

    let fn_start = (0..=error_line_idx)
        .rev()
        .find(|&i| lines.get(i).map(|l| l.contains("fn ")).unwrap_or(false))
        .unwrap_or(0);
    let mut depth: i32 = 0;
    let mut seen_open = false;
    let mut fn_end = error_line_idx;
    for (i, line) in lines.iter().enumerate().skip(fn_start) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    seen_open = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if seen_open && depth <= 0 {
            fn_end = i;
            break;
        }
    }

    for (i, line) in lines.iter().enumerate().take(fn_end + 1).skip(fn_start) {
        if let Some(label) = extract_increment_errors_label(line)
            && !label_re.is_match(label)
        {
            return Some(Violation {
                file: String::new(), // filled in by caller
                line: i + 1,
                snippet: format!(
                    "{}  (increment_errors label '{}' does not match <category>:<component>:<site> with category in {{b-prime, e, g}} — see ADR-0012)",
                    line.trim(),
                    label
                ),
            });
        }
    }
    None
}

fn should_report(
    _lines: &[&str],
    _line_idx: usize,
    raw_line: &str,
    file_rel: &str,
    allowlist: &std::collections::HashSet<String>,
) -> bool {
    let key = format!("{}:{}", file_rel, _line_idx + 1);
    if allowlist.contains(&key) {
        return false;
    }
    if raw_line.contains("// allow-log-levels") {
        return false;
    }
    true
}

/// Counts `// allow-log-levels` occurrences across all scanned `.rs` files.
/// Returns a Vec of (file, line) for each inline escape.
///
/// Excludes `scripts/xtask/` because that directory contains the lint itself —
/// its doc comments, string literals, test fixtures, and regex definitions all
/// mention the marker syntax and would otherwise be self-flagged as escapes.
/// ADR-0012 applies to component code under `crates/` and `examples/`, not to
/// meta-tooling.
fn count_inline_escapes(workspace_root: &Path) -> Result<Vec<(String, usize)>, String> {
    use std::path::Component;
    use walkdir::WalkDir;
    let escape_re = regex::Regex::new(r"//\s*allow-log-levels").expect("valid regex"); // allow-unwrap
    let mut locations = Vec::new();
    for entry in WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if is_test_file(path) {
            continue;
        }
        if !path
            .components()
            .any(|c| c == Component::Normal("src".as_ref()))
        {
            continue;
        }
        // Skip target/, nested .worktrees/, and scripts/ subdirectories.
        // Use strip_prefix so we don't skip files when the workspace root itself
        // lives inside a worktree (CI branches, parallel worktrees).
        let rel = path.strip_prefix(workspace_root).unwrap_or(path);
        if rel.components().any(|c| {
            c == Component::Normal("target".as_ref())
                || c == Component::Normal(".worktrees".as_ref())
                || c == Component::Normal("scripts".as_ref())
        }) {
            continue;
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        let file_rel = path
            .strip_prefix(workspace_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());
        for (idx, line) in content.lines().enumerate() {
            if escape_re.is_match(line) {
                locations.push((file_rel.clone(), idx + 1));
            }
        }
    }
    Ok(locations)
}

/// For each inline escape at (file, line), check the preceding 3 lines for:
///   1. A TODO(ADR-0012-...) marker.
///   2. A bd id reference.
///      Returns a Violation per escape missing either.
fn validate_inline_escape_markers(
    workspace_root: &Path,
    locations: &[(String, usize)],
) -> Vec<Violation> {
    let todo_re = regex::Regex::new(TODO_MARKER_REGEX).expect("valid todo regex"); // allow-unwrap
    let bd_re = regex::Regex::new(BD_ID_REGEX).expect("valid bd id regex"); // allow-unwrap

    let mut violations = Vec::new();
    for (file_rel, line_no) in locations {
        let path = workspace_root.join(file_rel);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let lines: Vec<&str> = content.lines().collect();
        let idx = line_no.saturating_sub(1);
        let window_start = idx.saturating_sub(3);
        let window_end = idx.min(lines.len().saturating_sub(1));
        let window: String = lines[window_start..=window_end].join("\n");
        if !todo_re.is_match(&window) {
            let line_text = lines.get(idx).copied().unwrap_or("");
            violations.push(Violation {
                file: file_rel.clone(),
                line: *line_no,
                snippet: format!(
                    "{}  (inline // allow-log-levels requires TODO(ADR-0012-<flavor>) marker within 3 lines — see ADR-0012 Task 6B)",
                    line_text.trim()
                ),
            });
            continue;
        }
        if !bd_re.is_match(&window) {
            let line_text = lines.get(idx).copied().unwrap_or("");
            violations.push(Violation {
                file: file_rel.clone(),
                line: *line_no,
                snippet: format!(
                    "{}  (TODO marker must reference a live bd id: 'bd <id>' — see ADR-0012 Task 6B)",
                    line_text.trim()
                ),
            });
        }
    }
    violations
}

/// Check if an error! site is structurally excluded from ADR-0012 lint.
///
/// Structural exclusions are symbol-bound (NOT file-bound): the lint checks
/// whether the error! falls inside a specific `impl ... for Type` block.
///
/// Current exclusions:
/// - camel-log LogProducer: user-output mechanism, NOT framework telemetry.
///   Per oracle ruling ses_16262b201ffeCmO67e3T6qa73b.
fn is_structurally_excluded(file_rel: &str, lines: &[&str], line_idx: usize) -> bool {
    // camel-log LogProducer — symbol-bound inside `impl Service<Exchange> for LogProducer`
    if file_rel.contains("camel-log/src/lib.rs") {
        let impl_start = lines
            .iter()
            .position(|l| l.contains("impl Service<Exchange> for LogProducer"));
        if let Some(start) = impl_start {
            let mut depth: i32 = 0;
            let mut seen_open = false;
            for (i, line) in lines.iter().enumerate().skip(start) {
                for ch in line.chars() {
                    match ch {
                        '{' => {
                            depth += 1;
                            seen_open = true;
                        }
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                if seen_open && depth <= 0 {
                    return line_idx >= start && line_idx <= i;
                }
            }
        }
    }
    false
}

pub fn lint_log_levels(workspace_root: &Path) -> Result<Vec<Violation>, String> {
    use regex::Regex;
    use std::path::Component;
    use walkdir::WalkDir;

    let error_re = Regex::new(r"\berror!\s*\(").expect("valid regex"); // allow-unwrap

    let allowlist_path = workspace_root
        .join("scripts")
        .join("xtask")
        .join("allowlist-log-levels.txt");
    let allowlist: std::collections::HashSet<String> = std::fs::read_to_string(&allowlist_path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .map(|l| l.trim().to_string())
        .collect();

    let inline_locations = count_inline_escapes(workspace_root)?;
    // Marker validation is a regular violation (not a structural failure).
    let inline_marker_violations =
        validate_inline_escape_markers(workspace_root, &inline_locations);

    let mut violations = Vec::new();

    for entry in WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if is_test_file(path) {
            continue;
        }
        if !path
            .components()
            .any(|c| c == Component::Normal("src".as_ref()))
        {
            continue;
        }
        // Skip target/ dirs and nested .worktrees/ subdirectories.
        // Use strip_prefix so we don't skip files when the workspace root itself
        // lives inside a worktree (CI branches, parallel worktrees).
        let rel = path.strip_prefix(workspace_root).unwrap_or(path);
        if rel.components().any(|c| {
            c == Component::Normal("target".as_ref())
                || c == Component::Normal(".worktrees".as_ref())
        }) {
            continue;
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;

        let file_rel = path
            .strip_prefix(workspace_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());

        let lines: Vec<&str> = content.lines().collect();
        let mut pending_test_attr = false;
        let mut test_scope_entry_depth: Option<i32> = None;
        let mut brace_depth: i32 = 0;

        for (line_idx, raw_line) in lines.iter().enumerate() {
            let trimmed = raw_line.trim();

            if test_scope_entry_depth.is_none()
                && (trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[test]"))
            {
                pending_test_attr = true;
            }

            let entering_test_scope = pending_test_attr && test_scope_entry_depth.is_none();

            for ch in trimmed.chars() {
                match ch {
                    '{' => {
                        brace_depth += 1;
                        if pending_test_attr && test_scope_entry_depth.is_none() {
                            test_scope_entry_depth = Some(brace_depth - 1);
                            pending_test_attr = false;
                        }
                    }
                    '}' => {
                        brace_depth -= 1;
                        if let Some(entry) = test_scope_entry_depth
                            && brace_depth <= entry
                        {
                            test_scope_entry_depth = None;
                        }
                    }
                    _ => {}
                }
            }

            if pending_test_attr && test_scope_entry_depth.is_none() && trimmed.contains(';') {
                pending_test_attr = false;
            }

            if pending_test_attr || entering_test_scope || test_scope_entry_depth.is_some() {
                continue;
            }
            if trimmed.starts_with("//") {
                continue;
            }

            if error_re.is_match(raw_line) {
                let prev = lines.get(line_idx.wrapping_sub(1)).copied().unwrap_or("");
                let kind = parse_log_policy(prev);

                match kind {
                    None => {
                        if is_structurally_excluded(&file_rel, &lines, line_idx) {
                            // Structural exclusion (e.g. camel-log LogProducer).
                        } else if should_report(&lines, line_idx, raw_line, &file_rel, &allowlist) {
                            violations.push(Violation {
                                file: path.to_string_lossy().to_string(),
                                line: line_idx + 1,
                                snippet: format!(
                                    "{}  (missing // log-policy: annotation — see ADR-0012)",
                                    raw_line.trim()
                                ),
                            });
                        }
                    }
                    Some(LogPolicyKind::HandlerOwned) => {
                        if should_report(&lines, line_idx, raw_line, &file_rel, &allowlist) {
                            violations.push(Violation {
                                file: path.to_string_lossy().to_string(),
                                line: line_idx + 1,
                                snippet: format!(
                                    "{}  (handler-owned must be warn!/debug!, not error!)",
                                    raw_line.trim()
                                ),
                            });
                        }
                    }
                    Some(LogPolicyKind::Unknown(s)) => {
                        if should_report(&lines, line_idx, raw_line, &file_rel, &allowlist) {
                            violations.push(Violation {
                                file: path.to_string_lossy().to_string(),
                                line: line_idx + 1,
                                snippet: format!(
                                    "{}  (unknown log-policy '{}' — must be system-broken | outside-contract | handler-owned)",
                                    raw_line.trim(),
                                    s
                                ),
                            });
                        }
                    }
                    Some(LogPolicyKind::SystemBroken) => {
                        // No further requirement.
                    }
                    Some(LogPolicyKind::OutsideContract) => {
                        if !has_replacement_signal(&lines, line_idx) {
                            if should_report(&lines, line_idx, raw_line, &file_rel, &allowlist) {
                                violations.push(Violation {
                                    file: path.to_string_lossy().to_string(),
                                    line: line_idx + 1,
                                    snippet: format!(
                                        "{}  (outside-contract requires increment_errors OR force_unhealthy_for_route OR if !bridged {{}} guard — see ADR-0012)",
                                        raw_line.trim()
                                    ),
                                });
                            }
                        } else {
                            // Validate labels on any increment_errors call in
                            // the same function — only when triggered by an
                            // outside-contract annotation. Labels elsewhere
                            // (e.g. legacy test helpers) are not validated.
                            if let Some(mut lv) = check_labels_in_function(&lines, line_idx) {
                                lv.file = path.to_string_lossy().to_string();
                                if should_report(&lines, line_idx, raw_line, &file_rel, &allowlist)
                                {
                                    violations.push(lv);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    violations.extend(inline_marker_violations);
    Ok(violations)
}

/// A secret leakage violation.
#[derive(Debug, PartialEq)]
pub struct SecretViolation {
    pub file: String,
    pub line: usize,
    pub snippet: String,
    pub rule: String,
}

/// Patterns that indicate potential secret leakage.
/// Each entry: (regex pattern, human-readable rule name).
///
/// Key design choices:
/// - `(?i)` case-insensitive matching.
/// - `[^;]{0,300}?` instead of `.*` to (a) match across newlines (`;` terminates
///   a macro call in practice), and (b) limit backtracking.
/// - Three categories: format macros, tracing structured fields (name = value),
///   and tracing shorthand fields (%field, ?field).
const SECRET_PATTERNS: &[(&str, &str)] = &[
    // format!/write!/println!/eprintln! with a sensitive field name — multiline-aware
    (
        r"(?i)(format|println|eprintln|print|writeln|write)!\s*\([^;]{0,300}?\b(password|secret|token|credential|api_key|auth_token|access_token|refresh_token|client_secret|private_key|bearer_token)\b",
        "sensitive field name in format macro",
    ),
    // tracing macros with sensitive structured field (name = value) — multiline-aware
    (
        r"(?i)(warn|error|info|debug|trace)!\s*\([^;]{0,300}?\b(password|secret|token|credential|api_key|auth_token|access_token|refresh_token|client_secret|private_key|bearer_token)\s*[=%?]",
        "sensitive structured field in tracing macro",
    ),
    // tracing shorthand fields: info!(%auth_token), info!(?password), info!(token)
    (
        r"(?i)(warn|error|info|debug|trace)!\s*\([^;]{0,300}?[%?]\s*(password|secret|token|credential|api_key|auth_token|access_token|refresh_token|client_secret|private_key|bearer_token)\b",
        "sensitive shorthand field in tracing macro",
    ),
    // tracing bare fields: info!(password, ...) or warn!(token, ...)
    // Overlap with patterns 2-3 is resolved by deduplication in the scanner.
    (
        r"(?i)(warn|error|info|debug|trace)!\s*\([^;]{0,300}?\b(password|secret|token|credential|api_key|auth_token|access_token|refresh_token|client_secret|private_key|bearer_token)\s*,",
        "sensitive bare field in tracing macro",
    ),
];

/// Scan all workspace `src/**/*.rs` files for potential secret leakage patterns.
///
/// Uses whole-file regex scanning (not per-line) so multiline macro calls like:
///   format!(
///       "password={}",
///       self.password
///   )
/// are correctly detected. Match positions are mapped back to line numbers.
pub fn lint_secrets(workspace_root: &Path) -> Result<Vec<SecretViolation>, String> {
    use regex::Regex;
    use std::path::Component;
    use walkdir::WalkDir;

    let compiled: Vec<(Regex, &str)> = SECRET_PATTERNS
        .iter()
        .map(|(pat, rule)| {
            Regex::new(pat)
                .map(|re| (re, *rule))
                .map_err(|e| format!("Invalid secret pattern '{pat}': {e}")) // allow-secret
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut violations = Vec::new();

    for entry in WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        // Only scan files under a src/ directory (portable)
        if !path
            .components()
            .any(|c| c == Component::Normal("src".as_ref()))
        {
            continue;
        }
        if path.components().any(|c| {
            c == Component::Normal("target".as_ref())
                || c == Component::Normal(".worktrees".as_ref())
        }) {
            continue;
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;

        // Build a table of line-start byte offsets for O(log n) line lookup.
        let line_starts: Vec<usize> = std::iter::once(0)
            .chain(content.match_indices('\n').map(|(i, _)| i + 1))
            .collect();

        // Maps a byte offset to a 1-based line number.
        let byte_to_line =
            |offset: usize| -> usize { line_starts.partition_point(|&s| s <= offset) };

        for (re, rule) in &compiled {
            let mut search_from = 0;
            while let Some(m) = re.find_at(&content, search_from) {
                let line_num = byte_to_line(m.start());
                let line_start = line_starts[line_num - 1];
                let line_end = content[line_start..]
                    .find('\n')
                    .map(|i| line_start + i)
                    .unwrap_or(content.len());
                let first_line = &content[line_start..line_end];

                // Also check the previous line for the escape hatch
                // (cargo fmt may reflow trailing comments onto other lines).
                let prev_line = if line_num > 1 {
                    let prev_start = line_starts[line_num - 2];
                    let prev_end = line_starts[line_num - 1].saturating_sub(1);
                    &content[prev_start..prev_end]
                } else {
                    ""
                };

                // Skip comment-only lines and lines (or their preceding line)
                // carrying the `// allow-secret` escape hatch.
                if !first_line.trim().starts_with("//")
                    && !first_line.contains("// allow-secret")
                    && !prev_line.contains("// allow-secret")
                {
                    violations.push(SecretViolation {
                        file: path.to_string_lossy().to_string(),
                        line: line_num,
                        snippet: first_line.to_string(),
                        rule: rule.to_string(),
                    });
                }

                // Advance past this match; guard against zero-length matches.
                search_from = m.end().max(m.start() + 1);
            }
        }
    }

    // Deduplicate violations by (file, line) — multiple patterns may match the
    // same line (e.g. structured field + bare field). Keep the first match.
    let mut seen = std::collections::HashSet::new();
    violations.retain(|v| seen.insert((v.file.clone(), v.line)));

    // Layer 2: credential-derive lint (ADR-0051). Walks `crates/**/src/**/*.rs`,
    // parses with `syn`, and flags structs/enums that violate the
    // classification/derive contract (e.g. manual-redaction + Debug).
    // Parse failures hard-fail: the caller exits non-zero on Err.
    violations.extend(lint_credential_derives(workspace_root)?);

    Ok(violations)
}

/// ADR-0051 credential-boundary classification for a struct or enum.
///
/// Declared via a rustdoc marker:
///
/// ```text
/// /// ADR-0051 credential boundary: <classification>
/// ```
///
/// where `<classification>` is one of the closed-vocabulary values below.
/// See `openspec/specs/credential-lint/spec.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Classification {
    /// Secret-bearing type. `Debug` and `Serialize` must be hand-written to
    /// redact; deriving either is forbidden.
    ManualRedaction,
    /// Type whose derived `Debug` already redacts (e.g. a newtype wrapper).
    /// `Debug` is safe; `Serialize` is forbidden.
    RedactingWrapper,
    /// Wire/protocol data-transfer object. `Serialize` is safe; `Debug` is
    /// forbidden.
    ProtocolDto,
}

impl Classification {
    fn from_keyword(kw: &str) -> Option<Classification> {
        match kw {
            "manual-redaction" => Some(Classification::ManualRedaction),
            "redacting-wrapper" => Some(Classification::RedactingWrapper),
            "protocol-dto" => Some(Classification::ProtocolDto),
            _ => None,
        }
    }
}

/// Prefix used by every credential-derive violation rule so they group cleanly
/// in reports and are easy to assert on.
const CREDENTIAL_RULE_PREFIX: &str = "credential-derive:";

/// The rustdoc marker that declares a credential-boundary classification.
const CREDENTIAL_BOUNDARY_MARKER: &str = "ADR-0051 credential boundary:";

/// Read the string literal from a `#[doc = "..."]` name-value meta, if any.
fn doc_literal(expr: &syn::Expr) -> Option<String> {
    if let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = expr
    {
        Some(s.value())
    } else {
        None
    }
}

/// Scan a type's doc-comment attributes for the `ADR-0051 credential boundary:`
/// marker and return the declared classification.
///
/// - `Ok(None)`: no marker present (classification is opt-in).
/// - `Ok(Some(cls))`: a single, well-formed, closed-vocabulary value.
/// - `Err(msg)`: missing value (malformed), an unknown value, or conflicting
///   duplicate markers. The caller reports `msg` as a violation.
fn parse_classification(attrs: &[syn::Attribute]) -> Result<Option<Classification>, String> {
    let mut values: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        let syn::Meta::NameValue(nv) = &attr.meta else {
            continue;
        };
        let Some(doc) = doc_literal(&nv.value) else {
            continue;
        };
        if let Some(rest) = doc.split(CREDENTIAL_BOUNDARY_MARKER).nth(1) {
            // The classification keyword runs to the end of the marker's line
            // (block doc-comments may place trailing prose on later lines).
            let value = rest.lines().next().unwrap_or("").trim();
            values.push(value.to_string());
        }
    }

    if values.is_empty() {
        return Ok(None);
    }

    // Any marker with no value after the colon is malformed.
    if values.iter().any(String::is_empty) {
        return Err("malformed classification (missing value)".to_string());
    }

    // Conflicting duplicates: more than one distinct value across markers.
    let mut distinct: Vec<&String> = Vec::new();
    for v in &values {
        if !distinct.contains(&v) {
            distinct.push(v);
        }
    }
    if distinct.len() > 1 {
        return Err("conflicting duplicate classifications".to_string());
    }

    let value = distinct[0].as_str();
    match Classification::from_keyword(value) {
        Some(cls) => Ok(Some(cls)),
        None => Err(format!("unknown classification '{value}'")),
    }
}

/// True when `ty` is (or ends in) a `Zeroizing` path segment, matching both
/// `Zeroizing<T>` and `zeroize::Zeroizing<T>`.
fn type_is_zeroizing(ty: &syn::Type) -> bool {
    matches!(
        ty,
        syn::Type::Path(tp)
            if tp.path.segments.iter().any(|seg| seg.ident == "Zeroizing")
    )
}

/// True when a struct or enum has at least one field whose type is `Zeroizing`.
fn has_zeroizing_field(item: &syn::Item) -> bool {
    match item {
        syn::Item::Struct(s) => s.fields.iter().any(|f| type_is_zeroizing(&f.ty)),
        syn::Item::Enum(e) => e
            .variants
            .iter()
            .any(|v| v.fields.iter().any(|f| type_is_zeroizing(&f.ty))),
        _ => false,
    }
}

/// Extract every trait name mentioned in `#[derive(...)]` attributes, across
/// single-line and multi-line derives. Uses the last path segment so qualified
/// forms like `#[derive(serde::Serialize)]` are detected, not just
/// `#[derive(Serialize)]`.
fn extract_derive_names(attrs: &[syn::Attribute]) -> Vec<String> {
    let mut names = Vec::new();
    for attr in attrs {
        // Skip non-derive attributes so e.g. `#[serde(rename = "x")]` does
        // not pollute the derive list with `rename`.
        if !attr.path().is_ident("derive") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if let Some(last) = meta.path.segments.last() {
                names.push(last.ident.to_string());
            }
            Ok(())
        });
    }
    names
}

/// True when `attrs` contains `#[cfg(test)]` (also `#[cfg(all(test, ...))]`).
fn has_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        let Ok(list) = attr.meta.require_list() else {
            return false;
        };
        let tokens = list.tokens.to_string();
        // Match the standalone identifier `test` (not e.g. `testing`).
        tokens == "test"
            || tokens
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|tok| tok == "test")
    })
}

/// True when `byte` may continue a Rust identifier (alnum or `_`).
fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// True when `line` contains the `keyword` token (`struct`/`enum`) immediately
/// followed by the `ident` token, with proper word boundaries (so `mystruct`
/// does not match `struct`).
fn line_has_decl(line: &str, keyword: &str, ident: &str) -> bool {
    let bytes = line.as_bytes();
    let mut search = 0usize;
    while let Some(rel) = line[search..].find(keyword) {
        let abs = search + rel;
        let before_ok = abs == 0 || !is_ident_byte(bytes[abs - 1]);
        let rest = &line[abs + keyword.len()..];
        if before_ok && rest.starts_with(|c: char| c.is_whitespace()) {
            let next = rest.trim_start();
            if next.starts_with(ident)
                && next[ident.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
            {
                return true;
            }
        }
        search = abs + 1;
    }
    false
}

/// Line number of a struct/enum item's declaration in `src`. Because
/// `proc_macro2::Span` does not expose line numbers without the unstable
/// semver-exempt cfg, the declaration is located lexically by matching the
/// `struct`/`enum` keyword followed by the item's identifier, skipping comment
/// lines. Returns `None` if the declaration cannot be located; callers skip
/// the item rather than misreport line 1.
fn declaration_line(src: &str, item: &syn::Item) -> Option<usize> {
    let (keyword, ident): (&'static str, String) = match item {
        syn::Item::Struct(s) => ("struct", s.ident.to_string()),
        syn::Item::Enum(e) => ("enum", e.ident.to_string()),
        _ => return None,
    };
    for (i, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        if line_has_decl(line, keyword, &ident) {
            return Some(i + 1);
        }
    }
    None
}

/// True when a struct/enum item itself carries `#[cfg(test)]`.
fn item_has_cfg_test(item: &syn::Item) -> bool {
    match item {
        syn::Item::Struct(s) => has_cfg_test(&s.attrs),
        syn::Item::Enum(e) => has_cfg_test(&e.attrs),
        _ => false,
    }
}

/// Push a single credential-derive violation built from the declaration line.
fn push_credential_violation(
    violations: &mut Vec<SecretViolation>,
    file_path: &str,
    line: usize,
    src: &str,
    rule_tail: &str,
) {
    let snippet = src
        .lines()
        .nth(line.saturating_sub(1))
        .unwrap_or("")
        .trim()
        .to_string();
    violations.push(SecretViolation {
        file: file_path.to_string(),
        line,
        snippet,
        rule: format!("{CREDENTIAL_RULE_PREFIX} {rule_tail}"),
    });
}

/// Check one struct/enum item for credential-derive consistency, appending any
/// violations to `violations`.
fn check_credential_item(
    item: &syn::Item,
    violations: &mut Vec<SecretViolation>,
    file_path: &str,
    src: &str,
) {
    let attrs: &[syn::Attribute] = match item {
        syn::Item::Struct(s) => &s.attrs,
        syn::Item::Enum(e) => &e.attrs,
        _ => return,
    };
    // If the lexical matcher fails, report with line 0 rather than
    // silently skip — a credential lint's dangerous failure mode is
    // the false negative.
    let line = declaration_line(src, item).unwrap_or(0);

    let derives = extract_derive_names(attrs);
    match parse_classification(attrs) {
        Ok(Some(cls)) => {
            let has_debug = derives.iter().any(|d| d == "Debug");
            let has_serialize = derives.iter().any(|d| d == "Serialize");
            let forbidden: Vec<&str> = match cls {
                Classification::ManualRedaction => {
                    let mut v = Vec::new();
                    if has_debug {
                        v.push("manual-redaction forbids Debug");
                    }
                    if has_serialize {
                        v.push("manual-redaction forbids Serialize");
                    }
                    v
                }
                Classification::RedactingWrapper => {
                    if has_serialize {
                        vec!["redacting-wrapper forbids Serialize"]
                    } else {
                        Vec::new()
                    }
                }
                Classification::ProtocolDto => {
                    if has_debug {
                        vec!["protocol-dto forbids Debug"]
                    } else {
                        Vec::new()
                    }
                }
            };
            for tail in forbidden {
                push_credential_violation(violations, file_path, line, src, tail);
            }
        }
        Ok(None) => {
            if has_zeroizing_field(item) {
                push_credential_violation(
                    violations,
                    file_path,
                    line,
                    src,
                    "Zeroizing field requires manual-redaction classification",
                );
            }
        }
        Err(msg) => {
            push_credential_violation(violations, file_path, line, src, &msg);
        }
    }
}

/// Recursively visit items, skipping `#[cfg(test)]` modules and `#[cfg(test)]`
/// items, checking each struct/enum for credential-derive consistency.
fn collect_credential_violations(
    items: &[syn::Item],
    violations: &mut Vec<SecretViolation>,
    file_path: &str,
    src: &str,
) {
    for item in items {
        match item {
            syn::Item::Mod(m) => {
                if has_cfg_test(&m.attrs) {
                    continue;
                }
                if let Some((_, inner)) = &m.content {
                    collect_credential_violations(inner, violations, file_path, src);
                }
            }
            syn::Item::Struct(_) | syn::Item::Enum(_) => {
                if item_has_cfg_test(item) {
                    continue;
                }
                check_credential_item(item, violations, file_path, src);
            }
            _ => {}
        }
    }
}

/// Core scanner: parse a single `.rs` source file with `syn` and return every
/// credential-derive violation. On `syn::parse_file` failure, returns `Err`
/// (hard-fail — parse failures must not silently skip enforcement).
fn lint_credential_derives_src(src: &str, file_path: &str) -> Result<Vec<SecretViolation>, String> {
    let file = syn::parse_file(src).map_err(|e| format!("{file_path}: {e}"))?;
    let mut violations = Vec::new();
    collect_credential_violations(&file.items, &mut violations, file_path, src);
    Ok(violations)
}

/// Walk `crates/**/src/**/*.rs` under `workspace_root` and collect every
/// credential-derive violation. Test files (`is_test_file`) are skipped at the
/// file level; `#[cfg(test)]` modules are skipped at the item level. Any parse
/// failure returns `Err`.
pub fn lint_credential_derives(workspace_root: &Path) -> Result<Vec<SecretViolation>, String> {
    use std::path::Component;
    use walkdir::WalkDir;

    let mut violations = Vec::new();

    for entry in WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if is_test_file(path) {
            continue;
        }

        let comps: Vec<_> = path.components().collect();
        let under_crates = comps
            .iter()
            .any(|c| *c == Component::Normal("crates".as_ref()));
        let under_src = comps
            .iter()
            .any(|c| *c == Component::Normal("src".as_ref()));
        let blocked = comps
            .iter()
            .any(|c| *c == Component::Normal("target".as_ref()))
            || path.starts_with(workspace_root.join(".worktrees"));
        if !under_crates || !under_src || blocked {
            continue;
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        violations.extend(lint_credential_derives_src(
            &content,
            &path.to_string_lossy(),
        )?);
    }

    Ok(violations)
}

/// Represents a workspace crate with its publish-relevant metadata.
#[derive(Clone, Debug)]
struct WorkspaceCrate {
    name: String,
    path: String,
    normal_deps: Vec<String>,
    /// Dev and build dependencies (also target-specific variants) that cargo
    /// embeds in the published Cargo.toml. `cargo publish` resolves these
    /// against the registry during package verification, so they participate
    /// in the topological publish order — but they can be broken when they
    /// form a cycle (the cycle member would need to be published first with
    /// `cargo publish --no-verify`, or the dev-dep restructured).
    weak_deps: Vec<String>,
    publish: bool,
}

/// Edge kind in the publish-order graph. `Normal` edges come from
/// `[dependencies]` (and target-specific variants); they are hard constraints
/// that must be satisfied before the dependent can be published. `Weak`
/// edges come from `[dev-dependencies]` and `[build-dependencies]`; cargo
/// still resolves them during `cargo publish`, but cycles closed only by
/// weak edges can be broken by publishing one member first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EdgeKind {
    Normal,
    Weak,
}

/// Discover workspace crates and compute the topological publish order with
/// SCC-accurate cycle detection. Returns `(sorted_crates, no_verify_holders,
/// broken_weak_edges)` where each broken edge is a `(holder, target)` pair and
/// `holder` is the crate that DECLARES the dev/build-dependency. Data-only —
/// callers format any human-readable report.
type PublishOrderResult = (
    Vec<WorkspaceCrate>,
    std::collections::HashSet<String>,
    Vec<(String, String)>,
);

fn resolve_publish_order(workspace_root: &Path) -> Result<PublishOrderResult, String> {
    let crates = discover_publishable_crates(workspace_root)?;
    compute_publish_order(crates)
}

/// Walk `crates/*/Cargo.toml` (nested one level) into `WorkspaceCrate` records.
/// Pure disk walk — no graph logic — so the topological solver is unit-testable
/// from in-memory `WorkspaceCrate` vecs.
fn discover_publishable_crates(workspace_root: &Path) -> Result<Vec<WorkspaceCrate>, String> {
    let mut crates: Vec<WorkspaceCrate> = Vec::new();

    let crates_dir = workspace_root.join("crates");
    for entry in walkdir::WalkDir::new(&crates_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.file_name() != Some(std::ffi::OsStr::new("Cargo.toml")) {
            continue;
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

        let name =
            extract_toml_name(&content).ok_or_else(|| format!("No name in {}", path.display()))?;

        if !name.starts_with("camel-") {
            continue;
        }

        let publish = !content.contains("publish = false");
        let (normal_deps, weak_deps) = extract_camel_deps_grouped(&content);
        let crate_dir = path
            .parent()
            .ok_or_else(|| format!("Cargo.toml has no parent directory: {}", path.display()))?;
        let rel_path = crate_dir
            .strip_prefix(workspace_root)
            .map_err(|e| {
                format!(
                    "Failed to make {} relative to workspace root: {e}",
                    crate_dir.display()
                )
            })?
            .to_string_lossy()
            .to_string();

        crates.push(WorkspaceCrate {
            name,
            path: rel_path,
            normal_deps,
            weak_deps,
            publish,
        });
    }

    Ok(crates)
}

/// Topological publish-order solver with SCC-gated weak-edge breaking.
///
/// Runs Kahn's algorithm; when the ready queue drains with unscheduled crates
/// remaining, computes Tarjan SCC over the subgraph induced by the unscheduled
/// nodes and breaks the single globally-lexicographically-smallest intra-SCC
/// weak edge (holder = declaring crate), then re-runs Kahn and recomputes the
/// SCC decomposition. This per-iteration recompute is what makes the result
/// cycle-accurate instead of the previous greedy over-breaking behaviour: a
/// weak edge is broken only if both its endpoints still lie inside the same
/// non-trivial SCC. When unscheduled nodes remain but no non-trivial SCC has a
/// breakable intra-SCC weak edge, a hard normal-only cycle is reported.
fn compute_publish_order(crates: Vec<WorkspaceCrate>) -> Result<PublishOrderResult, String> {
    let name_map: std::collections::HashMap<String, usize> = crates
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.clone(), i))
        .collect();

    let publishable: Vec<usize> = crates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.publish)
        .map(|(i, _)| i)
        .collect();

    // Build adjacency with edge-kind tagging. `adj[di]` lists the dependents
    // of `di` — i.e. for each entry `(ci, kind)`, `ci` declares a dependency
    // on `di`. We track the kind so we can break weak-only cycles after Kahn
    // stalls.
    let mut adj: Vec<Vec<(usize, EdgeKind)>> = vec![Vec::new(); crates.len()];
    let mut in_degree: Vec<usize> = vec![0; crates.len()];

    for &ci in &publishable {
        let mut seen_normal: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for dep_name in &crates[ci].normal_deps {
            // Self-references (e.g. `camel-foo = { path = ".", features = ["test-util"] }`
            // in [dev-dependencies] to enable a test-only feature) are a
            // standard Rust pattern. cargo resolves them to the crate itself
            // at publish time, so they do not participate in publish order.
            if dep_name == &crates[ci].name {
                continue;
            }
            if let Some(&di) = name_map.get(dep_name)
                && crates[di].publish
                && seen_normal.insert(di)
            {
                in_degree[ci] += 1;
                adj[di].push((ci, EdgeKind::Normal));
            }
        }
        // Weak edges: count toward in-degree, but mark them so we can break
        // them later if they participate in a cycle.
        let mut seen_weak: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for dep_name in &crates[ci].weak_deps {
            if dep_name == &crates[ci].name {
                continue;
            }
            if let Some(&di) = name_map.get(dep_name)
                && crates[di].publish
                && !seen_normal.contains(&di)
                && seen_weak.insert(di)
            {
                in_degree[ci] += 1;
                adj[di].push((ci, EdgeKind::Weak));
            }
        }
    }

    let mut queue: std::collections::VecDeque<usize> = publishable
        .iter()
        .filter(|&&i| in_degree[i] == 0)
        .copied()
        .collect();

    let mut sorted: Vec<usize> = Vec::new();
    // Each broken weak edge is `(holder, target)`: holder = the declaring
    // crate (the one whose dev/build-dep got cut, i.e. must publish with
    // --no-verify), target = the dependency the holder declared.
    let mut broken_weak_edges: Vec<(String, String)> = Vec::new();

    loop {
        // Drain the ready queue.
        while let Some(ci) = queue.pop_front() {
            sorted.push(ci);
            for &(dependent, _kind) in &adj[ci] {
                in_degree[dependent] -= 1;
                if in_degree[dependent] == 0 {
                    queue.push_back(dependent);
                }
            }
        }

        if sorted.len() == publishable.len() {
            break;
        }

        // Kahn stalled with unscheduled crates remaining. Induce the subgraph
        // on the unscheduled set and decompose it into SCCs.
        let sorted_set: std::collections::HashSet<usize> = sorted.iter().copied().collect();
        let sub_adj: Vec<Vec<(usize, EdgeKind)>> = (0..crates.len())
            .map(|i| {
                if sorted_set.contains(&i) {
                    Vec::new()
                } else {
                    adj[i]
                        .iter()
                        .filter(|(d, _)| !sorted_set.contains(d))
                        .copied()
                        .collect()
                }
            })
            .collect();
        let sccs = tarjan_scc(&sub_adj);

        // Non-trivial SCCs: size > 1, or size-1 carrying a self-loop.
        let nontrivial: Vec<&Vec<usize>> = sccs
            .iter()
            .filter(|scc| {
                if scc.len() > 1 {
                    return true;
                }
                let n = scc[0];
                adj[n].iter().any(|&(d, _)| d == n)
            })
            .collect();

        if nontrivial.is_empty() {
            // Unreachable in correct code: a Kahn stall with unscheduled
            // nodes implies a cycle, which implies a non-trivial SCC, so
            // `nontrivial` cannot be empty here. Kept as a fail-closed guard
            // against an in-degree bookkeeping regression.
            return Err("Cannot compute publish order due to dependency cycles".to_string());
        }

        // Pick the single globally-lexicographically-smallest breakable
        // intra-SCC weak edge across all non-trivial SCCs.
        let name_of = |i: usize| crates[i].name.clone();
        let mut chosen: Option<(usize, usize)> = None;
        let mut chosen_key: Option<(String, String)> = None;
        for scc in &nontrivial {
            let Some((holder_idx, target_idx)) = find_intra_scc_weak_edge(&adj, scc, name_of)
            else {
                continue;
            };
            let key = (
                crates[holder_idx].name.clone(),
                crates[target_idx].name.clone(),
            );
            if chosen_key.as_ref().is_none_or(|ck| &key < ck) {
                chosen = Some((holder_idx, target_idx));
                chosen_key = Some(key);
            }
        }

        let Some((holder_idx, target_idx)) = chosen else {
            // Non-trivial SCCs exist but none has a breakable intra-SCC weak
            // edge — a hard cycle closed only by normal edges.
            return Err("Cannot compute publish order due to dependency cycles".to_string());
        };

        // Break the edge. Recall `adj[target_idx]` holds `(holder_idx, Weak)`.
        adj[target_idx].retain(|&(d, k)| !(d == holder_idx && k == EdgeKind::Weak));
        in_degree[holder_idx] -= 1;
        broken_weak_edges.push((
            crates[holder_idx].name.clone(),
            crates[target_idx].name.clone(),
        ));
        if in_degree[holder_idx] == 0 {
            queue.push_back(holder_idx);
        }
        // Loop: re-drain Kahn, then recompute SCC if it stalls again.
    }

    let no_verify: std::collections::HashSet<String> =
        broken_weak_edges.iter().map(|(h, _)| h.clone()).collect();

    Ok((
        sorted.into_iter().map(|i| crates[i].clone()).collect(),
        no_verify,
        broken_weak_edges,
    ))
}

/// Iterative Tarjan strongly-connected-components algorithm over a graph whose
/// edges carry an `EdgeKind` — both kinds are traversable for cycle detection.
/// Returns every SCC as a vec of node indices; trivial singletons are included
/// (callers filter). Iterative rather than recursive so the workspace cannot
/// stack-overflow as the publish graph grows.
fn tarjan_scc(adj: &[Vec<(usize, EdgeKind)>]) -> Vec<Vec<usize>> {
    let n = adj.len();
    let mut index_counter: usize = 0;
    let mut stack: Vec<usize> = Vec::with_capacity(n);
    let mut on_stack = vec![false; n];
    let mut index: Vec<usize> = vec![usize::MAX; n];
    let mut lowlink: Vec<usize> = vec![0; n];
    let mut result: Vec<Vec<usize>> = Vec::new();
    // Explicit DFS work stack of `(node, next-child-cursor)` frames.
    let mut work: Vec<(usize, usize)> = Vec::with_capacity(n);

    for start in 0..n {
        if index[start] != usize::MAX {
            continue;
        }
        index[start] = index_counter;
        lowlink[start] = index_counter;
        index_counter += 1;
        stack.push(start);
        on_stack[start] = true;
        work.push((start, 0));

        while let Some((v, mut i)) = work.pop() {
            let neighbors_len = adj[v].len();
            let mut recursed = false;
            while i < neighbors_len {
                let w = adj[v][i].0;
                if index[w] == usize::MAX {
                    // Recurse into w: save v's resume point, then push w.
                    work.push((v, i + 1));
                    index[w] = index_counter;
                    lowlink[w] = index_counter;
                    index_counter += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                    recursed = true;
                    break;
                } else if on_stack[w] {
                    lowlink[v] = lowlink[v].min(index[w]);
                    i += 1;
                } else {
                    i += 1;
                }
            }
            if recursed {
                continue;
            }
            if lowlink[v] == index[v] {
                // v roots an SCC; pop the stack down to v.
                let mut comp = Vec::new();
                let mut found_root = false;
                while !found_root {
                    match stack.pop() {
                        Some(w) => {
                            on_stack[w] = false;
                            comp.push(w);
                            if w == v {
                                found_root = true;
                            }
                        }
                        None => break,
                    }
                }
                result.push(comp);
            }
            // Propagate lowlink to the parent frame now on top of `work`.
            if let Some(&(parent, _)) = work.last() {
                lowlink[parent] = lowlink[parent].min(lowlink[v]);
            }
        }
    }
    result
}

/// Find the lexicographically-smallest breakable weak edge whose both endpoints
/// lie in `scc`. Edge semantics: `adj[target_idx]` contains `(holder_idx,
/// Weak)` where `holder_idx` is the crate that DECLARES the dev/build-dep and
/// `target_idx` is the dependency target. Returns `(holder_idx, target_idx)`
/// ranked smallest by `(holder_name, target_name)`, or `None` if no such edge
/// exists in this SCC.
fn find_intra_scc_weak_edge(
    adj: &[Vec<(usize, EdgeKind)>],
    scc: &[usize],
    name_of: impl Fn(usize) -> String,
) -> Option<(usize, usize)> {
    let in_scc: std::collections::HashSet<usize> = scc.iter().copied().collect();
    let mut best: Option<(usize, usize)> = None;
    let mut best_key: Option<(String, String)> = None;
    for &target_idx in scc {
        for &(holder_idx, kind) in &adj[target_idx] {
            if kind != EdgeKind::Weak || !in_scc.contains(&holder_idx) {
                continue;
            }
            let key = (name_of(holder_idx), name_of(target_idx));
            if best_key.as_ref().is_none_or(|bk| &key < bk) {
                best = Some((holder_idx, target_idx));
                best_key = Some(key);
            }
        }
    }
    best
}

/// Format the broken-weak-edge report shared by the `publish-order` and
/// `publish` commands. `broken` entries are `(holder, target)`; the holder is
/// the crate that declared the cut dev/build-dep (it must publish with
/// `--no-verify`). Returns an empty string when there is nothing to report.
fn format_cycle_report(
    no_verify: &std::collections::HashSet<String>,
    broken: &[(String, String)],
) -> String {
    if broken.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "⚠️  {n} crate(s) in publish cycles; broke {m} weak (dev/build) edge(s):\n",
        n = no_verify.len(),
        m = broken.len()
    ));
    for (holder, target) in broken {
        out.push_str(&format!(
            "  {holder} --dev/build-dep--> {target} (cycle participant — remediate before publish)\n"
        ));
    }
    out
}

/// Fail-closed guard: if any crates are in dev/build-dep cycles, return an
/// error with the cycle report. Callers may wrap the error with additional
/// context.
fn check_no_publish_cycles(
    no_verify: &std::collections::HashSet<String>,
    broken: &[(String, String)],
) -> Result<(), String> {
    if !no_verify.is_empty() {
        Err(format_cycle_report(no_verify, broken))
    } else {
        Ok(())
    }
}

fn publish_order(workspace_root: &Path, shell: bool) -> Result<(), String> {
    let (sorted, no_verify, broken) = resolve_publish_order(workspace_root)?;

    if shell {
        for c in &sorted {
            println!("publish_crate \"{}\" \"{}\"", c.name, c.path);
        }
    } else {
        println!("Publish order ({} crates):", sorted.len());
        println!();
        for (i, c) in sorted.iter().enumerate() {
            if c.normal_deps.is_empty() {
                println!("{:3}. {:<42} (no deps)", i + 1, c.name);
            } else {
                let deps_str = c.normal_deps.join(", ");
                println!("{:3}. {:<42} ← {}", i + 1, c.name, deps_str);
            }
        }
        let skipped: Vec<&WorkspaceCrate> = sorted.iter().filter(|c| !c.publish).collect();
        if !skipped.is_empty() {
            println!();
            println!("Skipped (publish = false):");
            for c in &skipped {
                println!("  - {}", c.name);
            }
        }
        // The cycle report lives here (data-only resolver; callers format).
        let report = format_cycle_report(&no_verify, &broken);
        if !report.is_empty() {
            println!();
            print!("{report}");
        }
    }

    Ok(())
}

/// Print the `no_verify` set and broken weak edges from the SCC-accurate
/// cycle detector. Performs NO `cargo publish` and writes NO `Cargo.toml`.
fn show_cycles(workspace_root: &Path, w: &mut impl std::io::Write) -> Result<(), String> {
    let (sorted, no_verify, broken) = resolve_publish_order(workspace_root)?;
    let _ = sorted; // unused in diagnostic mode

    writeln!(w, "no_verify set: {} crate(s)", no_verify.len())
        .map_err(|e| format!("write failed: {e}"))?;
    if !no_verify.is_empty() {
        let mut holders: Vec<&String> = no_verify.iter().collect();
        holders.sort();
        for h in &holders {
            writeln!(w, "  {h}").map_err(|e| format!("write failed: {e}"))?;
        }
    }

    writeln!(w, "broken weak edges:").map_err(|e| format!("write failed: {e}"))?;
    if broken.is_empty() {
        writeln!(w).map_err(|e| format!("write failed: {e}"))?;
        return Ok(());
    }
    for (holder, target) in &broken {
        writeln!(w, "  {holder} --dev/build-dep--> {target}")
            .map_err(|e| format!("write failed: {e}"))?;
    }
    Ok(())
}

/// Enforce publish-topology invariants:
/// 1. The `no_verify` set (SCC cycle participants) must be empty.
/// 2. No publishable crate may declare `camel-test` as any kind of dependency
///    (`camel-test` is the publish-order leaf sink — ADR-0055).
fn lint_publish_cycles(workspace_root: &Path) -> Result<(), String> {
    let (_, no_verify, broken) = resolve_publish_order(workspace_root)?;
    if !no_verify.is_empty() {
        return Err(format_cycle_report(&no_verify, &broken));
    }

    let crates_dir = workspace_root.join("crates");
    for entry in walkdir::WalkDir::new(&crates_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.file_name() != Some(std::ffi::OsStr::new("Cargo.toml")) {
            continue;
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

        if !is_publishable_crate(&content) {
            continue;
        }

        if let Some(kind) = find_camel_test_dep_kind(&content) {
            let crate_name = extract_toml_name(&content)
                .ok_or_else(|| format!("No name in {}", path.display()))?;
            return Err(format!(
                "publishable crate {crate_name} depends on camel-test ({kind})"
            ));
        }
    }

    Ok(())
}

/// Returns true unless `publish = false` is set in `[package]`.
fn is_publishable_crate(content: &str) -> bool {
    let mut in_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "[package]" {
            in_package = true;
            continue;
        }
        if trimmed.starts_with('[') {
            if in_package {
                break;
            }
            continue;
        }
        if in_package && trimmed == "publish = false" {
            return false;
        }
    }
    true
}

/// Scan TOML content for a `camel-test` line under a dependency section.
/// Returns the section kind ("dependencies", "dev-dependencies", or
/// "build-dependencies") on first match, or `None` if `camel-test` is not
/// declared as a dependency.
fn find_camel_test_dep_kind(content: &str) -> Option<&'static str> {
    let mut section = "";
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') {
            section = trimmed;
            continue;
        }
        let Some(dep) = extract_camel_dep_name(trimmed) else {
            continue;
        };
        if dep != "camel-test" {
            continue;
        }
        if is_weak_dependency_section(section) {
            if section.contains("build-dependencies") {
                return Some("build-dependencies");
            } else {
                return Some("dev-dependencies");
            }
        } else if is_dependency_section(section) {
            return Some("dependencies");
        }
    }
    None
}

/// Get workspace version from root Cargo.toml.
fn workspace_version(workspace_root: &Path) -> Result<String, String> {
    let cargo_toml = std::fs::read_to_string(workspace_root.join("Cargo.toml"))
        .map_err(|e| format!("Failed to read root Cargo.toml: {e}"))?;
    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if let Some(version) = trimmed.strip_prefix("version = ") {
            return Ok(version.trim().trim_matches('"').to_string());
        }
    }
    Err("No version found in root Cargo.toml".to_string())
}

/// Check if a crate version already exists on crates.io.
fn crate_exists_on_crates_io(name: &str, version: &str) -> Result<bool, String> {
    let url = format!("https://crates.io/api/v1/crates/{name}/{version}");
    match ureq::get(&url).call() {
        Ok(_) => Ok(true),
        Err(ureq::Error::StatusCode(404)) => Ok(false),
        Err(e) => Err(format!(
            "Failed to check {name}@{version} on crates.io: {e}"
        )),
    }
}

/// Wait for a crate to appear in the registry index after publishing.
fn wait_for_crate_index(name: &str, version: &str) -> Result<(), String> {
    println!("⏳ Waiting for {name}@{version} to appear in Cargo registry index...");
    let attempts = 20;
    let delay = std::time::Duration::from_secs(15);

    for attempt in 1..=attempts {
        let output = std::process::Command::new("cargo")
            .args(["info", &format!("{name}@{version}")])
            .output()
            .map_err(|e| format!("Failed to run cargo info: {e}"))?;

        if output.status.success() {
            println!("✅ {name}@{version} is visible in Cargo registry index");
            return Ok(());
        }

        if attempt < attempts {
            println!("   attempt {attempt}/{attempts}: not visible yet; retrying in 15s...");
            std::thread::sleep(delay);
        }
    }

    Err(format!(
        "Timed out waiting for {name}@{version} in Cargo registry index"
    ))
}

/// Publish all workspace crates to crates.io in topological order.
fn publish_crates(workspace_root: &Path, dry_run: bool) -> Result<(), String> {
    let (sorted, no_verify, broken) = resolve_publish_order(workspace_root)?;
    let version = workspace_version(workspace_root)?;

    println!("📦 Publishing rust-camel crates v{version} to crates.io");
    println!("=============================================");
    check_no_publish_cycles(&no_verify, &broken).map_err(|report| {
        format!(
            "Publish blocked: {n} crate(s) in dev/build-dep cycles. Remediate the cycle before publishing.\n{report}",
            n = no_verify.len()
        )
    })?;

    let mut published = 0;
    let mut skipped = 0;

    for c in &sorted {
        println!();
        println!("📦 Publishing {}...", c.name);

        // Check if already published
        match crate_exists_on_crates_io(&c.name, &version) {
            Ok(true) => {
                println!(
                    "⚠️  {}@{version} already exists on crates.io, skipping...",
                    c.name
                );
                skipped += 1;
                continue;
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!(
                    "⚠️  Could not check crates.io for {}@{version}: {e}",
                    c.name
                );
                // Continue anyway — the publish itself will fail if it exists
            }
        }

        println!("📦 Publishing {}@{version}...", c.name);

        if dry_run {
            println!("⚠️  Dry-run: skipping cargo publish verification");
            skipped += 1;
            continue;
        }

        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("publish").arg("--allow-dirty");
        let output = cmd
            .current_dir(workspace_root.join(&c.path))
            .output()
            .map_err(|e| format!("Failed to run cargo publish for {}: {e}", c.name))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}{stderr}");

        if !output.status.success() {
            if combined.contains("already exists") {
                println!(
                    "⚠️  {}@{version} already exists (race), skipping...",
                    c.name
                );
                skipped += 1;
                continue;
            }
            eprintln!("{combined}");
            return Err(format!("Failed to publish {}@{version}", c.name));
        }

        println!("{combined}");
        published += 1;

        // Wait for registry index to propagate
        wait_for_crate_index(&c.name, &version)?;
        std::thread::sleep(std::time::Duration::from_secs(10));
    }

    println!();
    if dry_run {
        println!(
            "🔍 DRY RUN complete: {} crates would be published, {} skipped",
            sorted.len() - skipped,
            skipped
        );
    } else {
        println!("✅ Published {published} crates, skipped {skipped} (already existed)");
    }

    Ok(())
}

/// Extract `name = "..."` from a Cargo.toml [package] section.
fn extract_toml_name(content: &str) -> Option<String> {
    let mut in_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "[package]" {
            in_package = true;
            continue;
        }
        // Any other section header ends [package]
        if trimmed.starts_with('[') {
            if in_package {
                break;
            }
            continue;
        }
        if in_package && trimmed.starts_with("name = ") {
            let val = trimmed.strip_prefix("name = ")?.trim().trim_matches('"');
            return Some(val.to_string());
        }
    }
    None
}

/// Extract camel-* dependencies from all dependency sections that cargo
/// embeds in the published Cargo.toml and validates against the registry
/// index during `cargo publish`. This includes `[dependencies]`,
/// `[dev-dependencies]`, `[build-dependencies]`, and target-specific
/// variants like `[target.'cfg(...)'.dependencies]`. Workspace-internal
/// deps referenced in any of these sections must already exist on
/// crates.io when the crate is published, so they participate in the
/// topological publish order.
#[cfg(test)]
fn extract_normal_camel_deps(content: &str) -> Vec<String> {
    let (normal, weak) = extract_camel_deps_grouped(content);
    let mut all = normal;
    all.extend(weak);
    all.sort();
    all.dedup();
    all
}

/// Split camel-* dependencies into `(normal, weak)` groups.
///
/// `normal` covers `[dependencies]` and `[target.'...'.dependencies]` —
/// hard constraints that must be satisfied before the dependent ships.
///
/// `weak` covers `[dev-dependencies]` and `[build-dependencies]` (plus
/// their target-specific variants) — cargo still resolves them during
/// `cargo publish`, but cycles closed only by weak edges can be broken
/// by publishing one member first.
fn extract_camel_deps_grouped(content: &str) -> (Vec<String>, Vec<String>) {
    let mut normal = Vec::new();
    let mut weak = Vec::new();
    let mut seen_normal = std::collections::HashSet::new();
    let mut seen_weak = std::collections::HashSet::new();
    let mut section = "";

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') {
            section = trimmed;
            continue;
        }
        let Some(dep) = extract_camel_dep_name(trimmed) else {
            continue;
        };
        if is_weak_dependency_section(section) {
            if seen_weak.insert(dep.clone()) {
                weak.push(dep);
            }
        } else if is_dependency_section(section) && seen_normal.insert(dep.clone()) {
            normal.push(dep);
        }
    }
    (normal, weak)
}

/// Returns true for TOML section headers whose dependencies are weak (dev
/// or build). Covers plain sections (`[dev-dependencies]`,
/// `[build-dependencies]`) and target-specific variants. Guards against
/// non-section strings — returns `false` when brackets are absent.
fn is_weak_dependency_section(section: &str) -> bool {
    let section = section.trim();
    if !section.starts_with('[') || !section.ends_with(']') {
        return false;
    }
    let inner = &section[1..section.len() - 1];
    matches!(inner, "dev-dependencies" | "build-dependencies")
        || inner.ends_with(".dev-dependencies")
        || inner.ends_with(".build-dependencies")
}

/// Returns true for TOML section headers whose dependencies cargo resolves
/// when publishing. Covers plain sections (`[dependencies]`,
/// `[dev-dependencies]`, `[build-dependencies]`) and target-specific variants
/// (`[target.'cfg(unix)'.dependencies]`, etc.).
fn is_dependency_section(section: &str) -> bool {
    let section = section.trim();
    if !section.starts_with('[') || !section.ends_with(']') {
        return false;
    }
    let inner = &section[1..section.len() - 1];
    matches!(
        inner,
        "dependencies" | "dev-dependencies" | "build-dependencies"
    ) || inner.ends_with(".dependencies")
        || inner.ends_with(".dev-dependencies")
        || inner.ends_with(".build-dependencies")
}

fn extract_camel_dep_name(line: &str) -> Option<String> {
    let line = line.trim();
    if !line.starts_with("camel-") {
        return None;
    }
    let end = line.find(['.', '=', ' ']).unwrap_or(line.len());
    let name = &line[..end];
    if name.starts_with("camel-") && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Some(name.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn find_workspace_root_finds_sentinel() {
        let dir = std::env::temp_dir().join("xtask-test-ws");
        let bridges = dir.join("bridges");
        fs::create_dir_all(&bridges).unwrap();
        fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();

        let result = find_workspace_root_from(&dir.join("sub").join("deep"));
        assert_eq!(result, Some(dir.clone()));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_workspace_root_returns_none_without_sentinel() {
        let dir = std::env::temp_dir().join("xtask-test-no-sentinel");
        let sub = dir.join("a").join("b");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        // No bridges/ directory

        let result = find_workspace_root_from(&sub);
        assert_eq!(result, None);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(test)]
    mod lint_unwrap_tests {
        use super::*;
        use std::fs;
        use std::path::PathBuf;

        fn tmp_workspace(files: &[(&str, &str)]) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "xtask-lint-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos()
            ));
            for (rel_path, content) in files {
                let full = dir.join(rel_path);
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(&full, content).unwrap();
            }
            // Create bridges/ sentinel so find_workspace_root_from works
            fs::create_dir_all(dir.join("bridges")).unwrap();
            fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
            dir
        }

        #[test]
        fn detects_unwrap_in_production_code() {
            let ws = tmp_workspace(&[(
                "crates/foo/src/lib.rs",
                "fn run() {\n    let x = some_result().unwrap();\n}\n",
            )]);
            let violations = lint_unwrap(&ws).unwrap();
            assert_eq!(violations.len(), 1);
            assert!(violations[0].snippet.contains(".unwrap()"));
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn allows_escape_hatch_comment() {
            let ws = tmp_workspace(&[(
                "crates/foo/src/lib.rs",
                "fn run() {\n    let x = lock.unwrap(); // allow-unwrap\n}\n",
            )]);
            let violations = lint_unwrap(&ws).unwrap();
            assert!(violations.is_empty());
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn skips_tests_directory() {
            let ws = tmp_workspace(&[(
                "crates/foo/tests/integration.rs",
                "fn run() {\n    let x = something().unwrap();\n}\n",
            )]);
            let violations = lint_unwrap(&ws).unwrap();
            assert!(violations.is_empty());
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn detects_expect_in_production_code() {
            let ws = tmp_workspace(&[(
                "crates/foo/src/lib.rs",
                r#"fn run() { let x = val.expect("must exist"); }"#,
            )]);
            let violations = lint_unwrap(&ws).unwrap();
            assert_eq!(violations.len(), 1);
            assert!(violations[0].snippet.contains(".expect("));
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn skips_entire_cfg_test_mod_with_multiple_test_fns() {
            // Bug guard: nested #[test] attrs inside #[cfg(test)] mod must not
            // reset the scope tracker and leak production code into the skip zone.
            let ws = tmp_workspace(&[(
                "crates/foo/src/lib.rs",
                "#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() { let x = v.unwrap(); }\n    #[test]\n    fn b() { let y = v.unwrap(); }\n}\n",
            )]);
            let violations = lint_unwrap(&ws).unwrap();
            assert!(
                violations.is_empty(),
                "cfg(test) block must be fully skipped: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn does_not_skip_production_code_after_test_function() {
            // Bug guard: production code that follows a #[test] fn must still be scanned.
            let ws = tmp_workspace(&[(
                "crates/foo/src/lib.rs",
                "fn prod() { val.unwrap() }\n\n#[test]\nfn test_it() { val.unwrap() }\n\nfn prod2() { val.unwrap() }\n",
            )]);
            let violations = lint_unwrap(&ws).unwrap();
            // prod() and prod2() should be flagged; test_it() should not
            assert_eq!(
                violations.len(),
                2,
                "expected 2 production violations: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn skips_tests_rs_files() {
            let ws = tmp_workspace(&[(
                "crates/foo/tests/integration.rs",
                "fn run() { something().unwrap(); }\n",
            )]);
            let violations = lint_unwrap(&ws).unwrap();
            assert!(violations.is_empty());
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn detects_unwrap_after_cfg_test_type_alias() {
            // Bug guard: #[cfg(test)] type Foo = Bar; sets pending_test_attr but
            // never opens a brace. The flag must be cleared so production code on
            // the next line is still scanned.
            let ws = tmp_workspace(&[(
                "crates/foo/src/lib.rs",
                "#[cfg(test)]\ntype TestAlias = i32;\nfn prod() { val.unwrap() }\n",
            )]);
            let violations = lint_unwrap(&ws).unwrap();
            assert_eq!(
                violations.len(),
                1,
                "production unwrap after #[cfg(test)] type alias must be detected: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn ignores_braces_inside_string_literals_when_tracking_test_scope() {
            // Reproduces rc-4fs: a string literal with unbalanced braces
            // must NOT drift the test_scope_entry_depth counter.
            // The old scanner counted `}` inside "..." as real closing braces,
            // causing the #[cfg(test)] mod tests scope to exit prematurely,
            // flagging subsequent .expect() calls in test code as violations.
            // This test uses 9 closing braces to exceed the brace depth of 2
            // (mod tests { → fn helper { ), and has NO #[test] attribute on
            // the second function so it cannot re-enter the test scope.
            let src = "// rc-4fs regression\n\
                #[cfg(test)]\n\
                mod tests {\n\
                    fn helper_one() {\n\
                        let data = \"}}}\n}}}\n}}}\";\n\
                    }\n\n\
                    fn helper_two() {\n\
                        let v: Option<i32> = None;\n\
                        v.expect(\"should NOT be flagged - inside mod tests\");\n\
                    }\n\
                }\n";
            let violations = lint_unwrap_src(src, "test.rs");
            assert!(
                violations.is_empty(),
                "expected no violations (string literal braces should be ignored), \
                 got: {violations:?}"
            );
        }

        #[test]
        fn ignores_braces_inside_raw_strings_and_block_comments() {
            // rc-4fs extension: raw strings (r#"..."#) and block comments (/* ... */)
            // may also contain unbalanced braces and must NOT drift the counter.
            let src = "// rc-4fs regression\n\
                #[cfg(test)]\n\
                mod tests {\n\
                    fn helper_one() {\n\
                        let _x = r#\"raw string }}}}}}}\"#;\n\
                        /* block comment with }}} */\n\
                    }\n\n\
                    fn helper_two() {\n\
                        let v: Option<i32> = None;\n\
                        v.expect(\"should NOT be flagged - inside mod tests\");\n\
                    }\n\
                }\n";
            let violations = lint_unwrap_src(src, "test.rs");
            assert!(
                violations.is_empty(),
                "expected no violations (raw/block-comment braces should be ignored), \
                 got: {violations:?}"
            );
        }
    }

    #[cfg(test)]
    mod lint_secrets_tests {
        use super::*;
        use std::fs;
        use std::path::PathBuf;

        fn tmp_workspace_secrets(files: &[(&str, &str)]) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "xtask-secrets-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos()
            ));
            for (rel_path, content) in files {
                let full = dir.join(rel_path);
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(&full, content).unwrap();
            }
            fs::create_dir_all(dir.join("bridges")).unwrap();
            fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
            dir
        }

        #[test]
        fn detects_password_in_format_macro() {
            let ws = tmp_workspace_secrets(&[(
                "crates/foo/src/lib.rs",
                r#"fn log() { let msg = format!("connecting with password {}", self.password); }"#, // allow-secret
            )]);
            let violations = lint_secrets(&ws).unwrap();
            assert_eq!(
                violations.len(),
                1,
                "expected 1 violation, got: {violations:?}"
            );
            assert!(violations[0].rule.contains("format macro"));
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn detects_token_in_tracing_macro() {
            let ws = tmp_workspace_secrets(&[(
                "crates/foo/src/lib.rs",
                r#"fn log() { warn!(token = %self.token, "auth failed"); }"#, // allow-secret
            )]);
            let violations = lint_secrets(&ws).unwrap();
            assert_eq!(
                violations.len(),
                1,
                "expected 1 violation, got: {violations:?}"
            );
            assert!(violations[0].rule.contains("tracing macro"));
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn allows_escape_hatch_comment() {
            let ws = tmp_workspace_secrets(&[(
                "crates/foo/src/lib.rs",
                r#"fn test() { let msg = format!("password {}", "dummy"); } // allow-secret"#,
            )]);
            let violations = lint_secrets(&ws).unwrap();
            assert!(
                violations.is_empty(),
                "expected no violations, got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn clean_code_produces_no_violations() {
            let ws = tmp_workspace_secrets(&[(
                "crates/foo/src/lib.rs",
                r#"fn connect(url: &str) { info!(url = %url, "connecting"); }"#,
            )]);
            let violations = lint_secrets(&ws).unwrap();
            assert!(
                violations.is_empty(),
                "expected no violations, got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn detects_multiline_format_macro() {
            // Multiline macro calls must be caught even when the sensitive field
            // is on a different line than the macro invocation.
            let ws = tmp_workspace_secrets(&[(
                "crates/foo/src/lib.rs",
                "fn log() {\n    let msg = format!(\n        \"connecting with {}\",\n        self.password\n    );\n}\n", // allow-secret
            )]);
            let violations = lint_secrets(&ws).unwrap();
            assert_eq!(
                violations.len(),
                1,
                "multiline format! must be caught: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn detects_tracing_shorthand_field() {
            // Shorthand tracing fields like info!(%auth_token) must be caught.
            let ws = tmp_workspace_secrets(&[(
                "crates/foo/src/lib.rs",
                r#"fn log() { info!(%auth_token, "authenticating"); }"#, // allow-secret
            )]);
            let violations = lint_secrets(&ws).unwrap();
            assert_eq!(
                violations.len(),
                1,
                "shorthand %field must be caught: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn detects_bare_tracing_field() {
            // Bare tracing fields like info!(password, "msg") must be caught.
            let ws = tmp_workspace_secrets(&[(
                "crates/foo/src/lib.rs",
                r#"fn log() { info!(password, "msg"); }"#, // allow-secret
            )]);
            let violations = lint_secrets(&ws).unwrap();
            assert_eq!(
                violations.len(),
                1,
                "bare field must be caught: {violations:?}"
            );
            assert!(violations[0].rule.contains("bare field"));
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn detects_expanded_credential_names() {
            // Expanded credential names like client_secret must be caught.
            let ws = tmp_workspace_secrets(&[(
                "crates/foo/src/lib.rs",
                r#"fn log() { format!("client_secret={}", s); }"#, // allow-secret
            )]);
            let violations = lint_secrets(&ws).unwrap();
            assert_eq!(
                violations.len(),
                1,
                "client_secret in format! must be caught: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn test_lint_secrets_combines_sink_and_derive() {
            // One sink-pattern violation (format! password) in crates/foo/src/sink.rs
            // plus one credential-derive violation (manual-redaction struct + Debug)
            // in crates/bar/src/cred.rs. lint_secrets must surface BOTH.
            let ws = tmp_workspace_secrets(&[
                (
                    "crates/foo/src/sink.rs",
                    r#"fn log() { let msg = format!("password {}", self.password); }"#, // allow-secret
                ),
                (
                    "crates/bar/src/cred.rs",
                    "/// ADR-0051 credential boundary: manual-redaction\n#[derive(Debug)]\npub struct Cred { x: u32 }\n",
                ),
            ]);
            let violations = lint_secrets(&ws).unwrap();
            assert_eq!(
                violations.len(),
                2,
                "expected sink+derive = 2 violations, got: {violations:?}"
            );
            let rules: Vec<&str> = violations.iter().map(|v| v.rule.as_str()).collect();
            assert!(
                rules.iter().any(|r| r.contains("format macro")),
                "missing format-macro sink violation: {rules:?}"
            );
            assert!(
                rules
                    .iter()
                    .any(|r| r.contains("manual-redaction forbids Debug")),
                "missing credential-derive violation: {rules:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn test_lint_secrets_parse_failure_returns_err() {
            // A syntactically invalid .rs file under crates/**/src/ makes
            // lint_credential_derives return Err; lint_secrets must propagate
            // it as Err (hard-fail, not silently swallowed).
            let ws = tmp_workspace_secrets(&[("crates/foo/src/lib.rs", "struct Broken {\n")]);
            let res = lint_secrets(&ws);
            assert!(
                res.is_err(),
                "expected parse-failure Err, got Ok({:?})",
                res.as_ref().map(|v| v.len())
            );
            fs::remove_dir_all(&ws).unwrap();
        }
    }

    #[cfg(test)]
    mod lint_credential_derives_tests {
        use super::*;
        use std::fs;
        use std::path::PathBuf;

        fn tmp_workspace_cred(files: &[(&str, &str)]) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "xtask-cred-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap() // allow-unwrap
                    .subsec_nanos()
            ));
            for (rel_path, content) in files {
                let full = dir.join(rel_path);
                fs::create_dir_all(full.parent().unwrap()).unwrap(); // allow-unwrap
                fs::write(&full, content).unwrap(); // allow-unwrap
            }
            fs::create_dir_all(dir.join("bridges")).unwrap(); // allow-unwrap
            fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap(); // allow-unwrap
            dir
        }

        #[test]
        fn test_manual_redaction_debug_violation() {
            let src = "/// ADR-0051 credential boundary: manual-redaction\n#[derive(Debug)]\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert_eq!(
                v[0].rule,
                "credential-derive: manual-redaction forbids Debug"
            );
        }

        #[test]
        fn test_enum_manual_redaction_debug_violation() {
            // Exercises the Item::Enum arm of has_zeroizing_field,
            // check_credential_item, and item_has_cfg_test.
            let src = "/// ADR-0051 credential boundary: manual-redaction\n#[derive(Debug)]\npub enum Foo { A, B }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].rule.contains("manual-redaction forbids Debug"));
        }

        #[test]
        fn test_enum_zeroizing_without_classification() {
            // Exercises the Item::Enum arm of has_zeroizing_field by giving an
            // enum variant a Zeroizing field and no classification marker.
            let src = "pub enum Cred { Plain(String), Secret(Zeroizing<String>) }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(
                v[0].rule
                    .contains("Zeroizing field requires manual-redaction classification")
            );
        }

        #[test]
        fn test_qualified_derive_serialize_violation() {
            // Regression: `serde::Serialize` is a multi-segment path; the old
            // `meta.path.get_ident()` extraction silently dropped it.
            let src = "/// ADR-0051 credential boundary: redacting-wrapper\n#[derive(serde::Serialize)]\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].rule.contains("redacting-wrapper forbids Serialize"));
        }

        #[test]
        fn test_manual_redaction_serialize_violation() {
            let src = "/// ADR-0051 credential boundary: manual-redaction\n#[derive(Serialize)]\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].rule.contains("manual-redaction forbids Serialize"));
        }

        #[test]
        fn test_manual_redaction_clean() {
            let src = "/// ADR-0051 credential boundary: manual-redaction\n#[derive(Clone)]\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert!(v.is_empty(), "{v:?}");
        }

        #[test]
        fn test_manual_redaction_with_manual_impl_debug() {
            let src = "/// ADR-0051 credential boundary: manual-redaction\npub struct Foo { x: u32 }\nimpl std::fmt::Debug for Foo {\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, \"redacted\") }\n}\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert!(v.is_empty(), "{v:?}");
        }

        #[test]
        fn test_redacting_wrapper_debug_ok() {
            let src = "/// ADR-0051 credential boundary: redacting-wrapper\n#[derive(Debug)]\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert!(v.is_empty(), "{v:?}");
        }

        #[test]
        fn test_redacting_wrapper_serialize_violation() {
            let src = "/// ADR-0051 credential boundary: redacting-wrapper\n#[derive(Serialize)]\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].rule.contains("redacting-wrapper forbids Serialize"));
        }

        #[test]
        fn test_protocol_dto_serialize_ok() {
            let src = "/// ADR-0051 credential boundary: protocol-dto\n#[derive(Serialize)]\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert!(v.is_empty(), "{v:?}");
        }

        #[test]
        fn test_protocol_dto_debug_violation() {
            let src = "/// ADR-0051 credential boundary: protocol-dto\n#[derive(Debug)]\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].rule.contains("protocol-dto forbids Debug"));
        }

        #[test]
        fn test_zeroizing_without_classification() {
            let src = "pub struct Foo { value: Zeroizing<String> }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(
                v[0].rule
                    .contains("Zeroizing field requires manual-redaction classification")
            );
        }

        #[test]
        fn test_qualified_zeroizing_without_classification() {
            let src = "pub struct Foo { value: zeroize::Zeroizing<String> }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(
                v[0].rule
                    .contains("Zeroizing field requires manual-redaction classification")
            );
        }

        #[test]
        fn test_zeroizing_with_classification() {
            let src = "/// ADR-0051 credential boundary: manual-redaction\npub struct Foo { value: Zeroizing<String> }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert!(v.is_empty(), "{v:?}");
        }

        #[test]
        fn test_unannotated_no_zeroizing() {
            let src = "pub struct Foo { path: String }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert!(v.is_empty(), "{v:?}");
        }

        #[test]
        fn test_credential_suggesting_name_no_violation() {
            let src = "pub struct Foo { client_key_path: String }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert!(v.is_empty(), "{v:?}");
        }

        #[test]
        fn test_multiline_derive() {
            let src = "/// ADR-0051 credential boundary: manual-redaction\n#[derive(\n    Debug,\n    Clone,\n)]\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].rule.contains("manual-redaction forbids Debug"));
        }

        #[test]
        fn test_unknown_classification() {
            let src =
                "/// ADR-0051 credential boundary: unknown-value\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].rule.contains("unknown classification"));
        }

        #[test]
        fn test_malformed_classification() {
            let src = "/// ADR-0051 credential boundary:\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].rule.contains("malformed classification"));
        }

        #[test]
        fn test_conflicting_duplicate() {
            let src = "/// ADR-0051 credential boundary: manual-redaction\n/// ADR-0051 credential boundary: protocol-dto\npub struct Foo { x: u32 }\n";
            let v = lint_credential_derives_src(src, "t.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert!(v[0].rule.contains("conflicting duplicate classifications"));
        }

        #[test]
        fn test_parse_failure_returns_error() {
            let src = "struct Broken {";
            let res = lint_credential_derives_src(src, "t.rs");
            assert!(res.is_err(), "expected parse error, got: {res:?}");
        }

        #[test]
        fn test_violation_includes_file_and_line() {
            let src = "/// ADR-0051 credential boundary: manual-redaction\n#[derive(Debug)]\npub struct Foo {\n    x: u32,\n}\n";
            let v = lint_credential_derives_src(src, "my_file.rs").unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            assert_eq!(v[0].file, "my_file.rs");
            let expected_line = src
                .lines()
                .position(|l| l.contains("pub struct Foo"))
                .map(|i| i + 1)
                .unwrap(); // allow-unwrap
            assert_eq!(v[0].line, expected_line);
        }

        #[test]
        fn test_violations_present_exit_nonzero() {
            let ws = tmp_workspace_cred(&[(
                "crates/foo/src/lib.rs",
                "/// ADR-0051 credential boundary: manual-redaction\n#[derive(Debug)]\npub struct Foo { x: u32 }\n",
            )]);
            let v = lint_credential_derives(&ws).unwrap(); // allow-unwrap
            assert_eq!(v.len(), 1, "{v:?}");
            fs::remove_dir_all(&ws).unwrap(); // allow-unwrap
        }
    }

    #[cfg(test)]
    mod lint_log_levels_tests {
        use super::*;
        use std::fs;
        use std::path::PathBuf;

        fn tmp_workspace_log(files: &[(&str, &str)]) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "xtask-log-levels-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos()
            ));
            for (rel_path, content) in files {
                let full = dir.join(rel_path);
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(&full, content).unwrap();
            }
            fs::create_dir_all(dir.join("bridges")).unwrap();
            fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
            // Seed allowlist file (empty) so the lint doesn't error on missing path.
            let xtask = dir.join("scripts").join("xtask");
            fs::create_dir_all(&xtask).unwrap();
            fs::write(xtask.join("allowlist-log-levels.txt"), "# header\n").unwrap();
            dir
        }

        #[test]
        fn detects_unannotated_error_macro() {
            let ws =
                tmp_workspace_log(&[("crates/foo/src/lib.rs", "fn x() { error!(\"boom\"); }\n")]);
            let violations = lint_log_levels(&ws).unwrap();
            assert_eq!(violations.len(), 1);
            assert!(violations[0].snippet.contains("error!"));
            fs::remove_dir_all(&ws).unwrap();
        }

        /// Regression: production files often embed `#[cfg(test)] mod tests { ... }`
        /// with `error!()` inside. These MUST NOT be flagged — they're test scope.
        /// Ported from lint_unwrap's pending_test_attr logic.
        #[test]
        fn ignores_error_inside_cfg_test_mod_in_production_file() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn prod() { /* happy path */ }\n\
                 \n\
                 #[cfg(test)]\n\
                 mod tests {\n\
                     #[test]\n\
                     fn t() { error!(\"boom\"); }\n\
                 }\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn accepts_system_broken_annotation() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x() {\n    // log-policy: system-broken\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn rejects_outside_contract_without_replacement_signal() {
            // Formerly accepted as a skeleton; now outside-contract requires
            // an adjacent replacement signal (Task 4).
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x() {\n    // log-policy: outside-contract\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert_eq!(violations.len(), 1);
            assert!(violations[0].snippet.contains("outside-contract"));
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn rejects_handler_owned_with_error_macro() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x() {\n    // log-policy: handler-owned\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert_eq!(violations.len(), 1);
            assert!(violations[0].snippet.contains("handler-owned"));
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn rejects_unknown_annotation_kind() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x() {\n    // log-policy: made-up\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert_eq!(violations.len(), 1);
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn outside_contract_accepted_with_increment_errors() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x(metrics: &dyn MetricsCollector) {\n    metrics.increment_errors(\"route\", \"b-prime:sql:on-consume\");\n    // log-policy: outside-contract\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn outside_contract_accepted_with_force_unhealthy() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x(reg: &HealthCheckRegistry) {\n    reg.force_unhealthy_for_route(\"r\", \"endpoint-creation\", \"e\");\n    // log-policy: outside-contract\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn outside_contract_accepted_with_bridged_guard() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x(bridged: bool) {\n    if !bridged {\n        // log-policy: outside-contract\n        error!(\"boom\");\n    }\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn outside_contract_rejected_without_replacement() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x() {\n    // log-policy: outside-contract\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert_eq!(violations.len(), 1);
            assert!(violations[0].snippet.contains("outside-contract"));
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn outside_contract_rejects_invalid_label_format() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x(metrics: &dyn MetricsCollector) {\n    metrics.increment_errors(\"route\", \"on-consume\");\n    // log-policy: outside-contract\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert_eq!(violations.len(), 1);
            assert!(violations[0].snippet.contains("label"));
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn outside_contract_accepts_b_prime_label() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x(metrics: &dyn MetricsCollector) {\n    metrics.increment_errors(\"route\", \"b-prime:sql:on-consume\");\n    // log-policy: outside-contract\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn outside_contract_accepts_e_label() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x(metrics: &dyn MetricsCollector) {\n    metrics.increment_errors(\"route\", \"e:grpc:accept\");\n    // log-policy: outside-contract\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn outside_contract_accepts_g_label() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x(metrics: &dyn MetricsCollector) {\n    metrics.increment_errors(\"route\", \"g:http:endpoint-create\");\n    // log-policy: outside-contract\n    error!(\"boom\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn ignores_legacy_increment_errors_labels_outside_log_policy() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x(metrics: &dyn MetricsCollector) {\n    metrics.increment_errors(\"route\", \"timeout\");\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn allowlist_skips_listed_file_line() {
            let ws =
                tmp_workspace_log(&[("crates/foo/src/lib.rs", "fn x() { error!(\"boom\"); }\n")]);
            fs::create_dir_all(ws.join("scripts").join("xtask")).unwrap();
            fs::write(
                ws.join("scripts").join("xtask").join("allowlist-log-levels.txt"),
                "# allowlist for log-level lint (see ADR-0012)\n# format: <relative path>:<line>\ncrates/foo/src/lib.rs:1\n",
            ).unwrap();
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn inline_allow_escape_skips_violation() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x() {\n    // TODO(ADR-0012-e-metrics): via bd rc-test\n    error!(\"boom\"); // allow-log-levels\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        /// Regression for second-expert review Q2: every inline escape MUST
        /// carry a TODO(ADR-0012-...) marker with a bd id.
        #[test]
        fn inline_escape_without_todo_marker_is_violation() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x() { error!(\"boom\"); } // allow-log-levels\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert_eq!(violations.len(), 1);
            assert!(
                violations[0].snippet.contains("TODO(ADR-0012-"),
                "got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn inline_escape_with_todo_marker_and_bd_id_accepted() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x() {\n    // TODO(ADR-0012-e-metrics): wire increment_errors via bd rc-test\n    error!(\"boom\"); // allow-log-levels\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert!(violations.is_empty(), "got: {violations:?}");
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn inline_escape_with_todo_but_no_bd_id_is_violation() {
            let ws = tmp_workspace_log(&[(
                "crates/foo/src/lib.rs",
                "fn x() {\n    // TODO(ADR-0012-e-metrics): wire increment_errors someday\n    error!(\"boom\"); // allow-log-levels\n}\n",
            )]);
            let violations = lint_log_levels(&ws).unwrap();
            assert_eq!(violations.len(), 1);
            assert!(
                violations[0].snippet.contains("bd <id>"),
                "got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        /// Regression: the lint's own source under `scripts/xtask/` MUST be
        /// excluded from the inline-escape counter. Otherwise the lint
        /// self-reports its doc comments, regex definitions, error messages,
        /// and test fixtures as escapes (13+ mentions of `// allow-log-levels`
        /// in scripts/xtask/src/main.rs alone). ADR-0012 applies to component
        /// code under `crates/` and `examples/`, not to meta-tooling.
        #[test]
        fn inline_escape_counter_ignores_scripts_xtask_self_references() {
            let ws =
                tmp_workspace_log(&[("crates/foo/src/lib.rs", "fn x() { error!(\"boom\"); }\n")]);
            // Simulate the lint's own source file with multiple self-references
            // (doc comments + string literals + regex definition, exactly as
            // the real scripts/xtask/src/main.rs contains).
            let xtask_src = ws.join("scripts").join("xtask").join("src");
            fs::create_dir_all(&xtask_src).unwrap();
            fs::write(
                xtask_src.join("main.rs"),
                "//! doc comment mentioning // allow-log-levels\n\
                 fn count() {\n\
                 \x20   let re = regex::Regex::new(r\"//\\s*allow-log-levels\").unwrap();\n\
                 \x20   let fixture = \"error!(); // allow-log-levels\";\n\
                 }\n",
            )
            .unwrap();
            // Must NOT error: the 3 self-references in scripts/xtask/src/main.rs
            // are excluded by the scripts/ path-component filter.
            let violations = lint_log_levels(&ws).unwrap();
            // crates/foo/src/lib.rs has an unannotated error!() → 1 violation.
            // The 3 self-references in scripts/xtask/src/main.rs are NOT counted.
            assert_eq!(
                violations.len(),
                1,
                "scripts/xtask/ self-references must not be counted as inline escapes: got {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }
    }

    mod dependency_extraction {
        use super::*;

        #[test]
        fn includes_dev_dependencies() {
            // Reproduces the v0.13.0 release failure: camel-platform-kubernetes
            // only declares camel-core under [dev-dependencies], so the old
            // extractor missed it and the publish order was wrong.
            let cargo_toml = r#"
[package]
name = "camel-platform-kubernetes"

[dependencies]
camel-api = { workspace = true }

[dev-dependencies]
camel-core = { workspace = true }
"#;
            let deps = extract_normal_camel_deps(cargo_toml);
            assert!(deps.contains(&"camel-api".to_string()));
            assert!(
                deps.contains(&"camel-core".to_string()),
                "dev-dependencies must be included in publish order: got {deps:?}"
            );
        }

        #[test]
        fn includes_build_dependencies() {
            let cargo_toml = r#"
[package]
name = "camel-foo"

[dependencies]
camel-api = { workspace = true }

[build-dependencies]
camel-bean-macros = { workspace = true }
"#;
            let deps = extract_normal_camel_deps(cargo_toml);
            assert!(deps.contains(&"camel-api".to_string()));
            assert!(deps.contains(&"camel-bean-macros".to_string()));
        }

        #[test]
        fn includes_target_specific_dependencies() {
            let cargo_toml = r#"
[package]
name = "camel-foo"

[target.'cfg(unix)'.dependencies]
camel-core = { workspace = true }

[target.'cfg(windows)'.dev-dependencies]
camel-api = { workspace = true }
"#;
            let deps = extract_normal_camel_deps(cargo_toml);
            assert!(deps.contains(&"camel-core".to_string()));
            assert!(deps.contains(&"camel-api".to_string()));
        }

        #[test]
        fn ignores_unknown_sections() {
            let cargo_toml = r#"
[package]
name = "camel-foo"

[dependencies]
camel-core = { workspace = true }

[lints]
workspace = true

[features]
default = ["camel-api"]
"#;
            let deps = extract_normal_camel_deps(cargo_toml);
            assert_eq!(deps, vec!["camel-core"]);
        }

        #[test]
        fn deduplicates_dependencies() {
            let cargo_toml = r#"
[package]
name = "camel-foo"

[dependencies]
camel-core = { workspace = true }

[dev-dependencies]
camel-core = { workspace = true }
"#;
            let deps = extract_normal_camel_deps(cargo_toml);
            assert_eq!(deps.len(), 1);
            assert_eq!(deps[0], "camel-core");
        }

        #[test]
        fn is_dependency_section_classifies_headers() {
            assert!(is_dependency_section("[dependencies]"));
            assert!(is_dependency_section("[dev-dependencies]"));
            assert!(is_dependency_section("[build-dependencies]"));
            assert!(is_dependency_section("[target.'cfg(unix)'.dependencies]"));
            assert!(is_dependency_section(
                "[target.\"cfg(unix)\".dev-dependencies]"
            ));
            assert!(is_dependency_section(
                "[target.x86_64-pc-windows-msvc.build-dependencies]"
            ));

            assert!(!is_dependency_section("[package]"));
            assert!(!is_dependency_section("[features]"));
            assert!(!is_dependency_section("[lints]"));
            assert!(!is_dependency_section("[target.'cfg(unix)']"));
            // Bare `[target]` table header (no nested dep section) must not match.
            assert!(!is_dependency_section("[target]"));
        }
    }

    mod publish_order_scc_tests {
        use super::*;

        /// Build a publishable `WorkspaceCrate` with the given camel-* normal
        /// and weak (dev/build) deps. Paths are synthetic — these fixtures
        /// never touch disk.
        fn wc(name: &str, normal: &[&str], weak: &[&str]) -> WorkspaceCrate {
            WorkspaceCrate {
                name: name.to_string(),
                path: format!("crates/{name}"),
                normal_deps: normal.iter().map(|s| s.to_string()).collect(),
                weak_deps: weak.iter().map(|s| s.to_string()).collect(),
                publish: true,
            }
        }

        #[test]
        fn tarjan_identifies_nontrivial_scc() {
            // 0 → 1 → 2 → 0  (one 3-node cycle).
            let adj: Vec<Vec<(usize, EdgeKind)>> = vec![
                vec![(1, EdgeKind::Normal)],
                vec![(2, EdgeKind::Normal)],
                vec![(0, EdgeKind::Normal)],
            ];
            let sccs = tarjan_scc(&adj);
            let big: Vec<&Vec<usize>> = sccs.iter().filter(|s| s.len() == 3).collect();
            assert_eq!(
                big.len(),
                1,
                "expected exactly one size-3 SCC, got {sccs:?}"
            );
            let mut members = big[0].clone();
            members.sort();
            assert_eq!(members, vec![0, 1, 2]);
        }

        #[test]
        fn acyclic_weak_graph_breaks_zero_edges() {
            // A --weak--> B --weak--> C, no back path: no cycle, no break.
            let crates = vec![
                wc("camel-a", &[], &["camel-b"]),
                wc("camel-b", &[], &["camel-c"]),
                wc("camel-c", &[], &[]),
            ];
            let (sorted, no_verify, broken) = compute_publish_order(crates).expect("acyclic");
            assert!(
                broken.is_empty(),
                "no weak edge should be broken: {broken:?}"
            );
            assert!(no_verify.is_empty(), "no_verify must be empty");
            assert_eq!(
                sorted.len(),
                3,
                "all three crates must be scheduled: only {sorted:?}"
            );
        }

        #[test]
        fn deterministic_lexicographic_edge_selection() {
            // SCC {a, b, c}: a --weak--> c, b --weak--> c, c --normal--> {a, b}.
            // Two candidate breakable intra-SCC weak edges with holders "a"
            // and "b"; the lexicographically smaller holder ("a") must be
            // chosen first.
            let crates = vec![
                wc("camel-a", &[], &["camel-c"]),
                wc("camel-b", &[], &["camel-c"]),
                wc("camel-c", &["camel-a", "camel-b"], &[]),
            ];
            let (_sorted, no_verify, broken) = compute_publish_order(crates).expect("breakable");
            // The {a,b,c} SCC needs TWO breaks (a→c then b→c): breaking a→c
            // leaves {b,c} as a non-trivial SCC, which the recompute pass must
            // catch. Asserting the full 2-edge sequence exercises both the
            // lexicographic first pick AND the recompute-after-break contract.
            assert_eq!(
                broken.len(),
                2,
                "expected exactly two breaks (recompute catches {{b,c}}): {broken:?}"
            );
            assert_eq!(
                broken[0].0, "camel-a",
                "first break holder must be lexicographically-smallest camel-a, not {:?}",
                broken[0].0
            );
            assert_eq!(
                broken[1].0, "camel-b",
                "second break holder must be camel-b after recompute, not {:?}",
                broken[1].0
            );
            assert!(no_verify.contains("camel-a") && no_verify.contains("camel-b"));
        }

        #[test]
        fn recompute_scc_after_each_break() {
            // Two independent cycles: {p,q} and {r,s}. Each needs exactly one
            // weak-edge break. The old greedy loop fabricated phantom edges;
            // SCC-gating must break exactly 2 and no third.
            let crates = vec![
                wc("camel-p", &[], &["camel-q"]),
                wc("camel-q", &["camel-p"], &[]),
                wc("camel-r", &[], &["camel-s"]),
                wc("camel-s", &["camel-r"], &[]),
            ];
            let (sorted, _no_verify, broken) = compute_publish_order(crates).expect("breakable");
            assert_eq!(
                broken.len(),
                2,
                "exactly two real edges must break, no phantom: {broken:?}"
            );
            assert_eq!(sorted.len(), 4, "all four crates must be scheduled");
        }

        #[test]
        fn hard_normal_only_cycle_errors() {
            // a --normal--> b --normal--> a, no weak edge to break.
            let crates = vec![
                wc("camel-a", &["camel-b"], &[]),
                wc("camel-b", &["camel-a"], &[]),
            ];
            let err = compute_publish_order(crates).unwrap_err();
            assert!(
                err.contains("dependency cycles"),
                "hard normal-only cycle must error with 'dependency cycles', got: {err}"
            );
        }

        /// Build a tempdir workspace with the given `crates/*/Cargo.toml`
        /// files. Creates `bridges/` and root `Cargo.toml` sentinels so
        /// `resolve_publish_order` can crawl the workspace.
        fn tmp_ws(crates: &[(&str, &str)]) -> std::path::PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "xtask-show-cycles-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos()
            ));
            std::fs::create_dir_all(dir.join("bridges")).unwrap();
            std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
            for (rel_path, content) in crates {
                let full = dir.join(rel_path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, content).unwrap();
            }
            dir
        }

        #[test]
        fn show_cycles_formats_holder_first() {
            // camel-a --dev-dep--> camel-b, camel-b --normal-dep--> camel-a.
            // The cycle is closed by a → b (weak), so a is the holder.
            let ws = tmp_ws(&[
                (
                    "crates/camel-a/Cargo.toml",
                    "[package]\nname = \"camel-a\"\nversion = \"0.1.0\"\n\n[dev-dependencies]\ncamel-b = \"0.1.0\"\n",
                ),
                (
                    "crates/camel-b/Cargo.toml",
                    "[package]\nname = \"camel-b\"\nversion = \"0.1.0\"\n\n[dependencies]\ncamel-a = \"0.1.0\"\n",
                ),
            ]);
            let mut buf: Vec<u8> = Vec::new();
            show_cycles(&ws, &mut buf).unwrap();
            let out = String::from_utf8(buf).unwrap();
            assert!(
                out.contains("no_verify set: 1 crate(s)"),
                "expected no_verify header, got:\n{out}"
            );
            assert!(
                out.contains("  camel-a\n"),
                "expected camel-a listed as holder, got:\n{out}"
            );
            assert!(
                out.contains("camel-a --dev/build-dep--> camel-b"),
                "expected edge line with holder first, got:\n{out}"
            );
            std::fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn show_cycles_clean_graph() {
            // camel-a --normal--> camel-b (acyclic, no dev-dep).
            let ws = tmp_ws(&[
                (
                    "crates/camel-a/Cargo.toml",
                    "[package]\nname = \"camel-a\"\nversion = \"0.1.0\"\n\n[dependencies]\ncamel-b = \"0.1.0\"\n",
                ),
                (
                    "crates/camel-b/Cargo.toml",
                    "[package]\nname = \"camel-b\"\nversion = \"0.1.0\"\n",
                ),
            ]);
            let mut buf: Vec<u8> = Vec::new();
            show_cycles(&ws, &mut buf).unwrap();
            let out = String::from_utf8(buf).unwrap();
            assert!(
                out.contains("no_verify set: 0 crate(s)"),
                "expected empty no_verify, got:\n{out}"
            );
            assert!(
                out.contains("broken weak edges:\n"),
                "expected broken weak edges section, got:\n{out}"
            );
            // No edge line should follow — just the empty newline.
            let after_header = out.split("broken weak edges:\n").nth(1).unwrap();
            // show_cycles prints a newline for empty broken
            assert!(
                after_header.trim().is_empty(),
                "expected zero edge lines after broken header, got: '{after_header}'"
            );
            std::fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn lint_fails_when_no_verify_nonempty() {
            // camel-a --dev-dep--> camel-b, camel-b --normal-dep--> camel-a.
            // Cycle closed by a's weak edge; a is the holder.
            let ws = tmp_ws(&[
                (
                    "crates/camel-a/Cargo.toml",
                    "[package]\nname = \"camel-a\"\nversion = \"0.1.0\"\n\n[dev-dependencies]\ncamel-b = \"0.1.0\"\n",
                ),
                (
                    "crates/camel-b/Cargo.toml",
                    "[package]\nname = \"camel-b\"\nversion = \"0.1.0\"\n\n[dependencies]\ncamel-a = \"0.1.0\"\n",
                ),
            ]);
            let err = lint_publish_cycles(&ws).unwrap_err();
            assert!(
                err.contains("camel-a --dev/build-dep--> camel-b"),
                "lint error must name holder + edge, got: {err}"
            );
            assert!(
                err.contains("camel-a"),
                "lint error must mention holder camel-a, got: {err}"
            );
            std::fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn lint_passes_on_clean_graph() {
            // camel-a --normal--> camel-b (acyclic, no dev-dep),
            // camel-test is not a dependency of either.
            let ws = tmp_ws(&[
                (
                    "crates/camel-a/Cargo.toml",
                    "[package]\nname = \"camel-a\"\nversion = \"0.1.0\"\n\n[dependencies]\ncamel-b = \"0.1.0\"\n",
                ),
                (
                    "crates/camel-b/Cargo.toml",
                    "[package]\nname = \"camel-b\"\nversion = \"0.1.0\"\n",
                ),
            ]);
            lint_publish_cycles(&ws).expect("clean graph must return Ok(())");
            std::fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn lint_fails_on_camel_test_dependent() {
            // Publishable crate camel-x declares camel-test as a dev-dep.
            let ws = tmp_ws(&[(
                "crates/camel-x/Cargo.toml",
                "[package]\nname = \"camel-x\"\nversion = \"0.1.0\"\n\n[dev-dependencies]\ncamel-test = { workspace = true }\n",
            )]);
            let err = lint_publish_cycles(&ws).unwrap_err();
            assert!(
                err.contains("camel-x")
                    && err.contains("camel-test")
                    && err.contains("dev-dependencies"),
                "lint error must name crate camel-x + camel-test + dev-dependencies, got: {err}"
            );
            std::fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn lint_and_show_cycles_report_identical_set() {
            // Shared cycle fixture: camel-p --dev-dep--> camel-q --normal--> camel-p.
            // Both lint and show_cycles must report the same no_verify holder set.
            let ws = tmp_ws(&[
                (
                    "crates/camel-p/Cargo.toml",
                    "[package]\nname = \"camel-p\"\nversion = \"0.1.0\"\n\n[dev-dependencies]\ncamel-q = \"0.1.0\"\n",
                ),
                (
                    "crates/camel-q/Cargo.toml",
                    "[package]\nname = \"camel-q\"\nversion = \"0.1.0\"\n\n[dependencies]\ncamel-p = \"0.1.0\"\n",
                ),
            ]);

            let (_, no_verify, _) = resolve_publish_order(&ws).unwrap();
            assert!(
                no_verify.contains("camel-p"),
                "expected camel-p in no_verify: {no_verify:?}"
            );

            let lint_err = lint_publish_cycles(&ws).unwrap_err();
            for holder in &no_verify {
                assert!(
                    lint_err.contains(holder.as_str()),
                    "lint error must name holder {holder}, got: {lint_err}"
                );
            }
            std::fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn lint_asserts_camel_test_remains_publishable() {
            // Read the real camel-test manifest and assert publish is NOT false.
            let ws_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf();
            let manifest =
                std::fs::read_to_string(ws_root.join("crates/camel-test/Cargo.toml")).unwrap();
            assert!(
                is_publishable_crate(&manifest),
                "camel-test must remain publishable (publish = false must not be set)"
            );
        }

        #[test]
        fn publish_crates_fail_closed_on_no_verify() {
            let no_verify: std::collections::HashSet<String> =
                ["camel-a".to_string()].into_iter().collect();
            let broken = vec![("camel-a".to_string(), "camel-b".to_string())];
            let err = check_no_publish_cycles(&no_verify, &broken).unwrap_err();
            assert!(
                err.contains("camel-a --dev/build-dep--> camel-b"),
                "error must name holder + edge, got: {err}"
            );
            assert!(
                err.contains("camel-a"),
                "error must mention holder, got: {err}"
            );
        }

        #[test]
        fn publish_crates_pass_when_no_cycle() {
            let no_verify: std::collections::HashSet<String> = std::collections::HashSet::new();
            let broken: Vec<(String, String)> = vec![];
            check_no_publish_cycles(&no_verify, &broken)
                .expect("empty no_verify must return Ok(())");
        }
    }

    mod lint_non_exhaustive {
        use super::*;

        #[test]
        fn lint_passes_enum_with_non_exhaustive() {
            let src = "#[non_exhaustive]\npub enum E { A }\n";
            let v = lint_non_exhaustive_src(src, "test.rs");
            assert!(v.is_empty(), "expected no violations, got {v:?}");
        }

        #[test]
        fn lint_fails_enum_without_attribute_or_note() {
            let src = "pub enum E { A }\n";
            let v = lint_non_exhaustive_src(src, "test.rs");
            assert_eq!(v.len(), 1, "expected 1 violation, got {v:?}");
            assert_eq!(v[0].file, "test.rs");
            assert_eq!(v[0].line, 1);
        }

        #[test]
        fn lint_passes_enum_with_valid_exception_note() {
            let src = "/// exhaustive-by-contract: closed set is the contract\npub enum E { A }\n";
            let v = lint_non_exhaustive_src(src, "test.rs");
            assert!(v.is_empty(), "expected no violations, got {v:?}");
        }

        #[test]
        fn lint_rejects_plain_comment_marker() {
            let src = "// exhaustive-by-contract: foo\npub enum E { A }\n";
            let v = lint_non_exhaustive_src(src, "test.rs");
            assert_eq!(
                v.len(),
                1,
                "plain `//` comment must NOT satisfy the rule, got {v:?}"
            );
            assert_eq!(v[0].line, 2);
        }

        #[test]
        fn lint_rejects_empty_rationale() {
            let src = "/// exhaustive-by-contract:\npub enum E { A }\n";
            let v = lint_non_exhaustive_src(src, "test.rs");
            assert_eq!(
                v.len(),
                1,
                "empty rationale after the colon must NOT satisfy, got {v:?}"
            );
            assert_eq!(v[0].line, 2);
        }

        #[test]
        fn lint_ignores_non_pub_enum() {
            let src = "enum Internal { A }\n";
            let v = lint_non_exhaustive_src(src, "test.rs");
            assert!(
                v.is_empty(),
                "non-pub enum is not released contract surface, got {v:?}"
            );
        }

        #[test]
        fn lint_rejects_detached_marker() {
            let src = "/// exhaustive-by-contract: closed set\n\npub enum E { A }\n";
            let v = lint_non_exhaustive_src(src, "test.rs");
            assert_eq!(
                v.len(),
                1,
                "blank line between rustdoc and enum detaches the note, got {v:?}"
            );
            assert_eq!(v[0].line, 3);
        }
    }

    #[cfg(test)]
    mod lint_ignore_tests {
        use super::*;
        use std::fs;
        use std::path::PathBuf;

        /// Build a temp workspace containing the given relative-path files and
        /// seed `scripts/xtask/allowlist-ignore.txt` (empty) so the lint does
        /// not error on the missing path. Returns the workspace root.
        fn tmp_workspace_ignore(files: &[(&str, &str)], allowlist: &[&str]) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "xtask-ignore-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos()
            ));
            for (rel_path, content) in files {
                let full = dir.join(rel_path);
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(&full, content).unwrap();
            }
            fs::create_dir_all(dir.join("bridges")).unwrap();
            fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
            let xtask = dir.join("scripts").join("xtask");
            fs::create_dir_all(&xtask).unwrap();
            let mut allowlist_contents =
                String::from("# allowlist for ignore-test lint (see ADR-0054)\n");
            for entry in allowlist {
                allowlist_contents.push_str(entry);
                allowlist_contents.push('\n');
            }
            fs::write(xtask.join("allowlist-ignore.txt"), allowlist_contents).unwrap();
            dir
        }

        // ----- per-attribute validation -----

        #[test]
        fn bare_ignore_is_violation() {
            let ws = tmp_workspace_ignore(
                &[("crates/foo/src/lib.rs", "#[ignore]\n#[test]\nfn foo() {}\n")],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(
                violations.len(),
                1,
                "expected 1 violation, got {violations:?}"
            );
            assert!(
                violations[0].snippet.contains("ignore:missing-reason"),
                "expected `ignore:missing-reason` in snippet, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn valid_requires_pre_built_passes() {
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/components/camel-component-wasm/tests/foo.rs",
                    r#"#[ignore = "requires pre-built guest wasm"]
#[test]
fn foo() {}
"#,
                )],
                &["crates/components/camel-component-wasm/tests/foo.rs"],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert!(
                violations.is_empty(),
                "valid allowlisted `requires pre-built` must pass, got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn valid_slow_test_passes() {
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    r#"#[ignore = "slow test: file polling"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert!(
                violations.is_empty(),
                "valid `slow test:` must pass, got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn invalid_prefix_is_violation() {
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    r#"#[ignore = "because reasons"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(
                violations.len(),
                1,
                "expected 1 violation, got {violations:?}"
            );
            assert!(
                violations[0].snippet.contains("ignore:invalid-prefix"),
                "expected `ignore:invalid-prefix` in snippet, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn near_prefix_typo_rejected() {
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    r#"#[ignore = "requires livewire foo"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(violations.len(), 1, "got {violations:?}");
            assert!(
                violations[0].snippet.contains("ignore:invalid-prefix"),
                "near-prefix typo `requires livewire` must be rejected, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn case_sensitive_prefix() {
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    r#"#[ignore = "Requires live Redis"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(violations.len(), 1, "got {violations:?}");
            assert!(
                violations[0].snippet.contains("ignore:invalid-prefix"),
                "capitalized prefix `Requires live` must be rejected, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn empty_detail_rejected() {
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    r#"#[ignore = "slow test: "]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(violations.len(), 1, "got {violations:?}");
            assert!(
                violations[0].snippet.contains("ignore:empty-detail"),
                "empty detail after delimiter must be rejected, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn prefix_without_detail_no_space() {
            // `requires live` with no trailing space — does not match the
            // closed-vocabulary prefix, so it must be flagged as
            // `invalid-prefix` (no delimiter present at all).
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    r#"#[ignore = "requires live"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(violations.len(), 1, "got {violations:?}");
            assert!(
                violations[0].snippet.contains("ignore:invalid-prefix"),
                "missing delimiter must be `invalid-prefix`, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn slow_test_wrong_delimiter_rejected() {
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    r#"#[ignore = "slow test file polling"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(violations.len(), 1, "got {violations:?}");
            assert!(
                violations[0].snippet.contains("ignore:invalid-prefix"),
                "missing colon in `slow test` must be `invalid-prefix`, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn requires_live_wrong_delimiter_rejected() {
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    r#"#[ignore = "requires live: Kafka"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(violations.len(), 1, "got {violations:?}");
            assert!(
                violations[0].snippet.contains("ignore:migration-error"),
                "colon instead of space in `requires live:` must be `migration-error`, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn requires_live_is_migration_error() {
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    r#"#[ignore = "requires live Kafka"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(violations.len(), 1, "got {violations:?}");
            assert!(
                violations[0].snippet.contains("ignore:migration-error"),
                "`requires live` must emit migration-error, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn requires_pre_built_wrong_delimiter_rejected() {
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    r#"#[ignore = "requires pre-built: wasm"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(violations.len(), 1, "got {violations:?}");
            assert!(
                violations[0].snippet.contains("ignore:invalid-prefix"),
                "colon instead of space in `requires pre-built:` must be `invalid-prefix`, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        // ----- comment-line / scope handling -----

        #[test]
        fn ignore_in_comment_not_violation() {
            // Both `//` and `//!` lines must be skipped before regex matching,
            // so `#[ignore]` inside a comment is never flagged.
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/src/lib.rs",
                    "// REDIS-009: ... (#[ignore] by default)\n\
                     //! All tests here are #[ignore] by default.\n",
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert!(
                violations.is_empty(),
                "comment lines must be skipped, got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn scripts_dir_excluded() {
            // `scripts/` is in the path-component skip list — file is not
            // scanned at all, so the bare `#[ignore]` is never seen.
            let ws = tmp_workspace_ignore(
                &[("scripts/foo.rs", "#[ignore]\n#[test]\nfn foo() {}\n")],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert!(
                violations.is_empty(),
                "scripts/ must be excluded, got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn tests_dir_scanned() {
            // Unlike `lint_log_levels`, test files under `crates/.../tests/`
            // ARE scanned for `#[ignore]` policy violations. `requires live`
            // is now a migration error (ADR-0054 rev).
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/tests/bar.rs",
                    r#"#[ignore = "requires live Foo"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(violations.len(), 1, "got {violations:?}");
            assert!(
                violations[0].snippet.contains("ignore:migration-error"),
                "tests/ must be scanned; `requires live` should be migration-error, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        // ----- allowlist coupling -----

        #[test]
        fn pre_built_not_in_allowlist() {
            // File has a `requires pre-built` test but is not in the
            // allowlist → forward check violation.
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/foo/tests/bar.rs",
                    r#"#[ignore = "requires pre-built wasm"]
#[test]
fn foo() {}
"#,
                )],
                &[],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert_eq!(
                violations.len(),
                1,
                "expected 1 forward-check violation, got: {violations:?}"
            );
            assert!(
                violations[0]
                    .snippet
                    .contains("ignore:pre-built-not-in-allowlist"),
                "expected `ignore:pre-built-not-in-allowlist`, got: {}",
                violations[0].snippet
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn allowlist_out_of_scope() {
            // Allowlist entry is under `crates/other/tests/...` not the WASM
            // tests dir → reverse-check (a) violation.
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/other/tests/foo.rs",
                    r#"#[ignore = "requires pre-built guest wasm"]
#[test]
fn foo() {}
"#,
                )],
                &["crates/other/tests/foo.rs"],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert!(
                violations
                    .iter()
                    .any(|v| v.snippet.contains("ignore:allowlist-out-of-scope")),
                "expected `ignore:allowlist-out-of-scope`, got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn allowlist_stale() {
            // Allowlist entry points to a file that does not exist on disk
            // → reverse-check (b) violation. Line is the sentinel 0.
            let ws = tmp_workspace_ignore(
                &[],
                &["crates/components/camel-component-wasm/tests/nonexistent.rs"],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert!(
                violations
                    .iter()
                    .any(|v| v.snippet.contains("ignore:allowlist-stale") && v.line == 0),
                "expected `ignore:allowlist-stale` with line 0, got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn allowlist_no_pre_built_test() {
            // Real WASM test file exists, is in the allowlist, but only has
            // `requires live` tests (no `requires pre-built`) → reverse-check
            // (c) violation.
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/components/camel-component-wasm/tests/only_live.rs",
                    r#"#[ignore = "requires live Foo"]
#[test]
fn foo() {}
"#,
                )],
                &["crates/components/camel-component-wasm/tests/only_live.rs"],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert!(
                violations
                    .iter()
                    .any(|v| v.snippet.contains("ignore:allowlist-no-pre-built-test")),
                "expected `ignore:allowlist-no-pre-built-test`, got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }

        #[test]
        fn allowlist_mixed_reasons() {
            // Allowlisted file contains BOTH `requires pre-built` AND
            // `requires live` → `requires live` is now a migration error
            // (ADR-0054 rev), so the mixed-reasons check does NOT fire.
            // Instead, the `requires live` line produces a migration-error
            // violation.
            let ws = tmp_workspace_ignore(
                &[(
                    "crates/components/camel-component-wasm/tests/mixed.rs",
                    r#"#[ignore = "requires pre-built guest wasm"]
#[test]
fn a() {}

#[ignore = "requires live Foo"]
#[test]
fn b() {}
"#,
                )],
                &["crates/components/camel-component-wasm/tests/mixed.rs"],
            );
            let violations = lint_ignore(&ws).unwrap();
            assert!(
                violations
                    .iter()
                    .any(|v| v.snippet.contains("ignore:migration-error")),
                "expected `ignore:migration-error`, got: {violations:?}"
            );
            fs::remove_dir_all(&ws).unwrap();
        }
    }
}

/// Artifact hash: deterministic hash of an OpenSpec change's reviewable
/// artifacts. Used by /bless to bind the expert verdict to exact content,
/// and by /apply to detect drift between blessed and current artifacts.
mod artifact_hash {
    use sha2::{Digest, Sha256};
    use std::path::Path;

    pub fn compute(change_dir: &str) -> Result<String, String> {
        compute_inner(Path::new(change_dir))
    }

    fn compute_inner(change_dir: &Path) -> Result<String, String> {
        if !change_dir.is_dir() {
            return Err(format!("change dir '{}' not found", change_dir.display()));
        }
        let mut files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(change_dir)
            .sort_by_file_name()
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .filter(|p| {
                !matches!(
                    p.file_name().and_then(|n| n.to_str()),
                    Some(".bless.json") | Some(".review.json")
                )
            })
            .collect();
        files.sort();

        let mut hasher = Sha256::new();
        for path in &files {
            let rel = path
                .strip_prefix(change_dir)
                .map_err(|e| format!("strip_prefix: {e}"))?
                .to_string_lossy()
                .replace('\\', "/");
            let raw = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
            let bytes: Vec<u8> = String::from_utf8_lossy(&raw)
                .replace("- [x]", "- [ ]")
                .replace("- [X]", "- [ ]")
                .into_bytes();
            hasher.update(rel.as_bytes());
            hasher.update([0u8]);
            hasher.update(bytes.len().to_le_bytes());
            hasher.update([0u8]);
            hasher.update(&bytes);
        }
        Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn hash_is_deterministic() {
            let dir = tempfile::tempdir().unwrap(); // allow-unwrap
            std::fs::write(dir.path().join("proposal.md"), "hello").unwrap(); // allow-unwrap
            let d = dir.path().to_str().unwrap(); // allow-unwrap
            assert_eq!(compute(d).unwrap(), compute(d).unwrap()); // allow-unwrap
        }

        #[test]
        fn checkbox_normalization() {
            let dir = tempfile::tempdir().unwrap(); // allow-unwrap
            std::fs::write(dir.path().join("tasks.md"), "- [x] done").unwrap(); // allow-unwrap
            let d = dir.path().to_str().unwrap(); // allow-unwrap
            let h1 = compute(d).unwrap(); // allow-unwrap
            std::fs::write(dir.path().join("tasks.md"), "- [ ] done").unwrap(); // allow-unwrap
            assert_eq!(h1, compute(d).unwrap()); // allow-unwrap
        }

        #[test]
        fn content_change_detected() {
            let dir = tempfile::tempdir().unwrap(); // allow-unwrap
            std::fs::write(dir.path().join("spec.md"), "hello").unwrap(); // allow-unwrap
            let d = dir.path().to_str().unwrap(); // allow-unwrap
            let h1 = compute(d).unwrap(); // allow-unwrap
            std::fs::write(dir.path().join("spec.md"), "hello EVIL").unwrap(); // allow-unwrap
            assert_ne!(h1, compute(d).unwrap()); // allow-unwrap
        }
    }
}

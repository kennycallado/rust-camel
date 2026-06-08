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
    Schema,
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
        Commands::Schema => {
            if let Err(e) = generate_schema() {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::LintUnwrap => {
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let workspace_root = find_workspace_root_from(&manifest_dir)
                .ok_or_else(|| "Cannot locate workspace root".to_string())
                .unwrap_or_else(|e| {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                });
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
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let workspace_root = find_workspace_root_from(&manifest_dir)
                .ok_or_else(|| "Cannot locate workspace root".to_string())
                .unwrap_or_else(|e| {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                });
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
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let workspace_root = find_workspace_root_from(&manifest_dir)
                .ok_or_else(|| "Cannot locate workspace root".to_string())
                .unwrap_or_else(|e| {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                });
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
        Commands::PublishOrder { shell } => {
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let workspace_root = find_workspace_root_from(&manifest_dir)
                .ok_or_else(|| "Cannot locate workspace root".to_string())
                .unwrap_or_else(|e| {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                });
            if let Err(e) = publish_order(&workspace_root, shell) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Publish { dry_run } => {
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let workspace_root = find_workspace_root_from(&manifest_dir)
                .ok_or_else(|| "Cannot locate workspace root".to_string())
                .unwrap_or_else(|e| {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                });
            if let Err(e) = publish_crates(&workspace_root, dry_run) {
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
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = find_workspace_root_from(&manifest_dir)
        .ok_or_else(|| {
            "Cannot locate workspace root with bridges/ — are you running from the rust-camel workspace?".to_string()
        })?;

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

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = find_workspace_root_from(&manifest_dir)
        .ok_or_else(|| "Cannot locate workspace root".to_string())?;

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

    let status = Command::new(&cmd)
        .args(&args)
        .current_dir(&bridge_path)
        .env("GRADLE_USER_HOME", bridge_path.join(".gradle-local-cache"))
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
/// `PT_INTERP` in its ELF program headers. Static binaries have no dynamic
/// interpreter segment.
#[cfg(target_os = "linux")]
fn is_static_binary(binary: &Path) -> bool {
    let output = Command::new("readelf")
        .args(["-l", binary.to_str().unwrap_or_default()])
        .output();
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            !stdout.contains("PT_INTERP")
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

fn generate_schema() -> Result<(), String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root =
        find_workspace_root_from(&manifest_dir).ok_or("Cannot locate workspace root")?;
    let schemas_dir = workspace_root.join("schemas");
    std::fs::create_dir_all(&schemas_dir)
        .map_err(|e| format!("Failed to create schemas dir: {e}"))?;

    let schema = schemars::schema_for!(camel_api::CanonicalRouteSpec);
    let schema_json = serde_json::to_string_pretty(&schema)
        .map_err(|e| format!("Failed to serialize schema: {e}"))?;
    let schema_path = schemas_dir.join("canonical-route-spec.json");
    std::fs::write(&schema_path, &schema_json)
        .map_err(|e| format!("Failed to write schema: {e}"))?;
    println!("Generated: {}", schema_path.display());

    let ts_dir = schemas_dir.join("ts");
    std::fs::create_dir_all(&ts_dir).map_err(|e| format!("Failed to create ts dir: {e}"))?;

    let ts_config = ts_rs::Config::new().with_out_dir(&ts_dir);
    <camel_api::CanonicalRouteSpec as ts_rs::TS>::export(&ts_config)
        .map_err(|e| format!("Failed to export CanonicalRouteSpec: {e}"))?;
    <camel_api::runtime::CanonicalStepSpec as ts_rs::TS>::export(&ts_config)
        .map_err(|e| format!("Failed to export CanonicalStepSpec: {e}"))?;
    <camel_api::runtime::CanonicalWhenSpec as ts_rs::TS>::export(&ts_config)
        .map_err(|e| format!("Failed to export CanonicalWhenSpec: {e}"))?;
    <camel_api::runtime::CanonicalCircuitBreakerSpec as ts_rs::TS>::export(&ts_config)
        .map_err(|e| format!("Failed to export CanonicalCircuitBreakerSpec: {e}"))?;
    <camel_api::runtime::CanonicalSplitExpressionSpec as ts_rs::TS>::export(&ts_config)
        .map_err(|e| format!("Failed to export CanonicalSplitExpressionSpec: {e}"))?;
    <camel_api::runtime::CanonicalSplitAggregationSpec as ts_rs::TS>::export(&ts_config)
        .map_err(|e| format!("Failed to export CanonicalSplitAggregationSpec: {e}"))?;
    <camel_api::runtime::CanonicalAggregateStrategySpec as ts_rs::TS>::export(&ts_config)
        .map_err(|e| format!("Failed to export CanonicalAggregateStrategySpec: {e}"))?;
    <camel_api::runtime::CanonicalAggregateSpec as ts_rs::TS>::export(&ts_config)
        .map_err(|e| format!("Failed to export CanonicalAggregateSpec: {e}"))?;
    <camel_api::declarative::LanguageExpressionDef as ts_rs::TS>::export(&ts_config)
        .map_err(|e| format!("Failed to export LanguageExpressionDef: {e}"))?;
    println!("Generated TypeScript types in: {}", ts_dir.display());

    println!("Done! Run `quicktype` manually for Go/Python types.");
    Ok(())
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
    // Exclude files under a `tests/` directory (portable, no string slicing)
    path.components().any(|c| c == Component::Normal("tests".as_ref()))
        // Exclude test_*.rs, *_test.rs, *_tests.rs
        || name.starts_with("test_")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        // Exclude build scripts
        || name == "build.rs"
}

/// Scan all workspace `src/**/*.rs` files (excluding test files and build.rs)
/// for `.unwrap()` and `.expect(` calls not marked with `// allow-unwrap`.
///
/// NOTE: This is a lexical scanner. UFCS forms like `Option::unwrap(x)` are
/// not caught. They are rare in this codebase; add `// allow-unwrap` if needed.
pub fn lint_unwrap(workspace_root: &Path) -> Result<Vec<Violation>, String> {
    use regex::Regex;
    use std::path::Component;
    use walkdir::WalkDir;

    let unwrap_re = Regex::new(r"\.(unwrap\(\)|expect\()").expect("valid regex"); // allow-unwrap
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

        // Collect into a Vec so we can do one-line look-ahead for `// allow-unwrap`
        // placed on the next line by rustfmt (e.g. after opening `{` on an expect call).
        let lines: Vec<&str> = content.lines().collect();

        // Use a pending-attribute approach to correctly handle both:
        //   #[cfg(test)] mod tests { ... }  — a module block
        //   #[test] fn test_foo() { ... }   — an individual test function
        // Both open a braced scope, but they must not interfere with each other.
        let mut pending_test_attr = false;
        let mut test_scope_entry_depth: Option<i32> = None;
        let mut brace_depth: i32 = 0;

        for (line_idx, raw_line) in lines.iter().enumerate() {
            let trimmed = raw_line.trim();

            // Detect test attributes only when not already inside a test scope.
            // Updating test_mod_depth while already in a scope would cause premature
            // exit when the inner function closes (the nested-#[test] bug).
            if test_scope_entry_depth.is_none()
                && (trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[test]"))
            {
                pending_test_attr = true;
            }

            // Capture whether this line opens a test scope BEFORE counting braces.
            // This handles the case where #[test] and fn body are on separate lines
            // and the body's braces open+close on the same line.
            let entering_test_scope = pending_test_attr && test_scope_entry_depth.is_none();

            // Count braces on this line.
            // NOTE: This is a lexical approximation — braces inside string literals
            // or comments will affect the count. This is acceptable for a build-time
            // scanner as long as unusual cases can be suppressed with // allow-unwrap.
            for ch in trimmed.chars() {
                match ch {
                    '{' => {
                        brace_depth += 1;
                        // The first '{' after a test attribute opens the test scope.
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

            // If we set pending_test_attr on this line but no brace was opened,
            // the attribute applies to a non-block item (e.g. `type Foo = ...;`
            // or `use super::*;`).  Clear the flag so it does not bleed into the
            // next line's production code.
            if pending_test_attr && test_scope_entry_depth.is_none() && trimmed.contains(';') {
                pending_test_attr = false;
            }

            // Skip: the attribute line itself, the line that opens a test scope,
            // and all lines inside a test scope.
            if pending_test_attr || entering_test_scope || test_scope_entry_depth.is_some() {
                continue;
            }
            // Skip comment-only lines.
            if trimmed.starts_with("//") {
                continue;
            }
            // Skip lines with the escape hatch — also check the next line because
            // rustfmt sometimes moves `// allow-unwrap` onto the line after `expect(`
            // when the call opens a block (`= RunnerState::Failed { // allow-unwrap`
            // becomes the body's first line after fmt).
            let next_line_allow = lines
                .get(line_idx + 1)
                .map(|l| l.trim() == "// allow-unwrap")
                .unwrap_or(false);
            if raw_line.contains("// allow-unwrap") || next_line_allow {
                continue;
            }

            if unwrap_re.is_match(raw_line) {
                violations.push(Violation {
                    file: path.to_string_lossy().to_string(),
                    line: line_idx + 1,
                    snippet: raw_line.to_string(),
                });
            }
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

                // Skip comment-only lines
                if !first_line.trim().starts_with("//") && !first_line.contains("// allow-secret") {
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

    Ok(violations)
}

/// Represents a workspace crate with its publish-relevant metadata.
#[derive(Clone)]
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum EdgeKind {
    Normal,
    Weak,
}

/// Discover workspace crates and compute topological publish order.
fn resolve_publish_order(workspace_root: &Path) -> Result<Vec<WorkspaceCrate>, String> {
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

    // Build adjacency with edge-kind tagging. We need to track edges by kind
    // so we can break weak-only cycles after Kahn's algorithm stalls.
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
    while let Some(ci) = queue.pop_front() {
        sorted.push(ci);
        for &(dependent, _kind) in &adj[ci] {
            in_degree[dependent] -= 1;
            if in_degree[dependent] == 0 {
                queue.push_back(dependent);
            }
        }
    }

    // If Kahn stalled, try breaking weak edges that participate in cycles.
    // Each broken weak edge means the dependent must be published with
    // `cargo publish --no-verify` (or the dev-dep restructured), because
    // cargo cannot resolve it at publish time.
    let mut broken_weak_edges: Vec<(String, String)> = Vec::new();
    while sorted.len() < publishable.len() {
        let sorted_set: std::collections::HashSet<usize> = sorted.iter().copied().collect();

        // Find an unscheduled crate whose remaining in-degree comes entirely
        // from weak edges whose source is also unscheduled. Dropping one such
        // edge breaks at least one cycle.
        let mut progress = false;
        for &ci in &publishable {
            if sorted_set.contains(&ci) || in_degree[ci] == 0 {
                continue;
            }
            // Count how many of ci's remaining unresolved incoming edges are
            // weak and come from other unscheduled crates.
            let weak_unresolved: Vec<usize> = adj
                .iter()
                .enumerate()
                .filter_map(|(di, dependents)| {
                    if sorted_set.contains(&di) {
                        return None;
                    }
                    dependents
                        .iter()
                        .any(|&(d, k)| d == ci && k == EdgeKind::Weak)
                        .then_some(di)
                })
                .collect();

            if weak_unresolved.is_empty() {
                continue;
            }
            // Drop the first such weak edge. Pick the source with the smallest
            // index for deterministic output. `weak_unresolved` is guaranteed
            // non-empty here because we skipped empty cases above.
            let di = *weak_unresolved.iter().min().unwrap(); // allow-unwrap
            adj[di].retain(|&(d, _)| d != ci);
            in_degree[ci] -= 1;
            broken_weak_edges.push((crates[di].name.clone(), crates[ci].name.clone()));
            progress = true;
            if in_degree[ci] == 0 {
                queue.push_back(ci);
            }
        }

        if !progress {
            break;
        }

        // Drain the queue we may have just refilled.
        while let Some(ci) = queue.pop_front() {
            sorted.push(ci);
            for &(dependent, _kind) in &adj[ci] {
                in_degree[dependent] -= 1;
                if in_degree[dependent] == 0 {
                    queue.push_back(dependent);
                }
            }
        }
    }

    if sorted.len() != publishable.len() {
        let sorted_set: std::collections::HashSet<usize> = sorted.iter().copied().collect();
        eprintln!(
            "⚠️  CYCLE! Only sorted {} of {} publishable crates.",
            sorted.len(),
            publishable.len()
        );
        for &ci in &publishable {
            if !sorted_set.contains(&ci) {
                eprintln!("  {} (in-degree: {})", crates[ci].name, in_degree[ci]);
            }
        }
        return Err("Cannot compute publish order due to dependency cycles".to_string());
    }

    if !broken_weak_edges.is_empty() {
        eprintln!(
            "⚠️  Broke {} weak (dev/build) dependency edge(s) to resolve cycles:",
            broken_weak_edges.len()
        );
        for (from, to) in &broken_weak_edges {
            eprintln!(
                "  {from} --dev/build-dep--> {to} (publish {to} first; verify it does not need {from} at publish time)"
            );
        }
        eprintln!(
            "  If `cargo publish` fails for any of these, publish the affected crate manually with --no-verify."
        );
    }

    Ok(sorted.into_iter().map(|i| crates[i].clone()).collect())
}

fn publish_order(workspace_root: &Path, shell: bool) -> Result<(), String> {
    let sorted = resolve_publish_order(workspace_root)?;

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
    }

    Ok(())
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
    let sorted = resolve_publish_order(workspace_root)?;
    let version = workspace_version(workspace_root)?;

    println!("📦 Publishing rust-camel crates v{version} to crates.io");
    println!("=============================================");

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

        let output = std::process::Command::new("cargo")
            .args(["publish", "--allow-dirty"])
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

/// Returns true for `[dev-dependencies]`, `[build-dependencies]` and their
/// target-specific variants — sections cargo resolves during `cargo publish`
/// but which are weaker constraints than `[dependencies]` (cycles closed
/// only by these edges can be broken at publish time).
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
}

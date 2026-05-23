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

    // 4. Ensure the Gradle cache dir exists and is world-writable so the container
    //    user (quarkus/1001) can write to it regardless of the host user's uid.
    let cache_dir = bridge_dir.join(".gradle-docker-cache");
    if !cache_dir.exists() {
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create Gradle cache dir: {e}"))?;
    }
    // Make only the directories the container writes to world-writable.
    // The container user (quarkus/1001) needs write access to build/ and
    // .gradle-docker-cache/, but NOT to the entire source tree.
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
    //    On Unix, run as the host uid:gid to avoid chmod/ownership issues on
    //    bind-mounted Gradle cache directories.
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--network=host".to_string(),
        format!("--volume={}:/project:z", bridge_dir.display()),
        "--workdir=/project".to_string(),
        "--env=GRADLE_USER_HOME=/project/.gradle-docker-cache".to_string(),
        "--entrypoint".to_string(),
        "bash".to_string(),
    ];

    #[cfg(unix)]
    {
        let (uid, gid) = host_uid_gid()?;
        args.push(format!("--user={uid}:{gid}"));
    }

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

#[cfg(unix)]
fn host_uid_gid() -> Result<(String, String), String> {
    let uid = Command::new("id")
        .args(["-u"])
        .output()
        .map_err(|e| format!("Failed to resolve uid with `id -u`: {e}"))?;
    if !uid.status.success() {
        return Err(format!(
            "`id -u` failed with exit code {}",
            uid.status.code().unwrap_or(-1)
        ));
    }

    let gid = Command::new("id")
        .args(["-g"])
        .output()
        .map_err(|e| format!("Failed to resolve gid with `id -g`: {e}"))?;
    if !gid.status.success() {
        return Err(format!(
            "`id -g` failed with exit code {}",
            gid.status.code().unwrap_or(-1)
        ));
    }

    let uid = String::from_utf8_lossy(&uid.stdout).trim().to_string();
    let gid = String::from_utf8_lossy(&gid.stdout).trim().to_string();
    if uid.is_empty() || gid.is_empty() {
        return Err("Resolved empty uid/gid from id command".to_string());
    }

    Ok((uid, gid))
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
}

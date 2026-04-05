use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, Subcommand};

const MANDREL_IMAGE: &str = "quay.io/quarkus/ubi9-quarkus-mandrel-builder-image:jdk-21";
const EXPECTED_BINARY: &str = "build/native/jms-bridge";

#[derive(Parser)]
#[command(name = "xtask", about = "rust-camel build tasks")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the JMS bridge native binary using Docker (Mandrel)
    BuildJmsBridge {
        /// Version tag to pass to build-native.sh (e.g. 0.2.0)
        #[arg(long)]
        version: Option<String>,
        /// Clear Gradle cache before building
        #[arg(long)]
        no_cache: bool,
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
    }
}

fn validate_version(v: &str) -> Result<(), String> {
    let re = regex::Regex::new(r"^(dev|[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?)$").unwrap();
    if !re.is_match(v) {
        return Err(format!(
            "Invalid version '{v}' — must be 'dev' or semver pattern MAJOR.MINOR.PATCH[-PRERELEASE]"
        ));
    }
    Ok(())
}

fn build_jms_bridge(version: Option<String>, no_cache: bool) -> Result<(), String> {
    // Validate version early to prevent path traversal or malformed filenames
    if let Some(ref v) = version {
        validate_version(v)?;
    }

    // 1. Locate workspace root
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = find_workspace_root_from(&manifest_dir)
        .ok_or_else(|| {
            "Cannot locate workspace root with bridges/jms/ — are you running from the rust-camel workspace?".to_string()
        })?;

    let bridges_jms = workspace_root.join("bridges").join("jms");

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
        let cache_dir = bridges_jms.join(".gradle-docker-cache");
        if cache_dir.exists() {
            std::fs::remove_dir_all(&cache_dir)
                .map_err(|e| format!("Failed to clear Gradle cache: {e}"))?;
            println!("Cleared Gradle cache at {}", cache_dir.display());
        }
    }

    // 4. Ensure the Gradle cache dir exists and is world-writable so the container
    //    user (quarkus/1001) can write to it regardless of the host user's uid.
    let cache_dir = bridges_jms.join(".gradle-docker-cache");
    if !cache_dir.exists() {
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create Gradle cache dir: {e}"))?;
    }
    // Make only the directories the container writes to world-writable.
    // The container user (quarkus/1001) needs write access to build/ and
    // .gradle-docker-cache/, but NOT to the entire source tree.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let build_dir = bridges_jms.join("build");
        if !build_dir.exists() {
            std::fs::create_dir_all(&build_dir)
                .map_err(|e| format!("Failed to create build dir: {e}"))?;
        }
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
        format!("--volume={}:/project:z", bridges_jms.display()),
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

    args.push(MANDREL_IMAGE.to_string());

    // build-native.sh args
    args.push("./build-native.sh".to_string());
    args.push("--in-container".to_string());
    if let Some(ref v) = version {
        args.push("--version".to_string());
        args.push(v.clone());
    }

    println!("Building JMS bridge native image...");
    println!("  Image:     {MANDREL_IMAGE}");
    println!("  Source:    {}", bridges_jms.display());
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
    let binary_path = bridges_jms.join(EXPECTED_BINARY);
    if !binary_path.exists() {
        return Err(format!(
            "Build succeeded but binary not found at expected path: {}",
            binary_path.display()
        ));
    }

    // 6. On NixOS, patch the glibc-linked binary so it uses the Nix-store
    //    linker and libraries. This makes the binary runnable without
    //    enabling nix-ld at the system level.
    #[cfg(target_os = "linux")]
    patchelf_for_nixos(&binary_path)?;

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

/// On NixOS, the Mandrel native binary is linked against glibc with
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
            .args(["chmod", "a+w", binary.to_str().unwrap()])
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
        .args(["--set-interpreter", &interpreter, binary.to_str().unwrap()])
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
        .args(["--set-rpath", &rpath, binary.to_str().unwrap()])
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
/// with a `bridges/jms/` directory as sentinel. Returns the workspace root.
pub fn find_workspace_root_from(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    for _ in 0..10 {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists()
            && std::fs::read_to_string(&cargo_toml)
                .map(|contents| contents.contains("[workspace]"))
                .unwrap_or(false)
            && current.join("bridges").join("jms").exists()
        {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn find_workspace_root_finds_sentinel() {
        let dir = std::env::temp_dir().join("xtask-test-ws");
        let bridges_jms = dir.join("bridges").join("jms");
        fs::create_dir_all(&bridges_jms).unwrap();
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
        // No bridges/jms/ directory

        let result = find_workspace_root_from(&sub);
        assert_eq!(result, None);

        fs::remove_dir_all(&dir).unwrap();
    }
}

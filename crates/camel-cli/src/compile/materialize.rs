//! Confined per-boot materialization of substitution-targeted assets
//! (openspec change `r2embed`, Task 3.2; bd `rc-0ks57` file-ancestor
//! confinement discipline).
//!
//! Legacy path readers (TLS cert/key/CA consumers, `xslt:`, `validator:`,
//! `sql:file:`) need a real file. Materialization writes exactly the
//! substitution-targeted asset bytes into a random per-boot directory
//! under [`std::env::temp_dir`]:
//!
//! - the directory is created exclusively at mode 0700 (private to the
//!   booting process);
//! - every file is written exactly once with `create_new` at mode 0600 —
//!   an existing entry or a planted symlink at the target path fails the
//!   write instead of being followed;
//! - the nearest-existing ancestor of every target is re-checked after
//!   each created component, so a component planted between the checks
//!   cannot redirect the write outside the per-boot directory;
//! - dropping [`Materialization`] removes the directory (best effort,
//!   like any temp cleanup).
//!
//! Residue caveat, documented not hidden: `SIGKILL`, `process::exit`, or
//! the in-tree force-exit on a second stop signal (`commands/run.rs:691`,
//! `commands/job/signal.rs:82`) bypasses the drop guard and leaves the
//! per-boot directory behind in the OS temp directory. Operators who need
//! a stronger guarantee get it when these classes flip to a bytes seam.
//!
//! Only assets referenced by materialized consumers are written — never
//! the whole store. Static-file entries stay memory-served through the
//! asset registry and never touch the filesystem.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Materialization failure. Every variant names the offending path;
/// callers surface the diagnostic and exit 2 before boot.
#[derive(Debug)]
pub enum MaterializeError {
    /// The per-boot directory or a file write failed at the OS level.
    Io {
        /// Path the failed operation targeted.
        path: PathBuf,
        /// Underlying OS error.
        source: std::io::Error,
    },
    /// The nearest-existing ancestor of a write target is not the
    /// per-boot directory: a component was planted, is a symlink, or the
    /// target escapes the directory. The write never happened.
    NotConfined {
        /// Offending component path (the nearest existing ancestor).
        path: PathBuf,
    },
    /// The relative asset path is unusable (absolute, empty, or carries
    /// a `..` component). Defensive: store paths are validated at
    /// compile time and decode.
    InvalidRelativePath {
        /// The rejected relative path.
        relative: String,
    },
}

impl fmt::Display for MaterializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "materialization failed at {}: {source}", path.display())
            }
            Self::NotConfined { path } => write!(
                f,
                "materialization confined write rejected: {} is not inside the per-boot \
                 directory (planted entry or symlink)",
                path.display()
            ),
            Self::InvalidRelativePath { relative } => {
                write!(
                    f,
                    "materialization invalid relative asset path: {relative:?}"
                )
            }
        }
    }
}

impl std::error::Error for MaterializeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Owns one per-boot confined directory. Created exclusively at mode
/// 0700 under [`std::env::temp_dir`]; dropping the value removes the
/// directory with everything written inside it.
#[derive(Debug)]
pub struct Materialization {
    dir: tempfile::TempDir,
}

impl Materialization {
    /// Creates the per-boot directory under [`std::env::temp_dir`] with a
    /// random name, exclusively, at mode 0700. The only failure mode is
    /// the OS-level directory creation (the read-only contract's
    /// missing-writable-temp case).
    pub fn create() -> std::io::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("camel-assets-")
            .tempdir_in(std::env::temp_dir())?;
        // Affirm the private mode explicitly instead of relying on the
        // tempfile default, so the contract holds regardless of the
        // crate version in the lockfile.
        #[cfg(unix)]
        {
            let _ = std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700));
        }
        Ok(Self { dir })
    }

    /// The per-boot directory path.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The confined absolute path of a relative asset path inside the
    /// per-boot directory. Pure computation: no filesystem access, so
    /// callers can run the substitution path-safety rule on the result
    /// before anything is written.
    pub fn confined_path(&self, relative: &str) -> Result<PathBuf, MaterializeError> {
        let rel = validated_relative(relative)?;
        Ok(self.dir.path().join(rel))
    }

    /// Writes one asset's bytes to `relative` inside the per-boot
    /// directory: missing parent components are created one by one (each
    /// creation re-checks confinement), and the file itself is opened
    /// with `create_new` at mode 0600 — an existing entry or symlink at
    /// the target path fails the write instead of being followed.
    pub fn write(&self, relative: &str, bytes: &[u8]) -> Result<PathBuf, MaterializeError> {
        let rel = validated_relative(relative)?;
        let full = self.dir.path().join(rel);
        // Confinement holds before anything is created: the nearest
        // existing ancestor must be the per-boot directory itself.
        ensure_confinement(self.dir.path(), &full)?;
        // Create missing parent components top-down, re-checking
        // confinement after every created component (bd rc-0ks57).
        let mut missing = Vec::new();
        let mut cursor = full.parent().map(Path::to_path_buf);
        while let Some(path) = cursor {
            if path == self.dir.path() {
                break;
            }
            // A component that already exists is a real directory:
            // `ensure_confinement` verified every existing component
            // below the per-boot directory, so a shared subdirectory
            // created by an earlier write needs no re-creation —
            // stop collecting there instead of EEXIST-ing later.
            if std::fs::symlink_metadata(&path).is_ok() {
                break;
            }
            missing.push(path.clone());
            cursor = path.parent().map(Path::to_path_buf);
        }
        for component in missing.into_iter().rev() {
            std::fs::create_dir(&component).map_err(|source| MaterializeError::Io {
                path: component.clone(),
                source,
            })?;
            ensure_confinement(self.dir.path(), &full)?;
        }
        // Exclusive create: an existing entry — including a planted
        // symlink — fails here instead of being followed.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode_0600()
            .open(&full)
            .map_err(|source| MaterializeError::Io {
                path: full.clone(),
                source,
            })?;
        file.write_all(bytes)
            .map_err(|source| MaterializeError::Io {
                path: full.clone(),
                source,
            })?;
        Ok(full)
    }
}

/// Rejects an unusable relative asset path (absolute, empty, or carrying
/// a `..` component).
fn validated_relative(relative: &str) -> Result<&Path, MaterializeError> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(MaterializeError::InvalidRelativePath {
            relative: relative.to_string(),
        });
    }
    Ok(path)
}

/// Checks the confinement of `target`'s ancestor chain: every EXISTING
/// ancestor between the target and `dir` must be a real directory (never
/// a symlink), and the walk must terminate at `dir` itself, also a real
/// directory. The target leaf is deliberately not examined here — the
/// exclusive `create_new` open is its own protection (an existing entry
/// or symlink at the leaf fails the open instead of being followed).
fn ensure_confinement(dir: &Path, target: &Path) -> Result<(), MaterializeError> {
    let mut current: &Path = target;
    while let Some(parent) = current.parent() {
        current = parent;
        match std::fs::symlink_metadata(current) {
            Ok(meta) => {
                let real_dir = !meta.file_type().is_symlink() && meta.is_dir();
                if current == dir {
                    return if real_dir {
                        Ok(())
                    } else {
                        Err(MaterializeError::NotConfined {
                            path: current.to_path_buf(),
                        })
                    };
                }
                if !real_dir {
                    return Err(MaterializeError::NotConfined {
                        path: current.to_path_buf(),
                    });
                }
                // A real directory inside the per-boot dir: keep walking
                // toward `dir`.
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Component not created yet: keep walking toward `dir`.
            }
            Err(source) => {
                return Err(MaterializeError::Io {
                    path: current.to_path_buf(),
                    source,
                });
            }
        }
    }
    // Walked above `dir` without reaching it: the target escapes the
    // per-boot directory.
    Err(MaterializeError::NotConfined {
        path: target.to_path_buf(),
    })
}

/// Unix-only file mode 0600 on the create_new open.
trait Mode0600 {
    fn mode_0600(&mut self) -> &mut Self;
}

#[cfg(unix)]
impl Mode0600 for std::fs::OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }
}

#[cfg(not(unix))]
impl Mode0600 for std::fs::OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        self
    }
}

// ---------------------------------------------------------------------------
// Tests. The blessed test lives at MODULE level (not in a nested `mod
// tests`) so the mandated filter command
// `cargo test -p camel-cli --lib compile::materialize::materialize_confines_writes_and_cleans_up`
// matches exactly.
// ---------------------------------------------------------------------------

/// Confinement and cleanup: files land only under the per-boot directory
/// at mode 0600 (directory 0700), a planted symlink at a target path is
/// never followed (`create_new` failure surfaces), a symlinked directory
/// component is rejected as unconfined, and dropping the guard removes
/// the directory (r2embed Task 3.2).
#[cfg(test)]
#[test]
fn materialize_confines_writes_and_cleans_up() {
    let mat = Materialization::create().expect("per-boot directory creates");
    let dir = mat.path().to_path_buf();
    assert!(dir.is_dir(), "per-boot directory exists");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir)
            .expect("per-boot directory metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "per-boot directory must be 0700");
    }

    // A plain file: mode 0600, exact bytes.
    let written = mat
        .write("certs/svc.crt", b"materialized-bytes\n")
        .expect("plain write succeeds");
    assert_eq!(
        written,
        dir.join("certs").join("svc.crt"),
        "write target stays under the per-boot directory"
    );
    assert_eq!(
        std::fs::read(&written).expect("written bytes"),
        b"materialized-bytes\n",
        "materialized bytes are exact"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&written)
            .expect("written file metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "materialized file must be 0600");
    }

    // A symlink planted AT the target path is never followed: the
    // exclusive create fails and the symlink target is untouched.
    let victim = std::env::temp_dir().join("camel-materialize-no-follow-victim");
    let _ = std::fs::remove_file(&victim);
    std::fs::write(&victim, b"victim\n").expect("plant victim file");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&victim, dir.join("evil.bin"))
            .expect("plant symlink at target path");
        let err = mat
            .write("evil.bin", b"must-not-be-written\n")
            .expect_err("create_new must fail on the planted symlink");
        assert!(
            err.to_string().contains("evil.bin"),
            "failure names the target: {err}"
        );
        assert_eq!(
            std::fs::read(&victim).expect("victim stays intact"),
            b"victim\n",
            "the symlink was never followed"
        );
    }

    // A symlinked directory COMPONENT is rejected as unconfined and
    // nothing is written through it.
    #[cfg(unix)]
    {
        let outside = std::env::temp_dir().join("camel-materialize-outside");
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).expect("plant outside directory");
        std::os::unix::fs::symlink(&outside, dir.join("linkdir"))
            .expect("plant symlinked directory component");
        let err = mat
            .write("linkdir/escape.bin", b"escape\n")
            .expect_err("symlinked component must be rejected");
        assert!(
            err.to_string().contains("per-boot"),
            "failure names the confinement rule: {err}"
        );
        assert!(
            !outside.join("escape.bin").exists(),
            "nothing is written through the symlinked component"
        );
        let _ = std::fs::remove_dir_all(&outside);
    }

    // Cleanup: dropping the guard removes the directory.
    drop(mat);
    assert!(
        !dir.exists(),
        "dropping the guard removes the per-boot directory"
    );
    let _ = std::fs::remove_file(&victim);
}

/// A shared subdirectory created by an earlier write must not be
/// re-created by a later write: two files under one `sub/a/` parent both
/// materialize, both stay confined, and dropping the guard removes both
/// (r2embed Mission 243 regression: the missing-parent walk stops at the
/// first EXISTING component instead of collecting it and EEXIST-ing on
/// the re-created parent). Module-level so the mandated
/// `compile::materialize` filter matches it.
#[cfg(test)]
#[test]
fn materialize_shared_subdirectory_second_write_succeeds() {
    let mat = Materialization::create().expect("per-boot directory creates");
    let dir = mat.path().to_path_buf();

    let first = mat
        .write("sub/a/b1.txt", b"first\n")
        .expect("first write under a fresh shared subdirectory succeeds");
    assert_eq!(
        first,
        dir.join("sub").join("a").join("b1.txt"),
        "the first write stays under the per-boot directory"
    );
    assert_eq!(
        std::fs::read(&first).expect("first written bytes"),
        b"first\n",
        "the first file's bytes are exact"
    );

    // Second write into the SAME `sub/a/` directory. Red on the pre-fix
    // walk: it collected the now-existing `sub/a` (and `sub`) as missing,
    // then `create_dir` on the existing parent returned `File exists`.
    let second = mat
        .write("sub/a/b2.txt", b"second\n")
        .expect("second write into the shared subdirectory succeeds");
    assert_eq!(
        second,
        dir.join("sub").join("a").join("b2.txt"),
        "the second write stays under the per-boot directory"
    );
    assert_eq!(
        std::fs::read(&second).expect("second written bytes"),
        b"second\n",
        "the second file's bytes are exact"
    );
    assert!(
        first.starts_with(&dir) && second.starts_with(&dir),
        "both shared-subdirectory writes are confined under the per-boot directory"
    );

    // Cleanup removes the whole shared tree, not just the leaf files.
    drop(mat);
    assert!(
        !dir.exists(),
        "dropping the guard removes the shared subdirectory tree"
    );
}

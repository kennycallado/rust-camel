//! `camel compile <document> -o <artifact>` — build a self-contained
//! native executable artifact (openspec change `cli-compile`).
//!
//! The command captures one document as raw text BEFORE `${env:}`
//! interpolation, validates the v1 asset allowlist
//! ([`crate::compile::policy`]), derives the operational manifest, and only
//! then writes `<output>.tmp` (executable copy + trailer) and renames it
//! onto the output path. Every rejection — non-native target, dirty compile
//! environment, `Camel.toml` in the working directory, invalid UTF-8,
//! oversize payload, unsupported asset, unparsable document — exits 2 with
//! a named diagnostic; failures never delete or truncate a pre-existing
//! output and never leave a usable partial artifact.
//!
//! Trust model: the artifact is a copy of the Camel executable plus
//! authoring text; deployment owns process capabilities. v1 supports the
//! native Linux target only.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::Args;

use crate::compile::manifest;
use crate::compile::policy;
use crate::compile::trailer::{self, Trailer, TrailerKind};

/// Exit code for every named compile rejection.
const EXIT_REJECTION: i32 = 2;

/// CLI args for `camel compile`.
#[derive(Args, Debug)]
pub struct CompileArgs {
    /// Single route document (`*.yaml`/`*.yml`/`*.json`) or job document
    /// (`*.job.yaml`/`*.job.yml`) to embed.
    #[arg(value_name = "DOCUMENT")]
    pub document: PathBuf,

    /// Path of the executable artifact to write.
    #[arg(short, long, value_name = "ARTIFACT")]
    pub output: PathBuf,

    /// Requested target triple; v1 accepts the native Linux triple only.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,
}

/// Native target triple of this executable (`<arch>-unknown-linux-<libc>`).
fn native_triple() -> String {
    let libc = if cfg!(target_env = "musl") {
        "musl"
    } else {
        "gnu"
    };
    format!("{}-unknown-linux-{}", std::env::consts::ARCH, libc)
}

/// Run `camel compile`. Diagnostics go to stderr; the return value is the
/// process exit code (0 success, 2 named rejection).
pub fn run_compile(args: &CompileArgs) -> i32 {
    // Native-Linux-only gate: v1 has no cross-compilation path.
    if !cfg!(target_os = "linux") {
        eprintln!(
            "camel compile v1 supports native Linux only; this host is {}",
            std::env::consts::OS
        );
        return EXIT_REJECTION;
    }
    if let Some(requested) = &args.target
        && *requested != native_triple()
    {
        eprintln!(
            "camel compile v1 supports native Linux only: target '{requested}' does not match the native target '{}'",
            native_triple()
        );
        return EXIT_REJECTION;
    }

    // Clean compile environment: a CAMEL_* override would silently change
    // what the artifact embeds, so none may be present.
    let overrides: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _)| {
            let name = name.to_string_lossy();
            name.starts_with("CAMEL_").then(|| name.into_owned())
        })
        .collect();
    if !overrides.is_empty() {
        eprintln!(
            "camel compile v1 requires a clean compile environment; CAMEL_* variable(s) present: {}",
            overrides.join(", ")
        );
        return EXIT_REJECTION;
    }

    // No Camel.toml in the compile working directory: compiled documents
    // must be self-contained.
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("camel compile: cannot determine the working directory: {e}");
            return EXIT_REJECTION;
        }
    };
    if cwd.join("Camel.toml").is_file() {
        eprintln!(
            "camel compile v1 rejects a Camel.toml in the compile working directory ('{}'); compiled documents must be self-contained",
            cwd.display()
        );
        return EXIT_REJECTION;
    }

    // Capture raw bytes, then normalize before any interpolation.
    let raw = match std::fs::read(&args.document) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!(
                "camel compile: cannot read document '{}': {e}",
                args.document.display()
            );
            return EXIT_REJECTION;
        }
    };
    let text = match trailer::normalize_document(&raw) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("camel compile: {e}");
            return EXIT_REJECTION;
        }
    };

    // `.job.json` is not a compilable document kind: reject it explicitly
    // instead of silently compiling the file as a route document.
    let doc_name = args
        .document
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if doc_name.ends_with(".job.json") {
        eprintln!(
            "camel compile: unsupported document '{}': the '.job.json' suffix is not a compilable document kind; job documents must be '*.job.yaml' or '*.job.yml'",
            args.document.display()
        );
        return EXIT_REJECTION;
    }

    // Identify the single-document kind from the input suffix.
    let Some(kind) = document_kind(&args.document) else {
        eprintln!(
            "camel compile: unsupported document '{}': expected a route document (*.yaml, *.yml, *.json) or a job document (*.job.yaml, *.job.yml)",
            args.document.display()
        );
        return EXIT_REJECTION;
    };

    // Fail-closed asset policy, then manifest derivation — all before any
    // output byte exists.
    if let Err(e) = policy::reject_unsupported_assets(&text, kind) {
        eprintln!("camel compile: {e}");
        return EXIT_REJECTION;
    }
    let manifest = match manifest::derive(&source_name(&args.document, &cwd), kind, &text) {
        Ok(manifest) => manifest,
        Err(e) => {
            eprintln!("camel compile: {e}");
            return EXIT_REJECTION;
        }
    };

    // All validation succeeded: copy the current executable and append the
    // trailer as the only write phase.
    let artifact = Trailer {
        kind,
        payload: text.into_bytes(),
        manifest: manifest.to_canonical_json().into_bytes(),
    };
    if let Err(e) = write_artifact(&args.output, &trailer::encode(&artifact)) {
        eprintln!("camel compile: {e}");
        return EXIT_REJECTION;
    }
    0
}

/// Route/job kind from the document suffix; `None` for unsupported names.
fn document_kind(path: &Path) -> Option<TrailerKind> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".job.yaml") || name.ends_with(".job.yml") {
        Some(TrailerKind::Job)
    } else if name.ends_with(".yaml") || name.ends_with(".yml") || name.ends_with(".json") {
        Some(TrailerKind::Route)
    } else {
        None
    }
}

/// Logical source name: the input path relative to the compile working
/// directory (as given for relative inputs, absolute inputs stripped of
/// the cwd prefix), without `.` components.
fn source_name(document: &Path, cwd: &Path) -> String {
    let rel = if document.is_absolute() {
        document.strip_prefix(cwd).unwrap_or(document)
    } else {
        document
    };
    let cleaned: PathBuf = rel
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect();
    cleaned.to_string_lossy().into_owned()
}

/// Copy `current_exe()` plus the encoded trailer into `output` atomically.
/// Both are written to the sibling `<output>.tmp` first, flushed to disk,
/// and only the complete temp file is renamed onto `output`. On any failure
/// the temp file is removed and any pre-existing output is left untouched —
/// a rejected or failed compile never deletes or truncates an existing
/// artifact, and no partial artifact is ever visible at `output`.
fn write_artifact(output: &Path, trailer_bytes: &[u8]) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot locate the current executable: {e}"))?;
    let mut tmp_name = output.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);

    /// Remove the temp file this function owns; `output` is never touched.
    fn discard(tmp: &Path, message: String) -> String {
        let _ = std::fs::remove_file(tmp);
        message
    }

    let write = || -> std::io::Result<()> {
        let mut out = std::fs::File::create(&tmp)?;
        let mut exe_file = std::fs::File::open(&exe)?;
        std::io::copy(&mut exe_file, &mut out)?;
        out.write_all(trailer_bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            out.set_permissions(std::fs::Permissions::from_mode(0o755))?;
        }
        // Flush the complete artifact before the rename (the same
        // temp-then-rename durability convention as the file component's
        // `atomic_write`).
        out.sync_all()
    };
    if let Err(e) = write() {
        return Err(discard(
            &tmp,
            format!("cannot write the artifact to '{}': {e}", tmp.display()),
        ));
    }
    if let Err(e) = std::fs::rename(&tmp, output) {
        return Err(discard(
            &tmp,
            format!("cannot publish the artifact to '{}': {e}", output.display()),
        ));
    }
    Ok(())
}

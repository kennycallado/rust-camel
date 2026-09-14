//! `camel compile <document> -o <artifact>` — build a self-contained
//! native executable artifact (openspec changes `cli-compile` and
//! `multidoc`).
//!
//! The command resolves the explicit compile inputs — the primary
//! document plus the optional `--config <Camel.toml>` and repeatable
//! `--profile <name>` selections — captures every source BEFORE
//! `${env:}` interpolation as a typed virtual-store entry, validates the
//! asset allowlist ([`crate::compile::policy`]), derives the
//! schema-2 operational manifest, and only then writes
//! `<output>.tmp` (executable copy + v2 trailer) and renames it onto the
//! output path. Every rejection — non-native target, dirty compile
//! environment, `Camel.toml` in the working directory without
//! `--config`, invalid UTF-8, oversize payload, unsupported asset,
//! unresolvable or unconfined source, duplicate source, unknown
//! profile, unparsable document — exits 2 with a named diagnostic;
//! failures never delete or truncate a pre-existing output and never
//! leave a usable partial artifact.
//!
//! Trust model: the artifact is a copy of the Camel executable plus
//! authoring text; deployment owns process capabilities. v2 supports
//! the native Linux target only.
//!
//! Source selection is explicit-only: without `--config` the compiler
//! embeds no configuration and selects no profile (an ambient
//! `Camel.toml` in the compile working directory is still a v1-style
//! rejection); with it, resolution and confinement are owned by
//! [`crate::compile::sources`].

use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::Args;

use crate::compile::manifest;
use crate::compile::policy;
use crate::compile::sources::{self, SourceSelection};
use crate::compile::store::VirtualDocumentStore;
use crate::compile::trailer::{self, TrailerKind, TrailerV2};

/// Exit code for every named compile rejection.
const EXIT_REJECTION: i32 = 2;

/// CLI args for `camel compile`.
#[derive(Args, Debug)]
pub struct CompileArgs {
    /// Single route document (`*.yaml`/`*.yml`/`*.json`) or job document
    /// (`*.job.yaml`/`*.job.yml`) to embed as the artifact entry point.
    #[arg(value_name = "DOCUMENT")]
    pub document: PathBuf,

    /// Path of the executable artifact to write.
    #[arg(short, long, value_name = "ARTIFACT")]
    pub output: PathBuf,

    /// Requested target triple; v2 accepts the native Linux triple only.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,

    /// Explicitly selected `Camel.toml` to resolve and embed (ordered
    /// includes, selected profiles, route patterns). Its directory
    /// becomes the confinement root. Without this flag no
    /// configuration is embedded and none is discovered.
    #[arg(long, value_name = "CONFIG")]
    pub config: Option<PathBuf>,

    /// Selected configuration profile; repeatable, order preserved.
    /// Requires `--config`.
    #[arg(long = "profile", value_name = "NAME")]
    pub profile: Vec<String>,
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
    // Native-Linux-only gate: v2 has no cross-compilation path.
    if !cfg!(target_os = "linux") {
        eprintln!(
            "camel compile v2 supports native Linux only; this host is {}",
            std::env::consts::OS
        );
        return EXIT_REJECTION;
    }
    if let Some(requested) = &args.target
        && *requested != native_triple()
    {
        eprintln!(
            "camel compile v2 supports native Linux only: target '{requested}' does not match the native target '{}'",
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
            "camel compile v2 requires a clean compile environment; CAMEL_* variable(s) present: {}",
            overrides.join(", ")
        );
        return EXIT_REJECTION;
    }

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

    // Without an explicitly selected configuration, the v1 rule holds:
    // no Camel.toml in the compile working directory (compiled documents
    // must be self-contained; compile never discovers ambient
    // configuration). With --config the explicit selection governs and
    // an unrelated ambient file is simply never read.
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("camel compile: cannot determine the working directory: {e}");
            return EXIT_REJECTION;
        }
    };
    if args.config.is_none() && cwd.join("Camel.toml").is_file() {
        eprintln!(
            "camel compile v2 rejects a Camel.toml in the compile working directory ('{}'); pass --config <Camel.toml> to embed one explicitly",
            cwd.display()
        );
        return EXIT_REJECTION;
    }

    // Resolve and confine the explicit multi-document source set. This
    // is the single read of the entry document; the normalized text the
    // resolver captured is what every later gate sees.
    let selection = SourceSelection {
        config_path: args.config.clone(),
        profiles: args.profile.clone(),
    };
    let sources = match sources::resolve(&args.document, kind, &selection) {
        Ok(sources) => sources,
        Err(e) => {
            eprintln!("camel compile: {e}");
            return EXIT_REJECTION;
        }
    };

    // Fail-closed asset policy on the entry document, from the text the
    // resolver already captured (route-source declarations are permitted
    // here: the resolver embeds them). The rejection still precedes any
    // output creation and any writer action.
    let entry_document = sources
        .route_documents
        .first()
        .expect("the resolver always leads the plan with the entry document"); // allow-unwrap
    // Job documents run the argument-declaration checks at compile time
    // (jobtyped): name grammar, unknown fields, `type` grammar, and
    // typed-default coercion — a failure exits 2 with NO artifact
    // written. Route documents keep the exact prior path, and no
    // execution-value validation runs at compile time (cli-jobs spec,
    // `typed argument coercion`).
    if kind == TrailerKind::Job
        && let Err(e) =
            crate::commands::job::validate_job_declarations_for_compile(&entry_document.1)
    {
        eprintln!("camel compile: {e}");
        return EXIT_REJECTION;
    }
    if let Err(e) = policy::reject_entry_document_assets(&entry_document.1, kind) {
        eprintln!("camel compile: {e}");
        return EXIT_REJECTION;
    }

    // The stricter nested rule on every additionally embedded route/job
    // document: their own route-source declarations are not recursively
    // resolvable and stay rejected.
    for (path, doc_text) in sources.route_documents.iter().skip(1) {
        if let Err(e) = policy::reject_unsupported_assets(doc_text, TrailerKind::Route) {
            eprintln!("camel compile: embedded document '{path}': {e}");
            return EXIT_REJECTION;
        }
    }

    // Build the canonical v2 store and the schema-2 manifest that
    // mirrors it — all before any output byte exists.
    let store = match VirtualDocumentStore::build(
        &sources.entry_point,
        &sources.documents,
        &sources.config_references,
        &sources.source_plan,
    ) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("camel compile: invalid source set: {e}");
            return EXIT_REJECTION;
        }
    };
    let operational = match manifest::derive_for_store(&store, kind, &sources.route_documents) {
        Ok(operational) => operational,
        Err(e) => {
            eprintln!("camel compile: {e}");
            return EXIT_REJECTION;
        }
    };
    let index_bytes = match store.index.encode_canonical() {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("camel compile: cannot encode the store index: {e}");
            return EXIT_REJECTION;
        }
    };

    // Copy the current executable and append the v2 trailer
    // (`CAMELTR1 || content || index || manifest || 76-byte footer`) as
    // the only write phase.
    let artifact = TrailerV2 {
        kind,
        content: store.content,
        index: index_bytes,
        manifest: operational.to_canonical_json().into_bytes(),
    };
    if let Err(e) = write_artifact(&args.output, &trailer::encode_v2(&artifact)) {
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

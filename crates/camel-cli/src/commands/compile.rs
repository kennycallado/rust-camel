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
//! output path. With `--sign` (r4sign) the artifact bytes stream through
//! a SHA-512 prehash during that write and a detached `CAMELSG1`
//! Ed25519ph envelope is published to `<output>.sig` afterwards; a
//! signed manifest uses schema 5 (keypin) with the signing block,
//! carrying the unix-seconds freshness marker.
//! Every rejection — non-native target, dirty compile
//! environment, `Camel.toml` in the working directory without
//! `--config`, invalid UTF-8, aggregate payload over the configured
//! `--max-payload-bytes` cap, unsupported asset, secret-family asset
//! without `--embed-secrets`, unresolvable or unconfined source,
//! duplicate source, unknown profile, unparsable document, invalid
//! signing input, envelope-emission failure — exits 2 with a named
//! diagnostic;
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
use ed25519_dalek::{Digest as _, Sha512, SigningKey};

use crate::compile::manifest;
use crate::compile::policy;
use crate::compile::runtime::TRUSTSTORE_ENV;
use crate::compile::signature;
use crate::compile::sources::{self, SourceSelection};
use crate::compile::store::{StoreEntryKind, VirtualDocumentStore};
use crate::compile::trailer::{self, TrailerKind, TrailerV2};

/// Exit code for every named compile rejection.
const EXIT_REJECTION: i32 = 2;

/// The single namespaced compile-time environment variable allowed
/// through the clean-environment guard conditionally (r4sign): under
/// `--sign` it supplies the signing-key path (a signing input, not a
/// configuration override); its stray presence without `--sign` stays
/// rejected.
const SIGNING_KEY_ENV: &str = "CAMEL_COMPILE_SIGNING_KEY";

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

    /// Aggregate payload cap in bytes: the normalized document bytes plus
    /// the verbatim asset bytes of one artifact must not exceed it. Must
    /// be positive; defaults to 16777216 (16 MiB).
    #[arg(
        long,
        value_name = "BYTES",
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub max_payload_bytes: Option<u64>,

    /// Opt in to embedding secret-family assets (exactly the private-key
    /// family: the document `key` field and the `tlsKey`, `serverKeyPath`,
    /// and `clientKeyPath` TLS URI parameters). Without this flag a
    /// compile that collects a private-key asset fails closed before any
    /// output; with it the secret embeds and the artifact is written
    /// mode 0700.
    #[arg(long)]
    pub embed_secrets: bool,

    /// Sign the artifact with a detached Ed25519ph envelope written to
    /// `<output>.sig` (r4sign). The signing key comes from
    /// `--signing-key` or the `CAMEL_COMPILE_SIGNING_KEY` environment
    /// variable; the argument wins when both are present. The key is a
    /// signing input only: no key material enters the artifact, the
    /// envelope, the manifest, or any output.
    #[arg(long)]
    pub sign: bool,

    /// Path of the 32-byte ed25519 seed file used with `--sign`.
    /// Requires `--sign`.
    #[arg(long, value_name = "PATH")]
    pub signing_key: Option<PathBuf>,

    /// Mark the signature required: a compiled artifact started without
    /// its envelope fails closed at boot. Requires `--sign`.
    #[arg(long)]
    pub require_signature: bool,
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
    // what the artifact embeds, so none may be present. The carve-outs:
    // `CAMEL_COMPILE_SIGNING_KEY` (r4sign) — a signing input, not a
    // configuration override — allowed through only under `--sign` and
    // rejected as a stray variable otherwise; and `CAMEL_TRUSTSTORE`
    // (keypin) — a verify-side input, never read at compile time, so it
    // is benign in any compile.
    let mut stray_signing_key = false;
    let overrides: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _)| {
            let name = name.to_string_lossy();
            if !name.starts_with("CAMEL_") {
                return None;
            }
            if name == SIGNING_KEY_ENV {
                stray_signing_key = true;
                return None;
            }
            if name == TRUSTSTORE_ENV {
                return None;
            }
            Some(name.into_owned())
        })
        .collect();
    if !overrides.is_empty() {
        eprintln!(
            "camel compile v2 requires a clean compile environment; CAMEL_* variable(s) present: {}",
            overrides.join(", ")
        );
        return EXIT_REJECTION;
    }
    if stray_signing_key && !args.sign {
        eprintln!(
            "camel compile: stray compile-time environment variable {SIGNING_KEY_ENV} is set but the artifact is not signed; it supplies the --sign signing key, so pass --sign or unset the variable"
        );
        return EXIT_REJECTION;
    }

    // Signing input validation (r4sign Task 1.2): every rule below is a
    // named rejection that precedes any source resolution and any output
    // write.
    if args.signing_key.is_some() && !args.sign {
        eprintln!("camel compile: --signing-key requires --sign; pass --sign to sign the artifact");
        return EXIT_REJECTION;
    }
    if args.require_signature && !args.sign {
        eprintln!(
            "camel compile: --require-signature requires --sign; pass --sign to sign the artifact"
        );
        return EXIT_REJECTION;
    }
    let signing_key = if args.sign {
        // The argument wins over the environment when both are present.
        let key_path = match &args.signing_key {
            Some(path) => Some(path.clone()),
            None => std::env::var_os(SIGNING_KEY_ENV).map(PathBuf::from),
        };
        let Some(key_path) = key_path else {
            eprintln!(
                "camel compile: --sign requires a signing key: pass --signing-key <PATH> or set {SIGNING_KEY_ENV}"
            );
            return EXIT_REJECTION;
        };
        match signature::load_signing_key(&key_path) {
            Ok(key) => Some(key),
            Err(e) => {
                eprintln!("camel compile: {e}");
                return EXIT_REJECTION;
            }
        }
    } else {
        None
    };
    // Identity recorded in the schema-5 signing block: the algorithm
    // name, the BLAKE3 fingerprint of the verifying key (never the seed),
    // the required bit from `--require-signature`, and the compile-time
    // freshness marker (keypin Task 2.1).
    let signing_block = signing_key.as_ref().map(|key| manifest::SigningBlock {
        algorithm: signature::ALGORITHM_NAME_ED25519PH.to_string(),
        key_fingerprint: signature::fingerprint(&key.verifying_key().to_bytes()),
        required: args.require_signature,
        freshness: Some(unix_seconds_now()),
    });

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

    // An explicitly passed empty --config value (a programmatic caller
    // constructing `CompileArgs` with `Some(PathBuf::from(""))`; the CLI
    // parser already rejects empty values) must fail with a diagnostic
    // naming the flag instead of the opaque empty-string source error.
    if args.config.as_deref() == Some(Path::new("")) {
        eprintln!(
            "camel compile: --config requires a non-empty path; pass a Camel.toml path or omit the flag"
        );
        return EXIT_REJECTION;
    }

    // Resolve and confine the explicit multi-document source set. This
    // is the single read of the entry document; the normalized text the
    // resolver captured is what every later gate sees.
    // The cap threads to the single aggregation point in
    // `sources::resolve`; the flag-level validation (positive) already
    // ran in the clap value parser, before any work.
    let max_payload_bytes = args
        .max_payload_bytes
        .unwrap_or(trailer::MAX_PAYLOAD_BYTES as u64);
    let selection = SourceSelection {
        config_path: args.config.clone(),
        profiles: args.profile.clone(),
        max_payload_bytes,
        embed_secrets: args.embed_secrets,
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
    // mirrors it — all before any output byte exists. The confined
    // deploy-time assets and the substitution table resolved by
    // `sources::resolve` (r2embed Task 1.2) join the documents in the
    // store's canonical interleave.
    let store = match VirtualDocumentStore::build_with_assets(
        &sources.entry_point,
        &sources.documents,
        &sources.assets,
        &sources.config_references,
        &sources.source_plan,
        &sources.substitutions,
    ) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("camel compile: invalid source set: {e}");
            return EXIT_REJECTION;
        }
    };
    let mut operational = match manifest::derive_for_store(&store, kind, &sources.route_documents) {
        Ok(operational) => operational,
        Err(e) => {
            eprintln!("camel compile: {e}");
            return EXIT_REJECTION;
        }
    };
    // Signed compiles emit manifest schema 5 with the signing block and
    // its freshness marker (keypin Task 2.1); unsigned compiles stay
    // byte-identical schema 3 (the canonical JSON carries no `signing`
    // field).
    if let Some(signing) = signing_block {
        operational.manifest_schema = manifest::MANIFEST_SCHEMA_V5;
        operational.signing = Some(signing);
    }
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
    // Single artifact write site: the file mode follows the store class
    // inventory. Any embedded secret-family entry — exactly the
    // private-key class, which only reaches the store under
    // `--embed-secrets` — makes the artifact 0700 so secret material is
    // never world-executable; everything else keeps 0755.
    let artifact_mode = if store.index.entries.iter().any(|entry| {
        entry.kind == StoreEntryKind::Asset
            && entry.asset_class.as_deref() == Some(policy::SECRET_ASSET_CLASS)
    }) {
        0o700
    } else {
        0o755
    };
    let artifact = TrailerV2 {
        kind,
        content: store.content,
        index: index_bytes,
        manifest: operational.to_canonical_json().into_bytes(),
    };
    // When signing, the exact byte stream written below (executable copy
    // plus trailer) feeds the SHA-512 prehash digest inside
    // `write_artifact` — no re-read, no buffering of the artifact.
    let mut artifact_digest = signing_key.is_some().then(Sha512::default);
    if let Err(e) = write_artifact(
        &args.output,
        &trailer::encode_v2(&artifact),
        artifact_mode,
        artifact_digest.as_mut(),
    ) {
        eprintln!("camel compile: {e}");
        return EXIT_REJECTION;
    }

    // Envelope emission happens only after the artifact rename
    // succeeded: the same streamed digest signs the final artifact
    // bytes. A failure removes the artifact so no signed manifest is
    // ever published without its envelope.
    if let (Some(key), Some(hasher)) = (&signing_key, artifact_digest) {
        let digest: [u8; 64] = hasher.finalize().into();
        if let Err(e) = write_envelope(&args.output, key, &digest) {
            if let Err(remove_err) = std::fs::remove_file(&args.output) {
                eprintln!(
                    "camel compile: artifact removal after envelope failure also failed: {remove_err}"
                );
            }
            eprintln!("camel compile: {e}");
            return EXIT_REJECTION;
        }
    } else {
        // Unsigned compile: a stale envelope from a previous signed
        // compile of the same output would fail closed at boot as an
        // unpaired envelope — remove it so the fresh unsigned artifact
        // deploys clean. Absence and removal failure are both benign
        // here (the artifact itself is complete and valid).
        let mut stale_name = args.output.as_os_str().to_owned();
        stale_name.push(".sig");
        let _ = std::fs::remove_file(Path::new(&stale_name));
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

/// The compile-time freshness marker (keypin Task 2.1): unix seconds
/// since the epoch. The marker lives inside the signature-covered
/// manifest bytes; its one-second resolution bounds rollback detection,
/// not security.
fn unix_seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before the unix epoch") // allow-unwrap
        .as_secs()
}

/// Copy `current_exe()` plus the encoded trailer into `output` atomically.
/// Both are written to the sibling `<output>.tmp` first, flushed to disk,
/// and only the complete temp file is renamed onto `output`, carrying
/// `mode` (0755 for secret-free artifacts, 0700 when secret-family
/// entries are embedded). When `artifact_digest` is `Some` (r4sign), the
/// exact byte stream written — executable copy plus trailer — also feeds
/// the SHA-512 prehash digest, so the caller can sign the final artifact
/// bytes without a re-read and without buffering. The temp file is
/// created exclusively, in one
/// `create_new` open, with its final mode — so no window exists where
/// secret-bearing bytes sit in a readable file before a post-copy chmod,
/// and a planted symlink at `<output>.tmp` is refused instead of being
/// followed. A pre-existing `<output>.tmp` (for example left by a crashed
/// run) fails that open with a clear error naming it; this fail-closed
/// behavior is intentional. On any failure the temp file is removed and
/// any pre-existing output is left untouched — a rejected or failed
/// compile never deletes or truncates an existing artifact, and no
/// partial artifact is ever visible at `output`.
fn write_artifact(
    output: &Path,
    trailer_bytes: &[u8],
    mode: u32,
    mut artifact_digest: Option<&mut Sha512>,
) -> Result<(), String> {
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

    let mut write = || -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        // The file is born with its final mode (no post-copy chmod), and
        // `create_new` refuses an existing entry — including a planted
        // symlink — at the temp path instead of truncating and following
        // it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let mut out = opts.open(&tmp)?;
        let mut exe_file = std::fs::File::open(&exe)?;
        // Flush the complete artifact before the rename (the same
        // temp-then-rename durability convention as the file component's
        // `atomic_write`).
        match artifact_digest.as_deref_mut() {
            // Signing: every written byte also feeds the prehash digest.
            Some(digest) => {
                let mut out = HashingWriter { inner: out, digest };
                std::io::copy(&mut exe_file, &mut out)?;
                out.write_all(trailer_bytes)?;
                out.inner.sync_all()
            }
            None => {
                std::io::copy(&mut exe_file, &mut out)?;
                out.write_all(trailer_bytes)?;
                out.sync_all()
            }
        }
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

/// Writer adapter feeding every byte written to the artifact also into
/// the SHA-512 prehash digest (r4sign): the signature covers the exact
/// final artifact byte stream — no re-read, no buffering.
struct HashingWriter<'a, W: std::io::Write> {
    inner: W,
    digest: &'a mut Sha512,
}

impl<W: std::io::Write> std::io::Write for HashingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.digest.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Encode and publish the detached `CAMELSG1` envelope for `key` over
/// the streamed artifact digest to `<output>.sig` atomically: the
/// envelope is written to the sibling `<output>.sig.tmp` (created
/// exclusively with mode 0644, flushed), then renamed onto
/// `<output>.sig`. On any failure the temp file is removed and an error
/// naming the envelope step is returned; the caller removes the artifact
/// so no signed manifest is ever published without its envelope.
fn write_envelope(
    output: &Path,
    key: &SigningKey,
    message_sha512: &[u8; 64],
) -> Result<(), String> {
    let mut sig_name = output.as_os_str().to_owned();
    sig_name.push(".sig");
    let sig = PathBuf::from(sig_name);
    let mut tmp_name = sig.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);

    /// Remove the temp file this function owns; `sig` is never touched.
    fn discard(tmp: &Path, message: String) -> String {
        let _ = std::fs::remove_file(tmp);
        message
    }

    let write = || -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o644);
        }
        let mut out = opts.open(&tmp)?;
        out.write_all(&signature::encode_envelope(key, message_sha512))?;
        out.sync_all()
    };
    if let Err(e) = write() {
        return Err(discard(
            &tmp,
            format!(
                "cannot write the signature envelope to '{}': {e}",
                sig.display()
            ),
        ));
    }
    if let Err(e) = std::fs::rename(&tmp, &sig) {
        return Err(discard(
            &tmp,
            format!(
                "cannot publish the signature envelope to '{}': {e}",
                sig.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod empty_config_tests {
    use super::{CompileArgs, run_compile};
    use std::path::PathBuf;

    /// cfgempty: an explicitly empty `--config` value must be a named
    /// rejection (exit 2), not an opaque empty-source diagnostic. The
    /// CLI parser already rejects empty values for `Option<PathBuf>`
    /// args (clap `PathBufValueParser`), so this seam is only reachable
    /// by programmatic `CompileArgs` construction — which is exactly
    /// what this test pins. The stderr text is asserted at the clap
    /// boundary by `compile_command_test`; `run_compile` prints via
    /// `eprintln!` with no capture seam, so the in-process assertion is
    /// the exit code alone.
    #[test]
    fn compile_config_empty_value_is_named_rejection_exit_2() {
        let args = CompileArgs {
            document: PathBuf::from("routes.yaml"),
            output: PathBuf::from("artifact-out"),
            target: None,
            config: Some(PathBuf::from("")),
            profile: Vec::new(),
            max_payload_bytes: None,
            embed_secrets: false,
            sign: false,
            signing_key: None,
            require_signature: false,
        };
        assert_eq!(
            run_compile(&args),
            2,
            "empty --config must be the named rejection, not a source-resolution path"
        );
    }
}

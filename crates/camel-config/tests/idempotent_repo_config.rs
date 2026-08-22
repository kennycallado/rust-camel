use camel_config::CamelConfig;

fn make_cfg(toml: &str) -> CamelConfig {
    config::Config::builder()
        .add_source(config::File::from_str(toml, config::FileFormat::Toml))
        .build()
        .unwrap()
        .try_deserialize::<CamelConfig>()
        .unwrap()
}

// ── Pre-change redb TOML keeps parsing ────────────────────────────────────

#[test]
fn existing_redb_toml_parses_unchanged() {
    // A pre-backend-discriminator TOML section: only path + durability.
    // It must parse with backend defaulting to "redb".
    let cfg = make_cfg(
        r#"
[idempotent_repo]
path = "x.redb"
durability = "eventual"
"#,
    );

    let repo = cfg.idempotent_repo.expect("idempotent_repo must parse");
    assert_eq!(repo.backend, "redb");
    assert_eq!(repo.path.as_deref(), Some("x.redb"));
    assert_eq!(repo.durability.as_deref(), Some("eventual"));
}

#[test]
fn idempotent_redb_empty_path_still_rejected() {
    let cfg = make_cfg(
        r#"
[idempotent_repo]
path = ""
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("idempotent_repo") && msg.contains("path"),
        "validation error must name idempotent_repo.path, got: {msg}"
    );
}

#[test]
fn idempotent_redb_durability_default_immediate() {
    // Omitted durability means "immediate": the field stays `None` and the
    // registration site resolves it to JournalDurability::Immediate.
    let cfg = make_cfg(
        r#"
[idempotent_repo]
path = "x.redb"
"#,
    );

    let repo = cfg
        .idempotent_repo
        .as_ref()
        .expect("idempotent_repo must parse");
    assert_eq!(repo.backend, "redb");
    assert_eq!(repo.durability, None);
    cfg.validate()
        .expect("path-only redb config must pass validation");
}

#[test]
fn idempotent_unknown_backend_rejected() {
    let cfg = make_cfg(
        r#"
[idempotent_repo]
backend = "postgres"
path = "x.db"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("idempotent_repo.backend"),
        "validation error must name idempotent_repo.backend, got: {msg}"
    );
}

#[test]
fn idempotent_redb_invalid_durability_rejected() {
    let cfg = make_cfg(
        r#"
[idempotent_repo]
path = "x.redb"
durability = "sometimes"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("idempotent_repo.durability"),
        "validation error must name idempotent_repo.durability, got: {msg}"
    );
}

// ── Redis validation mirrors the cache matrix ─────────────────────────────

#[test]
fn idempotent_redis_validation_mirrors_matrix() {
    // sentinel_nodes without master_name → error naming master_name
    let cfg = make_cfg(
        r#"
[idempotent_repo]
backend = "redis"
sentinel_nodes = ["s-a:26379"]
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("idempotent_repo.master_name"),
        "validation error must name idempotent_repo.master_name, got: {msg}"
    );

    // url + sentinel_nodes → mutual exclusion error naming the repo
    let cfg = make_cfg(
        r#"
[idempotent_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
sentinel_nodes = ["s-a:26379"]
master_name = "orders"
"#,
    );
    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("idempotent_repo") && msg.contains("mutually exclusive"),
        "validation error must name idempotent_repo and mutual exclusion, got: {msg}"
    );
}

// ── Cross-backend field rejection (parity with cache_repo) ────────────────

#[test]
fn idempotent_redb_rejects_redis_fields() {
    let cfg = make_cfg(
        r#"
[idempotent_repo]
path = "x.redb"
url = "redis://127.0.0.1:6379"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("idempotent_repo.url") && msg.contains("does not apply"),
        "validation error must name idempotent_repo.url, got: {msg}"
    );
}

#[test]
fn idempotent_redis_rejects_redb_fields() {
    let cfg = make_cfg(
        r#"
[idempotent_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
path = "x.redb"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("idempotent_repo.path") && msg.contains("does not apply"),
        "validation error must name idempotent_repo.path, got: {msg}"
    );
}

// ── Debug redaction of redis credentials ──────────────────────────────────

#[test]
fn idempotent_debug_redacts_credentials() {
    let cfg = make_cfg(
        r#"
[idempotent_repo]
backend = "redis"
url = "redis://user:secret@h:6379"
sentinel_password = "hunter2"
"#,
    );

    let rendered = format!(
        "{:?}",
        cfg.idempotent_repo.expect("idempotent_repo must load")
    );
    assert!(
        !rendered.contains("secret") && !rendered.contains("hunter2"),
        "Debug output must not leak credentials, got: {rendered}"
    );
    assert!(
        rendered.contains("redis://***@h:6379"),
        "Debug output must redact URL userinfo as ***, got: {rendered}"
    );
}

// ── Cross-repository prefix collision ─────────────────────────────────────

#[test]
fn shared_database_prefix_collision_rejected() {
    // Both redis on the same database with identical prefixes → rejected.
    // (Grammar-valid urls: a `/N` db-in-path suffix fails validate() before
    // the collision rule is reached.)
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://h:6379"
key_prefix = "camel:shared"

[idempotent_repo]
backend = "redis"
url = "redis://h:6379"
key_prefix = "camel:shared"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("must be distinct"),
        "validation error must state the prefixes must be distinct, got: {msg}"
    );

    // Distinct prefixes on the same endpoint are fine.
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://h:6379"
key_prefix = "camel:cache"

[idempotent_repo]
backend = "redis"
url = "redis://h:6379"
key_prefix = "camel:idem"
"#,
    );

    cfg.validate()
        .expect("distinct prefixes on the same redis database must pass validation");
}

#[test]
fn collision_detects_default_port_spelling() {
    // `redis://h` and `redis://h:6379` address the same database: the
    // collision rule must fold the default port instead of treating the
    // two spellings as distinct endpoints.
    let cfg = make_cfg(
        r#"
[cache_repo]
backend = "redis"
url = "redis://h"
key_prefix = "camel:shared"

[idempotent_repo]
backend = "redis"
url = "redis://h:6379"
key_prefix = "camel:shared"
"#,
    );

    let msg = cfg.validate().unwrap_err().to_string();
    assert!(
        msg.contains("must be distinct"),
        "default-port spelling variants must collide, got: {msg}"
    );
}

// ── Redis idempotent wiring (live-lite; full live coverage in 3.5) ────────

#[tokio::test]
async fn redis_idempotent_registered_when_configured() {
    // An unreachable url fails the context build with an error naming
    // idempotent_repo — proving the redis registration branch was reached.
    let cfg = make_cfg(
        r#"
[idempotent_repo]
backend = "redis"
url = "redis://127.0.0.1:1/0"
"#,
    );

    let err = match CamelConfig::configure_context(&cfg).await {
        Ok(_) => panic!("an unreachable redis url must fail the context build"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("idempotent_repo"),
        "build error must name idempotent_repo, got: {err}"
    );
}

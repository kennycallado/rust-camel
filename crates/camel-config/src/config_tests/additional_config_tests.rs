use super::*;

#[test]
fn journal_durability_converts_to_core_type() {
    let immediate: camel_core::JournalDurability = JournalDurability::Immediate.into();
    let eventual: camel_core::JournalDurability = JournalDurability::Eventual.into();
    assert_eq!(immediate, camel_core::JournalDurability::Immediate);
    assert_eq!(eventual, camel_core::JournalDurability::Eventual);
}

#[test]
fn supervision_into_supervision_config_converts_durations() {
    let input = SupervisionCamelConfig {
        max_attempts: Some(7),
        initial_delay_ms: 123,
        backoff_multiplier: 1.5,
        max_delay_ms: 999,
    };

    let out = input.into_supervision_config();
    assert_eq!(out.max_attempts, Some(7));
    assert_eq!(out.initial_delay, Duration::from_millis(123));
    assert_eq!(out.backoff_multiplier, 1.5);
    assert_eq!(out.max_delay, Duration::from_millis(999));
}

#[test]
fn redb_journal_options_from_journal_config_copies_fields() {
    let cfg = JournalConfig {
        path: std::path::PathBuf::from("journal.db"),
        durability: JournalDurability::Eventual,
        compaction_threshold_events: 42,
    };

    let options: camel_core::RedbJournalOptions = (&cfg).into();
    assert_eq!(options.durability, camel_core::JournalDurability::Eventual);
    assert_eq!(options.compaction_threshold_events, 42);
}

#[test]
fn redb_idempotent_config_defaults_to_immediate_durability() {
    let toml_str = r#"
[idempotent_repo]
path = "x.redb"
"#;
    let config: CamelConfig = toml::from_str(toml_str).unwrap();
    let repo = config.idempotent_repo.expect("idempotent_repo must parse");
    // `None` means "immediate" — resolved at the registration site.
    assert_eq!(repo.durability, None);
}

#[test]
fn redb_idempotent_config_parses_eventual_durability() {
    let toml_str = r#"
[idempotent_repo]
path = "x.redb"
durability = "eventual"
"#;
    let config: CamelConfig = toml::from_str(toml_str).unwrap();
    let repo = config.idempotent_repo.expect("idempotent_repo must parse");
    assert_eq!(repo.durability.as_deref(), Some("eventual"));
}

#[test]
fn redb_idempotent_config_durability_roundtrips_to_core() {
    let durability = JournalDurability::Eventual;
    let core: camel_core::JournalDurability = durability.into();
    assert_eq!(core, camel_core::JournalDurability::Eventual);
}

#[test]
fn from_env_or_default_uses_camel_config_file_env() {
    use std::io::Write;

    // from_env_or_default reads the full CAMEL_* override allowlist, so
    // this test must serialize against the other env-touching tests or
    // a concurrent allowlist mutation leaks into its assertions.
    let _guard = super::env_lock();

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(
        br#"
watch = true
timeout_ms = 111
"#,
    )
    .unwrap();

    unsafe {
        std::env::set_var("CAMEL_CONFIG_FILE", file.path());
    }
    let cfg = CamelConfig::from_env_or_default().unwrap();
    unsafe {
        std::env::remove_var("CAMEL_CONFIG_FILE");
    }

    assert!(cfg.watch);
    assert_eq!(cfg.timeout_ms, 111);
}

#[test]
fn from_env_or_default_applies_env_overrides() {
    // Serialize against async env-reading tests (see ENV_OVERRIDE_LOCK).
    let _guard = super::env_lock();
    use std::io::Write;

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(br#"timeout_ms = 222"#).unwrap();

    super::set_env("CAMEL_CONFIG_FILE", file.path().to_str().unwrap());
    super::set_env("CAMEL_TIMEOUT_MS", "12345");
    let loaded = CamelConfig::from_env_or_default();
    super::unset_env("CAMEL_CONFIG_FILE");
    super::unset_env("CAMEL_TIMEOUT_MS");

    let cfg = loaded.expect("from_env_or_default should load the fixture");
    assert_eq!(
        cfg.timeout_ms, 12345,
        "from_env_or_default must apply allowlisted CAMEL_* overrides on top of the file"
    );
}

#[test]
fn from_file_with_profile_and_env_unknown_profile_errors() {
    use std::io::Write;

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(
        br#"
[default]
watch = false
"#,
    )
    .unwrap();

    let err =
        CamelConfig::from_file_with_profile_and_env(file.path().to_str().unwrap(), Some("missing"))
            .unwrap_err();
    assert!(err.to_string().contains("Unknown profile: missing"));
}

use super::*;

fn parse(toml: &str) -> CamelConfig {
    let cfg = config::Config::builder()
        .add_source(config::File::from_str(toml, config::FileFormat::Toml))
        .build()
        .unwrap();
    cfg.try_deserialize().unwrap()
}

#[test]
fn platform_default_is_noop() {
    let cfg = parse("");
    assert!(matches!(cfg.platform, PlatformCamelConfig::Noop));
}

#[test]
fn platform_parses_kubernetes_from_toml() {
    let cfg = parse(
        r#"
[platform]
type = "kubernetes"
namespace = "team-a"
lease_name_prefix = "camel-"
lease_duration_secs = 15
renew_deadline_secs = 10
retry_period_secs = 2
jitter_factor = 0.2
"#,
    );
    match cfg.platform {
        PlatformCamelConfig::Kubernetes(k8s) => {
            assert_eq!(k8s.namespace.as_deref(), Some("team-a"));
            assert_eq!(k8s.lease_name_prefix, "camel-");
            assert_eq!(k8s.lease_duration_secs, 15);
            assert_eq!(k8s.renew_deadline_secs, 10);
            assert_eq!(k8s.retry_period_secs, 2);
            assert!((k8s.jitter_factor - 0.2).abs() < f64::EPSILON);
        }
        other => panic!("expected Kubernetes, got {:?}", other),
    }
}

#[test]
fn platform_kubernetes_defaults() {
    let cfg = parse(
        r#"
[platform]
type = "kubernetes"
"#,
    );
    match cfg.platform {
        PlatformCamelConfig::Kubernetes(k8s) => {
            assert!(k8s.namespace.is_none());
            assert_eq!(k8s.lease_name_prefix, "camel-");
            assert_eq!(k8s.lease_duration_secs, 15);
            assert_eq!(k8s.renew_deadline_secs, 10);
            assert_eq!(k8s.retry_period_secs, 2);
            assert!((k8s.jitter_factor - 0.2).abs() < f64::EPSILON);
        }
        other => panic!("expected Kubernetes, got {:?}", other),
    }
}

#[test]
fn platform_parses_kubernetes_from_file_with_profile() {
    let _guard = super::env_lock();
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    f.write_all(
        br#"
[default]
[default.platform]
type = "kubernetes"
namespace = "production"

[dev]
[dev.platform]
type = "noop"
"#,
    )
    .expect("write config");

    let cfg_prod = CamelConfig::from_file_with_profile(f.path().to_str().unwrap(), Some("default"))
        .expect("prod config");
    assert!(matches!(
        cfg_prod.platform,
        PlatformCamelConfig::Kubernetes(_)
    ));

    let cfg_dev = CamelConfig::from_file_with_profile(f.path().to_str().unwrap(), Some("dev"))
        .expect("dev config");
    assert!(matches!(cfg_dev.platform, PlatformCamelConfig::Noop));
}

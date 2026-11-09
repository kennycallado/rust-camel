//! Published GitHub release probe for declared bridge versions.

use super::registry::{Bridge, Registry};

/// One failed release check, naming the bridge tag and the exact URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseFinding {
    pub tag: String,
    pub url: String,
    pub detail: String,
}

/// One published release that carries every expected asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePass {
    pub tag: String,
    pub assets: usize,
}

/// Release tag for `version` of `bridge`.
pub fn release_tag(bridge: Bridge, version: &str) -> String {
    format!("{}-bridge-v{}", bridge.key(), version)
}

/// GitHub release API URL for `tag`.
pub fn release_api_url(tag: &str) -> String {
    format!("https://api.github.com/repos/kennycallado/rust-camel/releases/tags/{tag}")
}

/// GitHub release download base URL for `tag`.
pub fn release_download_base(tag: &str) -> String {
    format!("https://github.com/kennycallado/rust-camel/releases/download/{tag}")
}

/// Assets every bridge release matrix publishes.
pub fn expected_assets(bridge: Bridge, version: &str) -> Vec<String> {
    let b = bridge.key();
    vec![
        format!("{b}-bridge-{version}-linux-musl-x86_64.tar.gz"),
        format!("{b}-bridge-{version}-macos-aarch64.tar.gz"),
        format!("{b}-bridge-{version}-windows-x86_64.zip"),
        format!("{b}-bridge-{version}-SHA256SUMS"),
    ]
}

#[derive(serde::Deserialize)]
struct ReleasePayload {
    draft: bool,
    assets: Vec<ReleaseAsset>,
}

#[derive(serde::Deserialize)]
struct ReleaseAsset {
    name: String,
}

/// Classify one release API response. Fails closed on every non-pass outcome.
pub fn classify_response(
    bridge: Bridge,
    version: &str,
    status: u16,
    body: Option<&str>,
) -> Result<ReleasePass, Vec<ReleaseFinding>> {
    let tag = release_tag(bridge, version);
    let api = release_api_url(&tag);

    match status {
        200 => {
            let Some(body) = body else {
                return Err(vec![ReleaseFinding {
                    tag,
                    url: api.clone(),
                    detail: format!(
                        "invalid JSON from {api}: empty response body - failing closed"
                    ),
                }]);
            };

            let payload = match serde_json::from_str::<ReleasePayload>(body) {
                Ok(payload) => payload,
                Err(e) => {
                    return Err(vec![ReleaseFinding {
                        tag,
                        url: api.clone(),
                        detail: format!("invalid JSON from {api}: {e} - failing closed"),
                    }]);
                }
            };

            let mut findings = Vec::new();
            if payload.draft {
                findings.push(ReleaseFinding {
                    tag: tag.clone(),
                    url: api.clone(),
                    detail: format!(
                        "release {tag} is a draft - publish it before declaring the bump complete"
                    ),
                });
            }

            let expected = expected_assets(bridge, version);
            let available: Vec<&str> = payload
                .assets
                .iter()
                .map(|asset| asset.name.as_str())
                .collect();
            let missing: Vec<&str> = expected
                .iter()
                .map(String::as_str)
                .filter(|name| !available.contains(name))
                .collect();
            for name in missing {
                findings.push(ReleaseFinding {
                    tag: tag.clone(),
                    url: release_download_base(&tag),
                    detail: format!(
                        "release {tag} is missing asset {name} - downloads: {}",
                        release_download_base(&tag)
                    ),
                });
            }

            if findings.is_empty() {
                Ok(ReleasePass {
                    tag,
                    assets: expected.len(),
                })
            } else {
                Err(findings)
            }
        }
        404 => Err(vec![ReleaseFinding {
            tag: tag.clone(),
            url: api,
            detail: format!(
                "release {tag} not found - publish the release or fix bridges/versions.toml"
            ),
        }]),
        _ => Err(vec![ReleaseFinding {
            tag: tag.clone(),
            url: api.clone(),
            detail: format!(
                "release {tag}: unexpected HTTP status {status} at {api} - failing closed"
            ),
        }]),
    }
}

/// Fail-closed finding for a transport error or timeout.
pub fn classify_transport_error(tag: &str, error: &str) -> ReleaseFinding {
    ReleaseFinding {
        tag: tag.to_string(),
        url: release_api_url(tag),
        detail: format!(
            "release {tag}: transport error {error} at {} - failing closed",
            release_api_url(tag)
        ),
    }
}

/// Probe every registry bridge release. Never short-circuits across bridges.
pub fn check_releases(registry: &Registry) -> (Vec<ReleasePass>, Vec<ReleaseFinding>) {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .build()
        .new_agent();

    let mut passes = Vec::new();
    let mut findings = Vec::new();

    for bridge in [Bridge::Cxf, Bridge::Jms, Bridge::Xml] {
        let version = registry.version(bridge);
        let tag = release_tag(bridge, version);
        let url = release_api_url(&tag);

        match agent.get(&url).call() {
            Ok(mut response) => {
                let status = response.status().as_u16();
                match response.body_mut().read_to_string() {
                    Ok(body) => match classify_response(bridge, version, status, Some(&body)) {
                        Ok(pass) => passes.push(pass),
                        Err(mut release_findings) => findings.append(&mut release_findings),
                    },
                    Err(e) => findings.push(classify_transport_error(&tag, &e.to_string())),
                }
            }
            Err(ureq::Error::StatusCode(code)) => {
                match classify_response(bridge, version, code, None) {
                    Ok(pass) => passes.push(pass),
                    Err(mut release_findings) => findings.append(&mut release_findings),
                }
            }
            Err(e) => findings.push(classify_transport_error(&tag, &e.to_string())),
        }
    }

    (passes, findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JMS_ASSETS: [&str; 4] = [
        "jms-bridge-0.8.8-linux-musl-x86_64.tar.gz",
        "jms-bridge-0.8.8-macos-aarch64.tar.gz",
        "jms-bridge-0.8.8-windows-x86_64.zip",
        "jms-bridge-0.8.8-SHA256SUMS",
    ];

    const PASS_BODY: &str = r#"{"draft":false,"assets":[{"name":"jms-bridge-0.8.8-linux-musl-x86_64.tar.gz"},{"name":"jms-bridge-0.8.8-macos-aarch64.tar.gz"},{"name":"jms-bridge-0.8.8-windows-x86_64.zip"},{"name":"jms-bridge-0.8.8-SHA256SUMS"}]}"#;

    fn body_with(assets: &[&str], draft: bool) -> String {
        let entries: Vec<String> = assets
            .iter()
            .map(|name| format!(r#"{{"name":"{name}"}}"#))
            .collect();
        format!(r#"{{"draft":{draft},"assets":[{}]}}"#, entries.join(","))
    }

    #[test]
    fn release_tag_formats_bridge_and_version() {
        assert_eq!(release_tag(Bridge::Jms, "0.9.1"), "jms-bridge-v0.9.1");
        assert_eq!(release_tag(Bridge::Xml, "0.8.4"), "xml-bridge-v0.8.4");
    }

    #[test]
    fn release_api_url_is_exact() {
        assert_eq!(
            release_api_url("jms-bridge-v0.9.1"),
            "https://api.github.com/repos/kennycallado/rust-camel/releases/tags/jms-bridge-v0.9.1"
        );
    }

    #[test]
    fn release_download_base_is_exact() {
        assert_eq!(
            release_download_base("jms-bridge-v0.8.8"),
            "https://github.com/kennycallado/rust-camel/releases/download/jms-bridge-v0.8.8"
        );
    }

    #[test]
    fn expected_assets_lists_three_platforms_and_sums() {
        for bridge in [Bridge::Cxf, Bridge::Jms, Bridge::Xml] {
            let b = bridge.key();
            let assets = expected_assets(bridge, "0.8.8");
            assert_eq!(assets.len(), 4);
            assert_eq!(
                assets,
                vec![
                    format!("{b}-bridge-0.8.8-linux-musl-x86_64.tar.gz"),
                    format!("{b}-bridge-0.8.8-macos-aarch64.tar.gz"),
                    format!("{b}-bridge-0.8.8-windows-x86_64.zip"),
                    format!("{b}-bridge-0.8.8-SHA256SUMS"),
                ]
            );
        }
    }

    #[test]
    fn classify_published_release_passes() {
        let pass = classify_response(Bridge::Jms, "0.8.8", 200, Some(PASS_BODY))
            .expect("published release passes");

        assert_eq!(
            pass,
            ReleasePass {
                tag: "jms-bridge-v0.8.8".to_string(),
                assets: 4,
            }
        );
    }

    #[test]
    fn classify_draft_release_fails() {
        let body = PASS_BODY.replace(r#""draft":false"#, r#""draft":true"#);

        let findings =
            classify_response(Bridge::Jms, "0.8.8", 200, Some(&body)).expect_err("draft fails");

        assert_eq!(findings.len(), 1);
        assert!(findings[0].detail.contains("is a draft"));
        assert_eq!(findings[0].url, release_api_url("jms-bridge-v0.8.8"));
    }

    #[test]
    fn classify_missing_windows_asset_fails() {
        let assets = [JMS_ASSETS[0], JMS_ASSETS[1], JMS_ASSETS[3]];
        let body = body_with(&assets, false);

        let findings = classify_response(Bridge::Jms, "0.8.8", 200, Some(&body))
            .expect_err("missing asset fails");

        assert_eq!(findings.len(), 1);
        assert!(findings[0].detail.contains("windows-x86_64.zip"));
        assert!(
            findings[0]
                .detail
                .contains(&release_download_base("jms-bridge-v0.8.8"))
        );
        assert_eq!(findings[0].url, release_download_base("jms-bridge-v0.8.8"));
    }

    #[test]
    fn classify_missing_checksums_fails() {
        let assets = [JMS_ASSETS[0], JMS_ASSETS[1], JMS_ASSETS[2]];
        let body = body_with(&assets, false);

        let findings = classify_response(Bridge::Jms, "0.8.8", 200, Some(&body))
            .expect_err("missing checksums fail");

        assert_eq!(findings.len(), 1);
        assert!(findings[0].detail.contains("SHA256SUMS"));
    }

    #[test]
    fn classify_each_missing_asset_is_a_separate_finding() {
        let assets = [JMS_ASSETS[0], JMS_ASSETS[1]];
        let body = body_with(&assets, false);

        let findings = classify_response(Bridge::Jms, "0.8.8", 200, Some(&body))
            .expect_err("every missing asset is a finding");

        assert_eq!(findings.len(), 2);
        assert!(
            findings
                .iter()
                .any(|finding| finding.detail.contains("windows-x86_64.zip"))
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.detail.contains("SHA256SUMS"))
        );
        let download_base = release_download_base("jms-bridge-v0.8.8");
        assert!(findings.iter().all(|finding| finding.url == download_base));
    }

    #[test]
    fn classify_404_names_api_url() {
        let findings =
            classify_response(Bridge::Jms, "0.9.1", 404, None).expect_err("missing release fails");

        assert_eq!(findings.len(), 1);
        assert!(findings[0].detail.contains("not found"));
        assert_eq!(
            findings[0].url,
            "https://api.github.com/repos/kennycallado/rust-camel/releases/tags/jms-bridge-v0.9.1"
        );
    }

    #[test]
    fn classify_non_404_status_fails_closed() {
        let findings = classify_response(Bridge::Jms, "0.8.8", 500, None)
            .expect_err("unexpected status fails closed");

        assert_eq!(findings.len(), 1);
        assert!(findings[0].detail.contains("500"));
        assert!(findings[0].detail.contains("failing closed"));
        assert_eq!(findings[0].url, release_api_url("jms-bridge-v0.8.8"));
    }

    #[test]
    fn classify_invalid_json_fails_closed() {
        let findings = classify_response(Bridge::Jms, "0.8.8", 200, Some("<html>gateway</html>"))
            .expect_err("invalid JSON fails closed");

        assert_eq!(findings.len(), 1);
        assert!(findings[0].detail.contains("invalid JSON"));
        assert!(findings[0].detail.contains("failing closed"));
    }

    #[test]
    fn classify_missing_body_fails_closed() {
        let findings = classify_response(Bridge::Jms, "0.8.8", 200, None)
            .expect_err("missing body fails closed");

        assert_eq!(findings.len(), 1);
        assert!(findings[0].detail.contains("invalid JSON"));
    }

    #[test]
    fn classify_transport_error_fails_closed() {
        let finding = classify_transport_error("jms-bridge-v0.8.8", "timed out");

        assert!(finding.detail.contains("timed out"));
        assert!(finding.detail.contains("failing closed"));
        assert_eq!(finding.url, release_api_url("jms-bridge-v0.8.8"));
    }
}

//! Real Keycloak JWKS validation test.
//!
//! All HTTP calls are performed inside the Keycloak container via `exec`
//! to avoid relying on Docker published-port networking which may be
//! unavailable in certain environments.
//!
//! Requires Docker and `integration-tests` feature.

#![cfg(feature = "integration-tests")]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_auth::{
    AuthError, ClaimPaths, JsonPointerClaimsMapper, Jwk, JwksProvider, LocalJwtValidator,
    TokenAuthenticator,
};
use testcontainers::core::{ExecCommand, WaitFor};
use testcontainers::{ContainerAsync, GenericImage, ImageExt, runners::AsyncRunner};

// ---------------------------------------------------------------------------
// Container-exec HTTP helpers
// ---------------------------------------------------------------------------

/// Perform an HTTP GET inside the container using bash `/dev/tcp`.
/// Returns the response body as a String (headers stripped).
/// Optionally includes a Bearer auth token.
async fn exec_get(
    container: &ContainerAsync<GenericImage>,
    path: &str,
    token: Option<&str>,
) -> String {
    let auth_header = match token {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let script = format!(
        r#"
exec 3<>/dev/tcp/127.0.0.1/8080
printf 'GET {path} HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n{auth_header}Connection: close\r\n\r\n' >&3
timeout 5 cat <&3 | sed '1,/^\r$/d'
exec 3>&-
"#,
    );
    let mut result = container
        .exec(ExecCommand::new(vec!["/bin/bash", "-c", &script]))
        .await
        .expect("exec GET failed");
    let bytes = result.stdout_to_vec().await.expect("stdout read failed");
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Perform an HTTP request with a JSON body inside the container using bash `/dev/tcp`.
/// The body is passed via base64 to avoid shell-escaping issues.
/// Returns the HTTP status code and response body.
async fn exec_json_request(
    container: &ContainerAsync<GenericImage>,
    method: &str,
    path: &str,
    token: Option<&str>,
    json_body: &str,
) -> (u16, String) {
    let b64 = base64_encode(json_body);
    let auth_header = match token {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let script = format!(
        r#"
BODY=$(printf '%s' '{b64}' | base64 -d)
LEN=${{#BODY}}
exec 3<>/dev/tcp/127.0.0.1/8080
printf '{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n{auth_header}Content-Type: application/json\r\nContent-Length: %s\r\nConnection: close\r\n\r\n' "$LEN" >&3
printf '%s' "$BODY" >&3
timeout 5 cat <&3
exec 3>&-
"#,
    );
    let mut result = container
        .exec(ExecCommand::new(vec!["/bin/bash", "-c", &script]))
        .await
        .expect("exec JSON request failed");
    let bytes = result.stdout_to_vec().await.expect("stdout read failed");
    let raw = String::from_utf8_lossy(&bytes).to_string();
    parse_http_response(&raw)
}

/// Perform a form-urlencoded POST inside the container using bash `/dev/tcp`.
/// Returns the HTTP status code and response body.
async fn exec_post_form(
    container: &ContainerAsync<GenericImage>,
    path: &str,
    form_body: &str,
) -> (u16, String) {
    let b64 = base64_encode(form_body);
    let script = format!(
        r#"
BODY=$(printf '%s' '{b64}' | base64 -d)
LEN=${{#BODY}}
exec 3<>/dev/tcp/127.0.0.1/8080
printf 'POST {path} HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: %s\r\nConnection: close\r\n\r\n' "$LEN" >&3
printf '%s' "$BODY" >&3
timeout 5 cat <&3
exec 3>&-
"#,
    );
    let mut result = container
        .exec(ExecCommand::new(vec!["/bin/bash", "-c", &script]))
        .await
        .expect("exec POST form failed");
    let bytes = result.stdout_to_vec().await.expect("stdout read failed");
    let raw = String::from_utf8_lossy(&bytes).to_string();
    parse_http_response(&raw)
}

/// Encode a string as base64 without any external dependency.
fn base64_encode(input: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((b1 & 0x0F) << 2) | (b2 >> 6)] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[b2 & 0x3F] as char
        } else {
            '='
        });
    }
    out
}

/// Parse a raw HTTP response into (status_code, body).
fn parse_http_response(raw: &str) -> (u16, String) {
    let status_code = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);

    let body = raw
        .find("\r\n\r\n")
        .map(|idx| raw[idx + 4..].trim().to_string())
        .unwrap_or_else(|| {
            raw.find("\n\n")
                .map(|idx| raw[idx + 2..].trim().to_string())
                .unwrap_or_default()
        });

    (status_code, body)
}

// ---------------------------------------------------------------------------
// Pre-fetches real JWKS keys from Keycloak, then serves them statically so
// camel-auth does not need a public localhost HTTP JWKS constructor.
// ---------------------------------------------------------------------------

struct StaticJwksProvider {
    keys: Vec<Jwk>,
}

#[async_trait]
impl JwksProvider for StaticJwksProvider {
    async fn get_signing_keys(&self) -> Result<Vec<Jwk>, AuthError> {
        Ok(self.keys.clone())
    }

    async fn refresh(&self) -> Result<(), AuthError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn real_keycloak_password_token_validates_via_jwks() {
    tokio::time::timeout(
        Duration::from_secs(90),
        real_keycloak_password_token_validates_via_jwks_inner(),
    )
    .await
    .expect("real Keycloak test timed out after 90s");
}

async fn real_keycloak_password_token_validates_via_jwks_inner() {
    let image = GenericImage::new("quay.io/keycloak/keycloak", "26.0")
        .with_wait_for(WaitFor::Nothing)
        .with_env_var("KC_BOOTSTRAP_ADMIN_USERNAME", "admin")
        .with_env_var("KC_BOOTSTRAP_ADMIN_PASSWORD", "admin")
        .with_cmd(vec![
            "start-dev",
            "--http-enabled=true",
            "--hostname-strict=false",
            "--health-enabled=true",
        ])
        .with_startup_timeout(Duration::from_secs(5));

    eprintln!("starting Keycloak test container");
    let container = image
        .start()
        .await
        .expect("Keycloak container failed to start");
    eprintln!("Keycloak container started, id={}", container.id());

    // Keycloak's internal base URL (reachable inside the container)
    let base = "http://127.0.0.1:8080";

    wait_for_keycloak(&container).await;

    let admin_token = keycloak_token(&container, "master", "admin-cli", "admin", "admin")
        .await
        .expect("admin token");

    create_realm(&container, &admin_token).await;
    create_public_client(&container, &admin_token).await;
    create_user(&container, &admin_token).await;

    let user_token = keycloak_token(&container, "camel-it", "camel-api", "alice", "alice-pass")
        .await
        .expect("user token");

    // Pre-fetch JWKS inside the container
    let jwks_path = "/realms/camel-it/protocol/openid-connect/certs";
    let jwks_json = exec_get(&container, jwks_path, None).await;

    #[derive(serde::Deserialize)]
    struct JwksResponse {
        keys: Vec<Jwk>,
    }
    let jwks_data: JwksResponse = serde_json::from_str(&jwks_json).expect("JWKS JSON parse failed");

    let jwks: Arc<dyn JwksProvider> = Arc::new(StaticJwksProvider {
        keys: jwks_data.keys,
    });

    let mapper = Arc::new(JsonPointerClaimsMapper::new(ClaimPaths {
        subject: "/sub".into(),
        roles: vec![
            "/realm_access/roles".into(),
            "/resource_access/camel-api/roles".into(),
        ],
        scopes: Some("/scope".into()),
    }));
    let issuer = format!("{base}/realms/camel-it");
    let validator = LocalJwtValidator::new(vec!["account".into()], issuer, jwks, mapper);

    let principal = validator.authenticate_bearer(&user_token).await.unwrap();
    // Keycloak 26 uses UUID as the subject (not username)
    assert!(!principal.subject.is_empty(), "subject should not be empty");
    assert!(
        principal.subject.contains('-'),
        "subject should be a UUID, got: {}",
        principal.subject
    );
    assert!(
        principal.issuer.contains("/realms/camel-it"),
        "issuer should contain /realms/camel-it, got: {}",
        principal.issuer
    );
}

// ---------------------------------------------------------------------------
// Keycloak setup helpers (all via container exec)
// ---------------------------------------------------------------------------

async fn wait_for_keycloak(container: &ContainerAsync<GenericImage>) {
    let path = "/realms/master/.well-known/openid-configuration";
    for attempt in 0..30 {
        let body = exec_get(container, path, None).await;
        if body.contains("issuer") && body.contains("authorization_endpoint") {
            eprintln!("Keycloak ready after {attempt}s");
            return;
        }
        if attempt == 29 {
            panic!("Keycloak discovery not ready after 30 attempts. Last response: {body}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn create_realm(container: &ContainerAsync<GenericImage>, admin_token: &str) {
    let (status, body) = exec_json_request(
        container,
        "POST",
        "/admin/realms",
        Some(admin_token),
        &serde_json::json!({"realm": "camel-it", "enabled": true}).to_string(),
    )
    .await;
    assert!(
        status == 201 || status == 409,
        "create realm expected 201 or 409, got {status}: {body}"
    );
}

async fn create_public_client(container: &ContainerAsync<GenericImage>, admin_token: &str) {
    let (status, body) = exec_json_request(
        container,
        "POST",
        "/admin/realms/camel-it/clients",
        Some(admin_token),
        &serde_json::json!({
            "clientId": "camel-api",
            "publicClient": true,
            "directAccessGrantsEnabled": true,
            "standardFlowEnabled": false,
            "serviceAccountsEnabled": false
        })
        .to_string(),
    )
    .await;
    assert!(
        status == 201 || status == 409,
        "create client expected 201 or 409, got {status}: {body}"
    );
}

async fn create_user(container: &ContainerAsync<GenericImage>, admin_token: &str) {
    // Keycloak 26 requires email, firstName, lastName for password grant to work
    let json = serde_json::json!({
        "username": "alice",
        "email": "alice@example.com",
        "firstName": "Alice",
        "lastName": "Wonder",
        "enabled": true,
        "emailVerified": true,
        "requiredActions": []
    })
    .to_string();
    let (status, body) = exec_json_request(
        container,
        "POST",
        "/admin/realms/camel-it/users",
        Some(admin_token),
        &json,
    )
    .await;
    assert!(
        status == 201 || status == 409,
        "create user expected 201 or 409, got {status}: {body}"
    );

    // Find the user ID for password setup
    let user_body = exec_get(
        container,
        "/admin/realms/camel-it/users?username=alice",
        Some(admin_token),
    )
    .await;
    let users: serde_json::Value =
        serde_json::from_str(&user_body).expect("users list parse failed");
    let user_id = users
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|u| u["id"].as_str())
        .expect("user id not found")
        .to_string();

    // Set password via dedicated Keycloak endpoint (PUT)
    let pw_json = serde_json::json!({
        "type": "password",
        "value": "alice-pass",
        "temporary": false
    })
    .to_string();
    let (pw_status, pw_body) = exec_json_request(
        container,
        "PUT",
        &format!("/admin/realms/camel-it/users/{user_id}/reset-password"),
        Some(admin_token),
        &pw_json,
    )
    .await;
    assert!(
        pw_status == 204 || pw_status == 200,
        "set password expected 204, got {pw_status}: {pw_body}"
    );
}

async fn keycloak_token(
    container: &ContainerAsync<GenericImage>,
    realm: &str,
    client_id: &str,
    username: &str,
    password: &str,
) -> Result<String, String> {
    let path = format!("/realms/{realm}/protocol/openid-connect/token");
    let form = format!(
        "grant_type=password&client_id={client_id}&username={username}&password={password}"
    );
    let (status, body) = exec_post_form(container, &path, &form).await;

    if status != 200 {
        return Err(format!("token request returned {status}: {body}"));
    }

    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("token response parse failed: {e}"))?;

    parsed["access_token"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| format!("no access_token in response: {parsed}"))
}

//! Task 2.1 — Skills extension protocol surface: discover advertisement,
//! `skills/list`, and `skills/get`.
//! Task 2.2 — catalog visibility follows caller authentication.
//!
//! The server is the real thing: `McpServerRegistry::get_or_spawn` loads the
//! catalog from the configured `skills_root` and mounts the rmcp server
//! adapter (no mock). Each test binds its own loopback IP (repo convention for
//! the process-global registry): this file owns `127.0.0.81`–`127.0.0.95`.
//! `server/discover` is driven with raw HTTP JSON-RPC POSTs when the wire
//! shape of the capabilities matters, and the typed `skills/*` handlers are
//! driven with rmcp's discover-lifecycle client.

mod common;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use camel_api::CamelError;
use camel_api::security_policy::{
    AccessMode, CredentialSource, Principal, RouteSecurityPlan, TransportId,
};
use camel_auth::ProviderEntry;
use camel_auth::ProviderRegistry;
use camel_auth::ScopePolicy;
use camel_auth::TokenAuthenticator;
use camel_component_api::consumer::ExchangeEnvelope;
use camel_component_api::{
    Component, Consumer, ConsumerContext, NoOpComponentContext, NoopRuntimeObservability,
    RuntimeObservability, SecurityContext,
};
use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::component::McpComponent;
use camel_component_mcp::config::{McpGlobalConfig, McpServerConfig};
use camel_component_mcp::error::McpError;
use camel_component_mcp::registry::McpListenerHandle;
use rmcp::ClientLifecycleMode;
use rmcp::ServiceError;
use rmcp::model::{
    CacheScope, ErrorCode, GetSkillRequestParams, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ResourceContents, ResultType, SkillResources,
};
use rmcp::service::ClientServiceExt;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Protocol version every raw POST in this file carries.
const PROTOCOL_VERSION: &str = "2026-07-28";

/// Bound on one raw HTTP exchange step: connect, write, and read-to-EOF each
/// get their own 5-second deadline so a wedged listener fails the test
/// instead of parking it.
const RAW_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Source of unique loopback binds for this file (`127.0.0.81` onwards).
static NEXT_BIND: AtomicU8 = AtomicU8::new(0);

/// Source of unique temp directory names for this file.
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// The next bind this file owns: `127.0.0.81:0`, `127.0.0.82:0`, ...
fn next_bind() -> String {
    format!(
        "127.0.0.{}:0",
        81 + NEXT_BIND.fetch_add(1, Ordering::SeqCst)
    )
}

/// Temp directory owned by one test, removed on drop (no `tempfile` dev-dep
/// in this crate; mirrors the in-module `TestDir` in `src/skills.rs`).
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let sequence = NEXT_DIR.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "camel-mcp-skills-{tag}-{}-{sequence}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Write one skill: `SKILL.md` from `frontmatter` plus the extra files.
fn write_skill(root: &Path, dir: &str, frontmatter: &str, files: &[(&str, &str)]) {
    let skill = root.join(dir);
    std::fs::create_dir_all(&skill).expect("create skill dir");
    std::fs::write(
        skill.join("SKILL.md"),
        format!("---\n{frontmatter}\n---\n# {dir}\n"),
    )
    .expect("write SKILL.md");
    for (relative, contents) in files {
        let path = skill.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create supporting dir");
        }
        std::fs::write(&path, contents).expect("write supporting file");
    }
}

/// The Task 1.1 `code-review` fixture with its supporting file.
fn write_code_review_fixture(root: &Path) {
    write_skill(
        root,
        "code-review",
        "name: code-review\ndescription: Review code.\nlicense: MIT",
        &[("references/checklist.md", "# checklist\n")],
    );
}

fn server_config(bind: &str, root: Option<&Path>) -> McpServerConfig {
    let mut config = serde_json::json!({ "bind": bind });
    if let Some(root) = root {
        config["skills_root"] = serde_json::Value::String(root.display().to_string());
    }
    serde_json::from_value(config).expect("valid server config")
}

/// Spawn the real listener for `bind`, optionally with a skills root.
async fn spawn_with_root(bind: &str, root: Option<&Path>) -> (SocketAddr, Arc<McpListenerHandle>) {
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &server_config(bind, root))
        .await
        .expect("listener must spawn");
    (handle.local_addr, handle)
}

/// One parsed HTTP reply.
struct RawHttpResponse {
    status: u16,
    body: String,
}

impl RawHttpResponse {
    /// The reply body parsed as JSON (every JSON-RPC reply in these tests is
    /// a single object).
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("reply must be JSON")
    }
}

/// POST one JSON-RPC message to the server's `/mcp` route. `Connection:
/// close` makes the reply EOF-terminated, so no chunked/length parsing is
/// needed; every socket step is deadline-bounded.
async fn raw_json_rpc_post(
    addr: SocketAddr,
    body: &serde_json::Value,
    extra_headers: &[(&str, &str)],
) -> RawHttpResponse {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let payload = body.to_string();
    // Streamable HTTP requires Accept to name both JSON and SSE.
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nConnection: close\r\n\
         Content-Length: {}\r\n",
        payload.len()
    );
    for (name, value) in extra_headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&payload);

    let mut stream = tokio::time::timeout(RAW_IO_TIMEOUT, tokio::net::TcpStream::connect(addr))
        .await
        .expect("connect to listener timed out")
        .expect("connect to listener");
    tokio::time::timeout(RAW_IO_TIMEOUT, stream.write_all(request.as_bytes()))
        .await
        .expect("write JSON-RPC request timed out")
        .expect("write JSON-RPC request");
    let mut raw = Vec::new();
    tokio::time::timeout(RAW_IO_TIMEOUT, stream.read_to_end(&mut raw))
        .await
        .expect("read reply timed out")
        .expect("read reply to EOF");
    let (status, body) = parse_http_response(&raw);
    RawHttpResponse { status, body }
}

/// Split one HTTP reply into (status code, body). The reply is read to EOF
/// (`Connection: close`), so no length/chunked framing is parsed.
fn parse_http_response(raw: &[u8]) -> (u16, String) {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .expect("HTTP reply must separate head and body");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (status, body.to_owned())
}

/// POST one JSON-RPC request carrying the mandatory `_meta` pair, the
/// `MCP-Protocol-Version` header, and an `Mcp-Method` header equal to the
/// body method (SEP-2243 is required at `2026-07-28`). `server/discover`
/// included.
async fn raw_call(addr: SocketAddr, method: &str, params: serde_json::Value) -> RawHttpResponse {
    let mut params = params;
    params
        .as_object_mut()
        .expect("JSON-RPC params must be an object")
        .insert(
            "_meta".to_owned(),
            serde_json::json!({
                "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
                "io.modelcontextprotocol/clientCapabilities": {},
            }),
        );
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    raw_json_rpc_post(
        addr,
        &body,
        &[
            ("MCP-Protocol-Version", PROTOCOL_VERSION),
            ("Mcp-Method", method),
        ],
    )
    .await
}

/// The on-disk bytes of a manifest URI under `root`.
fn disk_bytes(root: &Path, uri: &str) -> Vec<u8> {
    let relative = uri.strip_prefix("skill://").expect("skill uri prefix");
    let decoded = percent_encoding::percent_decode_str(relative)
        .decode_utf8()
        .expect("skill uri is utf-8");
    std::fs::read(root.join(decoded.as_ref())).expect("read skill file")
}

/// The `ErrorData` a raw peer call answered.
fn peer_error(error: ServiceError) -> rmcp::model::ErrorData {
    match error {
        ServiceError::McpError(data) => data,
        other => panic!("expected a JSON-RPC error, got {other:?}"),
    }
}

#[tokio::test]
async fn skills_extension_not_advertised_without_root() {
    let bind = next_bind();
    let (addr, _handle) = spawn_with_root(&bind, None).await;

    let discover = raw_call(addr, "server/discover", serde_json::json!({})).await;
    assert_eq!(
        discover.status, 200,
        "server/discover must be served, got status {} body {}",
        discover.status, discover.body
    );
    let value = discover.json();
    let capabilities = value["result"]["capabilities"]
        .as_object()
        .expect("discover must carry a capabilities object");
    assert!(
        capabilities.contains_key("tools"),
        "tools capability must be advertised: {value}"
    );
    assert!(
        capabilities.contains_key("resources"),
        "resources capability must be advertised: {value}"
    );
    assert!(
        !capabilities.contains_key("extensions"),
        "a listener without a skills root must advertise no extensions: {value}"
    );

    let list = raw_call(addr, "skills/list", serde_json::json!({})).await;
    let value = list.json();
    assert_eq!(
        value["error"]["code"],
        serde_json::json!(-32601),
        "skills/list without a skills root must be method-not-found: {value}"
    );
}

#[tokio::test]
async fn skills_list_returns_complete_manifest_with_digests() {
    let root = TempDir::new("manifest");
    write_code_review_fixture(root.path());
    write_skill(
        root.path(),
        "deploy",
        "name: deploy\ndescription: Deploy services.",
        &[],
    );
    let bind = next_bind();
    let (addr, _handle) = spawn_with_root(&bind, Some(root.path())).await;

    let discover = raw_call(addr, "server/discover", serde_json::json!({})).await;
    assert_eq!(discover.status, 200, "discover must be served");
    let discovered = discover.json();
    assert_eq!(
        discovered["result"]["capabilities"]["extensions"]["io.modelcontextprotocol/skills"],
        serde_json::json!({ "directoryRead": false }),
        "a skills root must advertise the extension with directoryRead false: {discovered}"
    );

    let client = common::raw_rmcp_client(addr).await;
    let list = client
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert_eq!(
        list.result_type,
        Some(ResultType::COMPLETE),
        "skills/list must be complete"
    );
    assert_eq!(list.ttl_ms, Some(0), "skills/list must carry ttlMs 0");
    assert_eq!(
        list.cache_scope,
        Some(CacheScope::Private),
        "skills/list must be private"
    );
    assert_eq!(list.skills.len(), 2, "both skills must be listed");

    let code_review = list
        .skills
        .iter()
        .find(|skill| skill.uri == "skill://code-review/SKILL.md")
        .expect("code-review must be listed");
    assert_eq!(
        code_review.frontmatter.name, "code-review",
        "the frontmatter name must be preserved"
    );
    assert_eq!(
        code_review.frontmatter.description, "Review code.",
        "the frontmatter description must be preserved"
    );
    assert_eq!(
        code_review.frontmatter.extra.get("license"),
        Some(&serde_json::json!("MIT")),
        "extra frontmatter fields must be preserved"
    );
    let SkillResources::Files(files) = &code_review.resources else {
        panic!(
            "a static skill must list its files, got {:?}",
            code_review.resources
        );
    };
    assert_eq!(files.len(), 2, "SKILL.md and references/checklist.md");
    for file in files {
        let bytes = disk_bytes(root.path(), &file.uri);
        file.verify(&bytes)
            .expect("each manifest digest and size must match the served bytes");
    }

    let directory = raw_call(
        addr,
        "resources/directory/read",
        serde_json::json!({ "uri": "skill://code-review" }),
    )
    .await;
    let value = directory.json();
    assert_eq!(
        value["error"]["code"],
        serde_json::json!(-32601),
        "directoryRead false must answer method-not-found: {value}"
    );
}

#[tokio::test]
async fn skills_get_by_uri_matches_list_entry() {
    let root = TempDir::new("get");
    write_code_review_fixture(root.path());
    let bind = next_bind();
    let (addr, _handle) = spawn_with_root(&bind, Some(root.path())).await;

    let client = common::raw_rmcp_client(addr).await;
    let list = client
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    let code_review = list
        .skills
        .iter()
        .find(|skill| skill.uri == "skill://code-review/SKILL.md")
        .expect("code-review must be listed")
        .clone();

    let get = client
        .peer()
        .get_skill(GetSkillRequestParams::new(code_review.uri.clone()))
        .await
        .expect("skills/get must succeed");
    assert_eq!(
        get.skill, code_review,
        "skills/get must return the same entry skills/list returned"
    );
    assert_eq!(
        get.result_type,
        Some(ResultType::COMPLETE),
        "skills/get must be complete"
    );
    assert_eq!(get.ttl_ms, Some(0), "skills/get must carry ttlMs 0");
    assert_eq!(
        get.cache_scope,
        Some(CacheScope::Private),
        "skills/get must be private"
    );
}

#[tokio::test]
async fn skills_list_paginates_with_next_cursor() {
    let root = TempDir::new("paginate");
    for index in 0..51 {
        let name = format!("skill-{index:02}");
        write_skill(
            root.path(),
            &name,
            &format!("name: {name}\ndescription: Skill {index}."),
            &[],
        );
    }
    let bind = next_bind();
    let (addr, _handle) = spawn_with_root(&bind, Some(root.path())).await;

    let client = common::raw_rmcp_client(addr).await;
    let first = client
        .peer()
        .list_skills(None)
        .await
        .expect("first page must succeed");
    assert_eq!(first.skills.len(), 50, "a page holds at most 50 skills");
    let cursor = first
        .next_cursor
        .clone()
        .expect("51 skills must leave a next cursor");

    let second = client
        .peer()
        .list_skills(Some(
            PaginatedRequestParams::default().with_cursor(Some(cursor)),
        ))
        .await
        .expect("second page must succeed");
    assert_eq!(
        second.skills.len(),
        1,
        "the second page holds the last skill"
    );
    assert_eq!(second.next_cursor, None, "the last page has no next cursor");

    let mut uris = HashSet::new();
    for skill in first.skills.iter().chain(second.skills.iter()) {
        assert!(
            uris.insert(skill.uri.clone()),
            "every skill must appear exactly once, saw {} twice",
            skill.uri
        );
        assert!(
            matches!(&skill.resources, SkillResources::Files(files) if !files.is_empty()),
            "every listed entry must carry a non-empty manifest: {}",
            skill.uri
        );
    }
    assert_eq!(uris.len(), 51, "all 51 skills must be listed across pages");
}

#[tokio::test]
async fn skills_bad_cursor_and_unknown_uri_are_invalid_params() {
    let root = TempDir::new("invalid");
    write_skill(
        root.path(),
        "code-review",
        "name: code-review\ndescription: Review code.",
        &[],
    );
    let bind = next_bind();
    let (addr, _handle) = spawn_with_root(&bind, Some(root.path())).await;

    let client = common::raw_rmcp_client(addr).await;
    for cursor in ["not-a-number", "999"] {
        let error = client
            .peer()
            .list_skills(Some(
                PaginatedRequestParams::default().with_cursor(Some(cursor.to_owned())),
            ))
            .await
            .expect_err("a malformed or out-of-range cursor must be rejected");
        let data = peer_error(error);
        assert_eq!(
            data.code,
            ErrorCode::INVALID_PARAMS,
            "cursor {cursor:?} must answer -32602, got {data:?}"
        );
    }

    let error = client
        .peer()
        .get_skill(GetSkillRequestParams::new("skill://nope/SKILL.md"))
        .await
        .expect_err("an unknown skill URI must be rejected");
    let data = peer_error(error);
    assert_eq!(
        data.code,
        ErrorCode::INVALID_PARAMS,
        "an unknown skill URI must answer -32602, got {data:?}"
    );
}

#[tokio::test]
async fn skills_list_follows_catalog_authentication() {
    let root = TempDir::new("auth");
    write_code_review_fixture(root.path());
    let bind = next_bind();
    let (addr, handle) = spawn_with_root(&bind, Some(root.path())).await;
    let owner = Arc::new(());
    handle.security.register_plan(
        "protected",
        scoped_plan(&["skills:read"]),
        Some(provider_registry(&[(
            "sec-token",
            test_principal(&["skills:read"]),
        )])),
        Arc::downgrade(&owner),
    );
    // The production path commits after `get_or_spawn`; a direct book
    // registration commits explicitly, or the catalog would stay public.
    handle.security.commit_protection();

    let anonymous = common::raw_rmcp_client(addr).await;
    let listed = anonymous
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert!(
        listed.skills.is_empty(),
        "an unauthenticated caller must see an empty catalog, got {:?}",
        listed.skills
    );

    let authenticated = rmcp_client_with_token(addr, "sec-token").await;
    let listed = authenticated
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert_eq!(
        listed.skills.len(),
        1,
        "a caller holding the skills:read scope must see the skill"
    );

    // Auth-first: a denied caller sees the same "not available" answer for a
    // hidden URI, an unknown URI, and an escaping URI, so the response never
    // reveals which of them exists. (On a public bind the escaping URI still
    // takes the "escapes" branch — see `skill_path_escape_is_rejected`.)
    for uri in [
        "skill://code-review/SKILL.md",
        "skill://nope/SKILL.md",
        "skill://code-review/../../outside.txt",
    ] {
        let error = anonymous
            .peer()
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .expect_err("a denied caller must not read any skill URI");
        let data = peer_error(error);
        assert_eq!(
            data.code,
            ErrorCode::INVALID_PARAMS,
            "denied URI {uri} must answer -32602, got {data:?}"
        );
        assert_eq!(
            data.message,
            format!("skill '{uri}' is not available"),
            "a denied caller must not distinguish hidden, unknown, and escaping URIs"
        );
    }
}

#[tokio::test]
async fn skills_latch_ignores_refused_duplicate_plan() {
    let root = TempDir::new("latch");
    write_code_review_fixture(root.path());
    let bind = next_bind();
    let (addr, handle) = spawn_with_root(&bind, Some(root.path())).await;
    let owner_a = Arc::new(());
    let owner_b = Arc::new(());
    handle
        .security
        .register_plan("r1", public_plan(), None, Arc::downgrade(&owner_a));
    handle.security.commit_protection();
    // The keep-incumbent path returns before the insert: a refused duplicate
    // must not latch the bind.
    handle.security.register_plan(
        "r1",
        scoped_plan(&["skills:read"]),
        Some(provider_registry(&[(
            "sec-token",
            test_principal(&["skills:read"]),
        )])),
        Arc::downgrade(&owner_b),
    );
    handle.security.commit_protection();

    assert!(
        !handle.security.protected_seen(),
        "a refused duplicate must not latch the bind"
    );

    let client = common::raw_rmcp_client(addr).await;
    let listed = client
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert_eq!(
        listed.skills.len(),
        1,
        "an unlatched bind lists the catalog to every caller"
    );
}

#[tokio::test]
async fn skills_stay_hidden_after_protected_route_stops() {
    let root = TempDir::new("stopped");
    write_code_review_fixture(root.path());
    let bind = next_bind();
    let (addr, handle) = spawn_with_root(&bind, Some(root.path())).await;
    let owner = Arc::new(());
    handle.security.register_plan(
        "protected",
        scoped_plan(&["skills:read"]),
        Some(provider_registry(&[(
            "sec-token",
            test_principal(&["skills:read"]),
        )])),
        Arc::downgrade(&owner),
    );
    handle.security.commit_protection();
    handle.security.unregister_plan("protected");
    assert!(
        handle.security.protected_seen(),
        "the latch must survive the protected route's stop"
    );

    let client = common::raw_rmcp_client(addr).await;
    let listed = client
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert!(
        listed.skills.is_empty(),
        "no live protected plan must keep the catalog hidden, got {:?}",
        listed.skills
    );

    let error = client
        .peer()
        .get_skill(GetSkillRequestParams::new("skill://code-review/SKILL.md"))
        .await
        .expect_err("a hidden skill must not be served");
    let data = peer_error(error);
    assert_eq!(
        data.code,
        ErrorCode::INVALID_PARAMS,
        "a hidden skill must answer -32602, got {data:?}"
    );
}

#[tokio::test]
async fn skills_policy_is_evaluated_per_plan() {
    let root = TempDir::new("per-plan");
    write_code_review_fixture(root.path());
    let bind = next_bind();
    let (addr, handle) = spawn_with_root(&bind, Some(root.path())).await;
    let providers = provider_registry(&[("sec-token", test_principal(&["skills:read"]))]);
    let owner_a = Arc::new(());
    let owner_b = Arc::new(());
    handle.security.register_plan(
        "b",
        scoped_plan(&["admin"]),
        Some(providers.clone()),
        Arc::downgrade(&owner_a),
    );
    handle.security.register_plan(
        "a",
        scoped_plan(&["skills:read"]),
        Some(providers.clone()),
        Arc::downgrade(&owner_b),
    );
    handle.security.commit_protection();

    let client = rmcp_client_with_token(addr, "sec-token").await;
    let listed = client
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert_eq!(
        listed.skills.len(),
        1,
        "the admin plan's denial must not shadow the skills:read grant"
    );
}

#[tokio::test]
async fn skill_file_is_served_through_resources_read() {
    let root = TempDir::new("read-serve");
    write_code_review_fixture(root.path());
    // A third file with a space in its name: its manifest URI is percent-
    // encoded, so it proves the advertised spelling resolves to the file.
    std::fs::write(
        root.path().join("code-review/references/my notes.md"),
        "notes\n",
    )
    .expect("write spaced fixture");
    // A file name carrying LITERAL backslashes (valid on Unix): its manifest
    // URI percent-encodes them as `%5C`, and the round-trip below proves the
    // catalog serves it from the manifest. `skill_uri_escapes` only guards
    // non-manifest URIs, so a manifest URI containing `%5C` must not be
    // rejected.
    #[cfg(unix)]
    std::fs::write(
        root.path()
            .join("code-review/references/weird\\..\\name.md"),
        "weird\n",
    )
    .expect("write backslash fixture");

    let bind = next_bind();
    let (addr, _handle) = spawn_with_root(&bind, Some(root.path())).await;

    let client = common::raw_rmcp_client(addr).await;
    let mut expected: Vec<(&str, &str)> = vec![
        ("skill://code-review/SKILL.md", "code-review/SKILL.md"),
        (
            "skill://code-review/references/checklist.md",
            "code-review/references/checklist.md",
        ),
        (
            "skill://code-review/references/my%20notes.md",
            "code-review/references/my notes.md",
        ),
    ];
    // A file name carrying LITERAL backslashes (valid on Unix) is advertised
    // as a percent-encoded `%5C` segment; the round-trip below proves it is
    // served from the manifest, not rejected by `skill_uri_escapes` (which
    // only guards non-manifest URIs).
    #[cfg(unix)]
    expected.push((
        "skill://code-review/references/weird%5C..%5Cname.md",
        "code-review/references/weird\\..\\name.md",
    ));
    for &(uri, relative) in &expected {
        let result = client
            .peer()
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .unwrap_or_else(|error| panic!("skill file {uri} must be served, got {error:?}"));
        assert_eq!(result.ttl_ms, Some(0), "{uri} must carry ttlMs 0");
        assert_eq!(
            result.cache_scope,
            Some(CacheScope::Private),
            "{uri} must be private"
        );
        let expected_bytes =
            std::fs::read(root.path().join(relative)).expect("read skill fixture file");
        let expected_text = std::str::from_utf8(&expected_bytes).expect("skill fixture is UTF-8");
        match &result.contents[0] {
            ResourceContents::TextResourceContents {
                text, mime_type, ..
            } => {
                assert_eq!(text, expected_text, "{uri} must return the on-disk bytes");
                assert_eq!(
                    mime_type.as_deref(),
                    Some("text/markdown"),
                    "{uri} must be served as text/markdown"
                );
            }
            other => panic!("expected text contents for {uri}, got {other:?}"),
        }
    }

    // Manifest-to-read round trip: every URI `skills/list` advertises must be
    // readable and return exactly its on-disk bytes.
    let manifest = client
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    let code_review = manifest
        .skills
        .iter()
        .find(|skill| skill.uri == "skill://code-review/SKILL.md")
        .expect("code-review must be listed");
    let SkillResources::Files(files) = &code_review.resources else {
        panic!(
            "a static skill must list its files, got {:?}",
            code_review.resources
        );
    };
    let on_disk: HashMap<&str, &str> = expected.iter().copied().collect();
    assert_eq!(
        files.len(),
        on_disk.len(),
        "the manifest must advertise exactly the fixture files: {:?}",
        files.iter().map(|file| &file.uri).collect::<Vec<_>>()
    );
    for file in files {
        let relative = on_disk
            .get(file.uri.as_str())
            .unwrap_or_else(|| panic!("manifest advertises unmapped URI {}", file.uri));
        let expected_bytes =
            std::fs::read(root.path().join(relative)).expect("read skill fixture file");
        let expected_text = std::str::from_utf8(&expected_bytes).expect("skill fixture is UTF-8");
        let result = client
            .peer()
            .read_resource(ReadResourceRequestParams::new(file.uri.clone()))
            .await
            .unwrap_or_else(|error| {
                panic!("manifest URI {} must be readable, got {error:?}", file.uri)
            });
        match &result.contents[0] {
            ResourceContents::TextResourceContents { text, .. } => assert_eq!(
                text, expected_text,
                "manifest URI {} must return its on-disk bytes",
                file.uri
            ),
            other => panic!("expected text contents for {}, got {other:?}", file.uri),
        }
    }
}

#[tokio::test]
async fn skill_file_read_follows_catalog_authentication() {
    let root = TempDir::new("read-auth");
    write_code_review_fixture(root.path());
    let bind = next_bind();
    let (addr, handle) = spawn_with_root(&bind, Some(root.path())).await;
    let owner = Arc::new(());
    handle.security.register_plan(
        "protected",
        scoped_plan(&["skills:read"]),
        Some(provider_registry(&[(
            "sec-token",
            test_principal(&["skills:read"]),
        )])),
        Arc::downgrade(&owner),
    );
    handle.security.commit_protection();

    let uri = "skill://code-review/SKILL.md";
    let error = common::raw_rmcp_client(addr)
        .await
        .peer()
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .expect_err("an unauthenticated caller must not read a hidden skill file");
    assert_eq!(
        peer_error(error).code,
        ErrorCode::INVALID_PARAMS,
        "a hidden skill file must answer -32602"
    );

    let authenticated = rmcp_client_with_token(addr, "sec-token").await;
    let result = authenticated
        .peer()
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .expect("a caller holding the skills:read scope must read the file");
    let expected = disk_bytes(root.path(), uri);
    let expected = std::str::from_utf8(&expected).expect("skill fixture is UTF-8");
    match &result.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => {
            assert_eq!(
                text, expected,
                "the authenticated read returns the file text"
            )
        }
        other => panic!("expected text contents, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn escaping_symlink_fails_spawn() {
    let outer = TempDir::new("symlink-outer");
    let outside_file = outer.path().join("outside.txt");
    std::fs::write(&outside_file, "outside the root\n").expect("write outside.txt");
    let root = outer.path().join("root");
    std::fs::create_dir_all(&root).expect("create skills root");
    write_skill(
        &root,
        "escape",
        "name: escape\ndescription: Escape attempt.",
        &[],
    );
    std::os::unix::fs::symlink(&outside_file, root.join("escape").join("leak.md"))
        .expect("create escaping symlink");

    let bind = next_bind();
    let cfg = server_config(&bind, Some(&root));
    let error = McpServerRegistry::global()
        .get_or_spawn(&bind, &cfg)
        .await
        .expect_err("an escaping symlink must fail the spawn");
    assert!(
        matches!(error, McpError::SkillPathEscape { .. }),
        "expected SkillPathEscape, got {error:?}"
    );

    // The catalog load fails before the bind and no handle is ever installed:
    // a retry fails the same way instead of reusing a listener that serves the
    // symlink target.
    let error = McpServerRegistry::global()
        .get_or_spawn(&bind, &cfg)
        .await
        .expect_err("no listener must have been spawned for the escaping root");
    assert!(
        matches!(error, McpError::SkillPathEscape { .. }),
        "expected SkillPathEscape on retry, got {error:?}"
    );
}

#[tokio::test]
async fn skill_path_escape_is_rejected() {
    let root = TempDir::new("escape-request");
    write_code_review_fixture(root.path());
    let bind = next_bind();
    let (addr, _handle) = spawn_with_root(&bind, Some(root.path())).await;

    let client = common::raw_rmcp_client(addr).await;
    for uri in [
        "skill://code-review/../../outside.txt",
        "skill://code-review/%2e%2e/%2e%2e/outside.txt",
        "skill://code-review/%2E%2E/%2E%2E/outside.txt",
        "skill://code-review%2F..%2F..%2Foutside.txt",
    ] {
        let error = client
            .peer()
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .expect_err("a dot-dot URI must never reach the filesystem");
        let data = peer_error(error);
        assert_eq!(
            data.code,
            ErrorCode::INVALID_PARAMS,
            "escape URI {uri} must answer -32602, got {data:?}"
        );
        assert!(
            data.message.contains("escapes"),
            "escape URI {uri} must name the escape, got: {}",
            data.message
        );
    }

    // A non-`skill://` URI keeps the registry path, even with a `..` segment.
    let error = client
        .peer()
        .read_resource(ReadResourceRequestParams::new("crm://x/../y"))
        .await
        .expect_err("an unregistered non-skill URI must be a clean error");
    let data = peer_error(error);
    assert_eq!(
        data.message, "resource 'crm://x/../y' is not available",
        "a non-skill URI must not take the escape branch"
    );
}

#[tokio::test]
async fn unknown_skill_file_and_resources_list_stay_closed() {
    let root = TempDir::new("unknown-file");
    write_code_review_fixture(root.path());
    let bind = next_bind();
    let (addr, _handle) = spawn_with_root(&bind, Some(root.path())).await;

    let client = common::raw_rmcp_client(addr).await;
    let error = client
        .peer()
        .read_resource(ReadResourceRequestParams::new(
            "skill://code-review/unknown.md",
        ))
        .await
        .expect_err("an unknown skill file must not be served");
    assert_eq!(
        peer_error(error).code,
        ErrorCode::INVALID_PARAMS,
        "an unknown skill file must answer -32602"
    );

    let listed = client
        .peer()
        .list_resources(None)
        .await
        .expect("resources/list must succeed");
    assert!(
        listed
            .resources
            .iter()
            .all(|resource| !resource.uri.starts_with("skill://")),
        "no listed resource may be a skill file, got {:?}",
        listed.resources
    );
}

fn component_with_server(name: &str, cfg: McpServerConfig) -> McpComponent {
    let mut servers = HashMap::new();
    servers.insert(name.to_string(), cfg);
    McpComponent::new(McpGlobalConfig {
        servers,
        remotes: HashMap::new(),
    })
}

fn rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoopRuntimeObservability)
}

/// URL-encode a value for a query parameter (the DSL lowering channel).
fn encoded(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// Create the consumer for a consumer-shaped URI (endpoint + consumer).
fn consumer_for(component: &McpComponent, uri: &str) -> Box<dyn Consumer> {
    let endpoint = component
        .create_endpoint(uri, &NoOpComponentContext)
        .expect("endpoint creation must succeed");
    endpoint
        .create_consumer(rt())
        .expect("consumer creation must succeed")
}

/// A hand-owned route channel: the test acts as the route's pipeline.
fn test_context() -> (ConsumerContext, mpsc::Receiver<ExchangeEnvelope>) {
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "test-route".to_string());
    (ctx, rx)
}

#[tokio::test]
async fn skill_uri_route_conflicts_with_skills_root() {
    let root = TempDir::new("route-conflict");
    write_skill(
        root.path(),
        "blocked",
        "name: blocked\ndescription: Blocked by the catalog.",
        &[],
    );
    let bind = next_bind();
    let cfg = server_config(&bind, Some(root.path()));
    let component = component_with_server("crm", cfg.clone());

    let endpoint = component
        .create_endpoint(
            "mcp:crm/resource/blocked?uri=skill%3A%2F%2Fblocked",
            &NoOpComponentContext,
        )
        .expect("endpoint creation must succeed");
    let mut consumer = endpoint
        .create_consumer(rt())
        .expect("consumer creation must succeed");
    let (ctx, _route_rx) = test_context();
    let error = consumer
        .start(ctx)
        .await
        .expect_err("a skill:// route must conflict with the skills catalog");
    assert!(
        error.to_string().contains("skills catalog"),
        "the start error must name the skills catalog conflict, got: {error}"
    );

    let handle = McpServerRegistry::global()
        .get_or_spawn(&bind, &cfg)
        .await
        .expect("shared listener");
    assert_eq!(
        handle.skills.as_ref().map(|catalog| catalog.len()),
        Some(1),
        "the catalog must keep its skill after the refused route"
    );

    let client = common::raw_rmcp_client(handle.local_addr).await;
    let result = client
        .peer()
        .read_resource(ReadResourceRequestParams::new("skill://blocked/SKILL.md"))
        .await
        .expect("the catalog must keep serving the skill file");
    let expected = disk_bytes(root.path(), "skill://blocked/SKILL.md");
    let expected = std::str::from_utf8(&expected).expect("skill fixture is UTF-8");
    match &result.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => {
            assert_eq!(text, expected, "the catalog read must return the file text")
        }
        other => panic!("expected text contents, got {other:?}"),
    }
}

/// Regression (holistic review round 2): a protected start that fails before
/// any listener exists must not leave the bind's protection latch behind. The
/// consumer registers its plan, `get_or_spawn` fails (invalid `skills_root`),
/// and the plan is released — `protected_seen` must follow, or a later
/// successful Public consumer on the same bind would hide every skill though
/// no listener was ever protected.
#[tokio::test]
async fn failed_protected_start_does_not_latch_the_bind() {
    let root = TempDir::new("failed-start");
    write_code_review_fixture(root.path());
    // A root that does not exist: the catalog load fails with `SkillLoad`.
    let missing_root = TempDir::new("failed-start-missing");
    let missing_path = missing_root.path().to_path_buf();
    std::fs::remove_dir_all(&missing_path).expect("remove the would-be skills root");

    let bind = next_bind();
    let broken_cfg = server_config(&bind, Some(&missing_path));
    let broken_component = component_with_server("crm", broken_cfg);
    let providers = provider_registry(&[("sec-token", test_principal(&["skills:read"]))]);
    let schema = encoded(r#"{"type":"object"}"#);

    let mut failed = consumer_for(
        &broken_component,
        &format!("mcp:crm/tool/broken?schema={schema}"),
    );
    failed.set_security_context(
        SecurityContext::from_plan(scoped_plan(&["skills:read"])).with_providers(providers),
    );
    let (ctx, _broken_rx) = test_context();
    let error = failed
        .start(ctx)
        .await
        .expect_err("a missing skills root must fail the protected start");
    match &error {
        CamelError::Config(message) => assert!(
            message.contains("skill load error"),
            "the start error must carry the typed SkillLoad reason, got: {message}"
        ),
        other => panic!("expected a SkillLoad config error, got {other:?}"),
    }

    // A second component/config on the SAME bind, valid root, and a Public
    // tool consumer: the failed protected start must not have latched the
    // bind, so the catalog is listed to every caller.
    let healthy_cfg = server_config(&bind, Some(root.path()));
    let healthy_component = component_with_server("crm", healthy_cfg.clone());
    let mut healthy = consumer_for(
        &healthy_component,
        &format!("mcp:crm/tool/healthy?schema={schema}"),
    );
    let (tx, _healthy_rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "healthy-route".to_string());
    healthy
        .start(ctx)
        .await
        .expect("the Public consumer must start after the failed protected start");

    let handle = McpServerRegistry::global()
        .get_or_spawn(&bind, &healthy_cfg)
        .await
        .expect("shared listener");
    assert!(
        !handle.security.protected_seen(),
        "a failed protected start must not latch the bind"
    );

    let client = common::raw_rmcp_client(handle.local_addr).await;
    let listed = client
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert_eq!(
        listed.skills.len(),
        1,
        "an unlatched bind lists the catalog to an unauthenticated caller, got {:?}",
        listed.skills
    );

    healthy.stop().await.expect("clean stop");
    if handle.serve_alive.load(Ordering::Acquire) {
        handle.monitor_task.abort();
    }
}

/// Regression (follow-up to the failed-start latch): a start that fails
/// against an already LIVE listener (config conflict) must not clear the
/// sticky skills latch. Only a start that never reached a listener may
/// release it.
#[tokio::test]
async fn conflicting_start_does_not_clear_a_live_latch() {
    let root = TempDir::new("conflict-latch");
    write_code_review_fixture(root.path());
    let bind = next_bind();
    let providers = provider_registry(&[("sec-token", test_principal(&["skills:read"]))]);
    let schema = encoded(r#"{"type":"object"}"#);

    // Component A spawns the listener with max_tools 128 and a protected tool
    // consumer, latching the bind.
    let mut cfg_a = server_config(&bind, Some(root.path()));
    cfg_a.max_tools = Some(128);
    let component_a = component_with_server("crm", cfg_a.clone());
    let mut first = consumer_for(&component_a, &format!("mcp:crm/tool/first?schema={schema}"));
    first.set_security_context(
        SecurityContext::from_plan(scoped_plan(&["skills:read"])).with_providers(providers),
    );
    let (ctx, _first_rx) = test_context();
    first
        .start(ctx)
        .await
        .expect("the protected consumer must start");

    let handle = McpServerRegistry::global()
        .get_or_spawn(&bind, &cfg_a)
        .await
        .expect("shared listener");
    assert!(
        handle.security.protected_seen(),
        "the protected registration must latch the bind"
    );

    // Component B declares a conflicting max_tools on the SAME bind: its start
    // fails in `get_or_spawn`'s conflict check and runs the cleanup path.
    let mut cfg_b = server_config(&bind, Some(root.path()));
    cfg_b.max_tools = Some(1);
    let component_b = component_with_server("crm", cfg_b);
    let mut second = consumer_for(
        &component_b,
        &format!("mcp:crm/tool/second?schema={schema}"),
    );
    let (tx, _second_rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "second-route".to_string());
    let error = second
        .start(ctx)
        .await
        .expect_err("a conflicting max_tools must fail the second start");
    assert!(
        error.to_string().contains("max_tools"),
        "the start error must name the conflicting cap, got: {error}"
    );

    assert!(
        handle.security.protected_seen(),
        "a live listener's latch must stay sticky across a failed start"
    );
    let client = common::raw_rmcp_client(handle.local_addr).await;
    let listed = client
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert!(
        listed.skills.is_empty(),
        "the live latch keeps the catalog hidden from an unauthenticated caller, got {:?}",
        listed.skills
    );

    // Stronger variant of the same path: stop the only protected consumer so
    // no live protected plan remains, while the listener stays alive. The
    // retried conflicting start must still leave the sticky latch alone.
    first.stop().await.expect("clean stop");
    assert!(
        handle.security.protected_seen(),
        "stopping a protected route must not clear the sticky latch"
    );
    let (tx, _retry_rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "second-route".to_string());
    let error = second
        .start(ctx)
        .await
        .expect_err("the retried conflicting start must fail the same way");
    assert!(
        error.to_string().contains("max_tools"),
        "the retried start error must name the conflicting cap, got: {error}"
    );
    assert!(
        handle.security.protected_seen(),
        "a failed start on a live listener must not clear the sticky latch"
    );

    if handle.serve_alive.load(Ordering::Acquire) {
        handle.monitor_task.abort();
    }
}

/// Regression (holistic review A): a protected route registers its plan on
/// the bind security book BEFORE `get_or_spawn`. Dead-server eviction must
/// carry that same book into the respawn: a fresh book would leave the new
/// listener with no plan and `protected_seen` false — a fail-open skills
/// catalog and unauthenticated tool dispatch.
#[tokio::test]
async fn protected_respawn_keeps_the_security_book() {
    let root = TempDir::new("respawn-book");
    write_code_review_fixture(root.path());
    let bind = next_bind();
    let cfg = server_config(&bind, Some(root.path()));
    let component = component_with_server("crm", cfg.clone());
    let providers = provider_registry(&[("sec-token", test_principal(&["skills:read"]))]);
    let schema = encoded(r#"{"type":"object"}"#);

    // The consumer registers its plan on the slot's book (before
    // `get_or_spawn`), then spawns the listener. Its plan is `admin`-only, so
    // the `skills:read` token can never authenticate against it.
    let mut first = consumer_for(&component, &format!("mcp:crm/tool/first?schema={schema}"));
    first.set_security_context(
        SecurityContext::from_plan(scoped_plan(&["admin"])).with_providers(providers.clone()),
    );
    let (tx, _first_rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "first-route-id".to_string());
    first
        .start(ctx)
        .await
        .expect("first protected consumer must start");

    let handle = McpServerRegistry::global()
        .get_or_spawn(&bind, &cfg)
        .await
        .expect("shared listener");
    let old_book = Arc::clone(&handle.security);
    assert!(
        old_book.protected_seen(),
        "the first protected registration must latch the book"
    );

    // Kill the listener so `get_or_spawn` evicts the slot and respawns.
    handle.monitor_task.abort();
    let mut stopped = false;
    for _ in 0..100 {
        if !handle.serve_alive.load(Ordering::Acquire) {
            stopped = true;
            break;
        }
        // Bounded poll interval (1s total), not synchronization: the
        // assertion below re-reads the atomic.
        tokio::time::sleep(Duration::from_millis(10)).await; // allow-test-sleep: bounded poll of the serve-liveness atomic
    }
    assert!(
        stopped,
        "the aborted listener must flip serve_alive within 1s"
    );

    // Drop the first consumer (and with it the owner Arc) after the abort: its
    // `admin` plan is now dead and must be pruned on read, not kept alive.
    drop(first);

    // A second protected route on the same bind registers its own plan on the
    // book, then triggers the respawn.
    let mut second = consumer_for(&component, &format!("mcp:crm/tool/second?schema={schema}"));
    second.set_security_context(
        SecurityContext::from_plan(scoped_plan(&["skills:read"])).with_providers(providers.clone()),
    );
    let (tx, _second_rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "second-route-id".to_string());
    second
        .start(ctx)
        .await
        .expect("second protected consumer must start after the eviction");

    let handle2 = McpServerRegistry::global()
        .get_or_spawn(&bind, &cfg)
        .await
        .expect("respawned shared listener");

    assert!(
        Arc::ptr_eq(&handle2.security, &old_book),
        "the respawned listener must mount the SAME security book the consumer registered on"
    );
    assert!(
        old_book.protected_seen(),
        "the protection latch must survive the respawn"
    );
    assert!(
        old_book.plan_for("first-route-id").is_none(),
        "the first owner's dead plan must be pruned"
    );
    assert!(
        old_book.plan_for("second-route-id").is_some(),
        "the second route's plan must be visible on the respawned book"
    );

    let anonymous = common::raw_rmcp_client(handle2.local_addr).await;
    let listed = anonymous
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert!(
        listed.skills.is_empty(),
        "an unauthenticated caller must see an empty catalog after the respawn, got {:?}",
        listed.skills
    );

    let authenticated = rmcp_client_with_token(handle2.local_addr, "sec-token").await;
    let listed = authenticated
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert_eq!(
        listed.skills.len(),
        1,
        "a caller holding the skills:read scope must see the skill after the respawn"
    );

    second.stop().await.expect("clean stop");
    if handle2.serve_alive.load(Ordering::Acquire) {
        handle2.monitor_task.abort();
    }
}

/// Regression (holistic review round 3): the committed-protection latch must
/// survive a FAILED respawn. Once a listener committed a live protected plan,
/// neither the abort nor a later start that fails before binding (invalid
/// `skills_root`) may un-hide the catalog of the listener that eventually
/// comes back on the same bind.
#[tokio::test]
async fn failed_respawn_keeps_the_committed_latch() {
    let root = TempDir::new("respawn-latch");
    write_code_review_fixture(root.path());
    // A root that does not exist: the respawn attempt fails with `SkillLoad`.
    let missing_root = TempDir::new("respawn-latch-missing");
    let missing_path = missing_root.path().to_path_buf();
    std::fs::remove_dir_all(&missing_path).expect("remove the would-be skills root");

    let bind = next_bind();
    let cfg = server_config(&bind, Some(root.path()));
    let component = component_with_server("crm", cfg.clone());
    let providers = provider_registry(&[("sec-token", test_principal(&["skills:read"]))]);
    let schema = encoded(r#"{"type":"object"}"#);

    // A protected consumer commits the latch when its listener comes up.
    let mut first = consumer_for(&component, &format!("mcp:crm/tool/guarded?schema={schema}"));
    first.set_security_context(
        SecurityContext::from_plan(scoped_plan(&["skills:read"])).with_providers(providers),
    );
    let (ctx, _first_rx) = test_context();
    first
        .start(ctx)
        .await
        .expect("the protected consumer must start");

    let handle = McpServerRegistry::global()
        .get_or_spawn(&bind, &cfg)
        .await
        .expect("shared listener");
    let old_book = Arc::clone(&handle.security);
    assert!(
        old_book.protected_seen(),
        "the listener start must commit the protected plan"
    );

    // Kill the listener so the next `get_or_spawn` evicts the slot (carrying
    // the security book, and with it the committed latch, forward).
    handle.monitor_task.abort();
    let mut stopped = false;
    for _ in 0..100 {
        if !handle.serve_alive.load(Ordering::Acquire) {
            stopped = true;
            break;
        }
        // Bounded poll interval (1s total), not synchronization: the
        // assertion below re-reads the atomic.
        tokio::time::sleep(Duration::from_millis(10)).await; // allow-test-sleep: bounded poll of the serve-liveness atomic
    }
    assert!(
        stopped,
        "the aborted listener must flip serve_alive within 1s"
    );

    // A start on the same bind whose catalog load fails: it must not clear
    // the committed latch.
    let broken_cfg = server_config(&bind, Some(&missing_path));
    let broken_component = component_with_server("crm", broken_cfg);
    let mut broken = consumer_for(
        &broken_component,
        &format!("mcp:crm/tool/broken?schema={schema}"),
    );
    broken.set_security_context(
        SecurityContext::from_plan(scoped_plan(&["skills:read"])).with_providers(
            provider_registry(&[("sec-token", test_principal(&["skills:read"]))]),
        ),
    );
    let (ctx, _broken_rx) = test_context();
    let error = broken
        .start(ctx)
        .await
        .expect_err("a missing skills root must fail the respawn attempt");
    match &error {
        CamelError::Config(message) => assert!(
            message.contains("skill load error"),
            "the start error must carry the typed SkillLoad reason, got: {message}"
        ),
        other => panic!("expected a SkillLoad config error, got {other:?}"),
    }
    assert!(
        old_book.protected_seen(),
        "a failed respawn must not clear the committed latch"
    );

    // A Public consumer on the same bind now spawns the listener for real: the
    // carried book keeps the committed latch, so the catalog stays hidden from
    // an unauthenticated caller.
    let mut public = consumer_for(&component, &format!("mcp:crm/tool/open?schema={schema}"));
    let (ctx, _public_rx) = test_context();
    public
        .start(ctx)
        .await
        .expect("the Public consumer must start after the failed respawn");

    let handle2 = McpServerRegistry::global()
        .get_or_spawn(&bind, &cfg)
        .await
        .expect("respawned shared listener");
    assert!(
        Arc::ptr_eq(&handle2.security, &old_book),
        "the respawned listener must mount the carried security book"
    );
    assert!(
        handle2.security.protected_seen(),
        "the committed latch must survive the failed respawn"
    );

    let anonymous = common::raw_rmcp_client(handle2.local_addr).await;
    let listed = anonymous
        .peer()
        .list_skills(None)
        .await
        .expect("skills/list must succeed");
    assert!(
        listed.skills.is_empty(),
        "the committed latch keeps the catalog hidden from an unauthenticated caller, got {:?}",
        listed.skills
    );

    public.stop().await.expect("clean stop");
    if handle2.serve_alive.load(Ordering::Acquire) {
        handle2.monitor_task.abort();
    }
}

/// Holistic review round 4: the skills flight and catalog are slot-owned, so
/// a listener generation that dies drops both. A respawn on the same bind must
/// re-read the tree and serve the new on-disk content, not the old catalog.
#[tokio::test]
async fn respawn_reloads_the_skills_catalog() {
    let root = TempDir::new("respawn-reload");
    write_skill(
        root.path(),
        "code-review",
        "name: code-review\ndescription: Review code.",
        &[("references/checklist.md", "# content A\n")],
    );
    let bind = next_bind();
    let (addr_a, handle_a) = spawn_with_root(&bind, Some(root.path())).await;

    let uri = "skill://code-review/references/checklist.md";
    let client_a = common::raw_rmcp_client(addr_a).await;
    let read = client_a
        .peer()
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .expect("generation A must serve the skill file");
    match &read.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => {
            assert_eq!(text, "# content A\n", "generation A serves content A")
        }
        other => panic!("expected text contents, got {other:?}"),
    }
    drop(client_a);

    handle_a.monitor_task.abort();
    let mut stopped = false;
    for _ in 0..100 {
        if !handle_a.serve_alive.load(Ordering::Acquire) {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await; // allow-test-sleep: bounded poll of the serve-liveness atomic
    }
    assert!(
        stopped,
        "the aborted listener must flip serve_alive within 1s"
    );

    std::fs::write(
        root.path().join("code-review/references/checklist.md"),
        "# content B\n",
    )
    .expect("rewrite the skill file");

    // Registry-level respawn: `get_or_spawn` on the same bind, NOT the dead
    // generation's handle/address.
    let (addr_b, handle_b) = spawn_with_root(&bind, Some(root.path())).await;
    assert!(
        !Arc::ptr_eq(&handle_a, &handle_b),
        "the respawn must install a new generation handle"
    );

    let client_b = common::raw_rmcp_client(addr_b).await;
    let read = client_b
        .peer()
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .expect("generation B must serve the skill file");
    match &read.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => assert_eq!(
            text, "# content B\n",
            "the respawned listener must serve the re-read catalog, not the old generation"
        ),
        other => panic!("expected text contents, got {other:?}"),
    }

    if handle_b.serve_alive.load(Ordering::Acquire) {
        handle_b.monitor_task.abort();
    }
}

/// A fresh discover-lifecycle rmcp client that authenticates every request
/// with `token` (rmcp sends it as `Authorization: Bearer <token>`).
async fn rmcp_client_with_token(
    addr: SocketAddr,
    token: &str,
) -> rmcp::service::RunningService<rmcp::service::RoleClient, ()> {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
            .auth_header(token),
    );
    ().serve_with_lifecycle(
        transport,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await
    .expect("discover-lifecycle connect against the component server")
}

fn test_principal(scopes: &[&str]) -> Principal {
    Principal {
        subject: "tester".into(),
        issuer: "test".into(),
        audience: vec![],
        scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
        roles: vec![],
        claims: serde_json::json!({}),
    }
}

struct TestAuthenticator {
    tokens: Vec<(String, Principal)>,
}

#[async_trait::async_trait]
impl TokenAuthenticator for TestAuthenticator {
    async fn authenticate_bearer(&self, token: &str) -> Result<Principal, CamelError> {
        self.tokens
            .iter()
            .find(|(candidate, _)| candidate == token)
            .map(|(_, principal)| principal.clone())
            .ok_or_else(|| CamelError::Unauthenticated("invalid credential".into()))
    }
}

fn provider_registry(tokens: &[(&str, Principal)]) -> Arc<ProviderRegistry> {
    let registry = ProviderRegistry::new();
    registry.register(
        "test-idp",
        ProviderEntry {
            authenticator: Arc::new(TestAuthenticator {
                tokens: tokens
                    .iter()
                    .map(|(token, principal)| ((*token).to_string(), principal.clone()))
                    .collect(),
            }),
            audience_binding: None,
        },
    );
    Arc::new(registry)
}

fn scoped_plan(scopes: &[&str]) -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authorized(Arc::new(ScopePolicy::new(
            scopes.iter().map(|scope| (*scope).to_string()).collect(),
            false,
        ))),
        provider_ref: Some("test-idp".into()),
        transport: TransportId::Mcp,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    }
}

fn public_plan() -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Public,
        provider_ref: None,
        transport: TransportId::Mcp,
        credential_sources: vec![],
        audience_binding: None,
    }
}

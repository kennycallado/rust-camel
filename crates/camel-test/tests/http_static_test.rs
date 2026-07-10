//! Integration tests for http-static component.
//!
//! Tests verify: shared-port API + static, precompressed content-encoding,
//! config precedence (cacheControl), SPA fallback, mount prefixes, segment
//! boundaries, error pages, path traversal, and consumer lifecycle.
//!
//! Requires `integration-tests` feature to compile and run.

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use camel_component_api::{
    Component, ComponentBundle, Consumer, ConsumerContext, ExchangeEnvelope, NoOpComponentContext,
};
use camel_component_http::registry::{MountMode, StaticMount};
use camel_component_http::{
    HttpStaticBundle, HttpStaticComponent, HttpStaticConfig, HttpStaticConsumer, RequestEnvelope,
    ServerRegistry,
};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;

// ---------------------------------------------------------------------------
// Test mutex — serialises all tests that touch the global ServerRegistry
// ---------------------------------------------------------------------------

static TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Bind to port 0 to obtain a free port, then drop the listener.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("failed to bind")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// Create a test ConsumerContext with a controllable cancellation token.
fn test_ctx() -> (ConsumerContext, CancellationToken, Arc<Notify>) {
    let (_tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let token = CancellationToken::new();
    let notify = Arc::new(Notify::new());
    let ctx = ConsumerContext::new(_tx, token.clone(), "http-static-test-route".to_string());
    (ctx, token, notify)
}

/// Start a static consumer in a background task; returns the cancel token.
async fn start_static_consumer(
    config: HttpStaticConfig,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let (ctx, token, _notify) = test_ctx();
    let rt = Arc::new(NoOpComponentContext);
    let mut consumer = HttpStaticConsumer::new(config, rt);

    let handle = tokio::spawn(async move {
        let _ = consumer.start(ctx).await;
    });

    (token, handle)
}

/// Wait for the server on `port` to become reachable.
async fn wait_for_server(port: u16, max_retries: usize) {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/");
    for _ in 0..max_retries {
        if client.get(&url).send().await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server on port {port} did not become ready after {max_retries} retries");
}

/// Write a file into a directory, returning the full path.
fn write_file(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).expect("write file");
    path
}

/// Gzip-compress `content` and write to `dir/name.gz`.
fn write_gz_file(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    let path = dir.join(format!("{name}.gz"));
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(content.as_bytes()).expect("gz write");
    let compressed = encoder.finish().expect("gz finish");
    std::fs::write(&path, &compressed).expect("write gz file");
    path
}

// ---------------------------------------------------------------------------
// Test 1: Shared-port API + static on same port
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_shared_port_api_and_static() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "hello.txt", "hello from static");

    let port = free_port();

    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let registry = ServerRegistry::global()
        .get_or_spawn(
            "127.0.0.1",
            port,
            2 * 1024 * 1024,
            10 * 1024 * 1024,
            1024,
            Arc::new(camel_component_api::test_support::NoopRuntimeObservability),
            "test-route".to_string(),
            None,
        )
        .await
        .expect("get registry");

    let (api_tx, mut api_rx) = mpsc::channel::<RequestEnvelope>(16);
    tokio::spawn(async move {
        while let Some(envelope) = api_rx.recv().await {
            let reply = camel_component_http::HttpReply {
                status: 200,
                headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
                body: camel_component_http::HttpReplyBody::Bytes(bytes::Bytes::from(
                    "hello from api",
                )),
            };
            let _ = envelope.reply_tx.send(reply);
        }
    });

    registry
        .register_api_route("/api/hello".to_string(), api_tx)
        .await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/hello"))
        .send()
        .await
        .expect("api request");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("api body");
    assert_eq!(body, "hello from api");

    let resp = client
        .get(format!("http://127.0.0.1:{port}/hello.txt"))
        .send()
        .await
        .expect("static request");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("static body");
    assert_eq!(body, "hello from static");
}

// ---------------------------------------------------------------------------
// Test 2: Basic static file serving
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_basic_file_serving() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "index.html", "<h1>Hello</h1>");
    write_file(dir.path(), "style.css", "body { color: red; }");

    let port = free_port();
    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/index.html"))
        .send()
        .await
        .expect("index request");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "<h1>Hello</h1>");

    let resp = client
        .get(format!("http://127.0.0.1:{port}/style.css"))
        .send()
        .await
        .expect("css request");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "body { color: red; }");
}

// ---------------------------------------------------------------------------
// Test 3: Precompressed gzip with Content-Encoding assertion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_precompressed_gzip_content_encoding() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    let js_content = "console.log('hello world');";
    write_file(dir.path(), "app.js", js_content);
    write_gz_file(dir.path(), "app.js", js_content);

    let port = free_port();
    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    // Build client with auto-decompress disabled so we can see Content-Encoding
    let client = reqwest::ClientBuilder::new()
        .no_gzip()
        .build()
        .expect("build client");

    let resp = client
        .get(format!("http://127.0.0.1:{port}/app.js"))
        .header("Accept-Encoding", "gzip")
        .send()
        .await
        .expect("gzip request");

    assert_eq!(resp.status(), 200);

    // Verify Content-Encoding header is present
    let content_encoding = resp
        .headers()
        .get("content-encoding")
        .expect("Content-Encoding header missing")
        .to_str()
        .expect("Content-Encoding not valid UTF-8");
    assert!(
        content_encoding.contains("gzip"),
        "Expected Content-Encoding to contain 'gzip', got: {content_encoding}"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Cache-Control header from config
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_cache_control_from_config() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir1 = tempfile::TempDir::new().expect("tempdir");
    write_file(dir1.path(), "file.txt", "from dir1");

    let dir2 = tempfile::TempDir::new().expect("tempdir");
    write_file(dir2.path(), "file.txt", "from dir2");

    let port1 = free_port();
    let port2 = free_port();

    let config1 = HttpStaticConfig {
        dir: dir1.path().to_path_buf(),
        port: port1,
        host: "127.0.0.1".to_string(),
        cache_control: "public, max-age=0".to_string(),
        ..HttpStaticConfig::default()
    };

    let config2 = HttpStaticConfig {
        dir: dir2.path().to_path_buf(),
        port: port2,
        host: "127.0.0.1".to_string(),
        cache_control: "public, max-age=31536000, immutable".to_string(),
        ..HttpStaticConfig::default()
    };

    let (_token1, _handle1) = start_static_consumer(config1).await;
    let (_token2, _handle2) = start_static_consumer(config2).await;
    wait_for_server(port1, 40).await;
    wait_for_server(port2, 40).await;

    let client = reqwest::Client::new();

    let resp1 = client
        .get(format!("http://127.0.0.1:{port1}/file.txt"))
        .send()
        .await
        .expect("dir1 request");
    assert_eq!(resp1.status(), 200);
    let cc1 = resp1
        .headers()
        .get("cache-control")
        .expect("Cache-Control missing for dir1")
        .to_str()
        .expect("Cache-Control not valid UTF-8");
    assert_eq!(cc1, "public, max-age=0");

    let resp2 = client
        .get(format!("http://127.0.0.1:{port2}/file.txt"))
        .send()
        .await
        .expect("dir2 request");
    assert_eq!(resp2.status(), 200);
    let cc2 = resp2
        .headers()
        .get("cache-control")
        .expect("Cache-Control missing for dir2")
        .to_str()
        .expect("Cache-Control not valid UTF-8");
    assert_eq!(cc2, "public, max-age=31536000, immutable");
}

// ---------------------------------------------------------------------------
// Test 5: SPA fallback serves index for extensionless paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_spa_fallback_serves_index() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "index.html", "<div id=\"app\">SPA</div>");

    let port = free_port();
    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        spa_fallback: true,
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/some/route"))
        .header("Accept", "text/html")
        .send()
        .await
        .expect("spa fallback request");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("spa body");
    assert!(
        body.contains("SPA"),
        "Expected SPA index.html content, got: {body}"
    );
}

// ---------------------------------------------------------------------------
// Test 6: SPA wins over custom error page (status 404, body = index.html)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_spa_wins_over_custom_error_page() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(
        dir.path(),
        "index.html",
        "<div id=\"app\">SPA Content</div>",
    );
    write_file(dir.path(), "custom404.html", "<h1>Custom 404 Page</h1>");

    let port = free_port();

    let mut error_pages = HashMap::new();
    error_pages.insert(404u16, dir.path().join("custom404.html"));

    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        spa_fallback: true,
        error_pages,
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/nonexistent/path"))
        .header("Accept", "text/html")
        .send()
        .await
        .expect("spa precedence request");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("precedence body");
    assert!(
        body.contains("SPA Content"),
        "Expected SPA index.html content (SPA wins), got: {body}"
    );
    assert!(
        !body.contains("Custom 404"),
        "Should NOT have received custom 404 page"
    );
}

// ---------------------------------------------------------------------------
// Test 7: Subdirectory serving
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_subdirectory_serving() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("assets/css")).expect("create subdirs");
    write_file(dir.path(), "index.html", "<h1>Root</h1>");
    write_file(&dir.path().join("assets/css"), "main.css", "body {}");

    let port = free_port();
    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/assets/css/main.css"))
        .send()
        .await
        .expect("subdir request");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "body {}");
}

// ---------------------------------------------------------------------------
// Test 8: 404 for missing files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_404_for_missing_file() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "exists.txt", "I exist");

    let port = free_port();
    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/does-not-exist.txt"))
        .send()
        .await
        .expect("404 request");
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Test 9: Mount path prefix — /assets/style.css serves, /style.css returns 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_mount_path_prefix_serves_files() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "style.css", "body { color: blue; }");

    let port = free_port();
    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        mount_path: "/assets".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/assets/style.css"))
        .send()
        .await
        .expect("mount prefix request");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "body { color: blue; }");

    let resp = client
        .get(format!("http://127.0.0.1:{port}/style.css"))
        .send()
        .await
        .expect("root request");
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Test 10: Multiple mounts on same port (isolated)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_multiple_mounts_same_port() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir_assets = tempfile::TempDir::new().expect("tempdir assets");
    let dir_public = tempfile::TempDir::new().expect("tempdir public");
    write_file(dir_assets.path(), "app.js", "console.log('app')");
    write_file(dir_public.path(), "robots.txt", "User-agent: *");

    let port = free_port();

    let config_assets = HttpStaticConfig {
        dir: dir_assets.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        mount_path: "/assets".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token1, _handle1) = start_static_consumer(config_assets).await;

    let config_public = HttpStaticConfig {
        dir: dir_public.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        mount_path: "/public".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token2, _handle2) = start_static_consumer(config_public).await;

    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/assets/app.js"))
        .send()
        .await
        .expect("assets request");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "console.log('app')");

    let resp = client
        .get(format!("http://127.0.0.1:{port}/public/robots.txt"))
        .send()
        .await
        .expect("public request");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "User-agent: *");

    let resp = client
        .get(format!("http://127.0.0.1:{port}/other/file.txt"))
        .send()
        .await
        .expect("other request");
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Test 11: Duplicate mount path rejection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_duplicate_mount_path_rejected() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir1 = tempfile::TempDir::new().expect("tempdir1");
    let dir2 = tempfile::TempDir::new().expect("tempdir2");
    write_file(dir1.path(), "a.txt", "a");
    write_file(dir2.path(), "b.txt", "b");

    let port = free_port();

    let config1 = HttpStaticConfig {
        dir: dir1.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        mount_path: "/shared".to_string(),
        ..HttpStaticConfig::default()
    };
    let (token1, _handle1) = start_static_consumer(config1).await;
    wait_for_server(port, 40).await;

    let registry = ServerRegistry::global()
        .get_or_spawn(
            "127.0.0.1",
            port,
            2 * 1024 * 1024,
            10 * 1024 * 1024,
            1024,
            Arc::new(camel_component_api::test_support::NoopRuntimeObservability),
            "test-route".to_string(),
            None,
        )
        .await
        .expect("get registry");

    let canonical_dir2 = std::fs::canonicalize(dir2.path()).unwrap();
    let serve_dir2 = ServeDir::new(&canonical_dir2)
        .precompressed_gzip()
        .precompressed_br()
        .append_index_html_on_directories(true);

    let mount2 = StaticMount {
        mount_path: "/shared".to_string(),
        mode: MountMode::Static,
        dir: canonical_dir2,
        cache_control: "public, max-age=0".to_string(),
        error_pages: std::collections::HashMap::new(),
        serve_dir: serve_dir2,
    };

    let result = registry.register_static_mount(mount2).await;
    assert!(
        result.is_err(),
        "Expected duplicate mount_path to be rejected"
    );
    if let Err(camel_component_api::CamelError::Config(msg)) = &result {
        assert!(msg.contains("duplicate static mount path"));
    }

    token1.cancel();
}

// ---------------------------------------------------------------------------
// Test 12: Longest prefix wins (/assets/sub over /assets)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_longest_prefix_wins() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir_assets = tempfile::TempDir::new().expect("tempdir assets");
    let dir_assets_sub = tempfile::TempDir::new().expect("tempdir assets/sub");
    write_file(dir_assets.path(), "file.txt", "from assets");
    write_file(dir_assets_sub.path(), "file.txt", "from assets/sub");

    let port = free_port();

    let config_assets = HttpStaticConfig {
        dir: dir_assets.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        mount_path: "/assets".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token1, _handle1) = start_static_consumer(config_assets).await;

    let config_sub = HttpStaticConfig {
        dir: dir_assets_sub.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        mount_path: "/assets/sub".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token2, _handle2) = start_static_consumer(config_sub).await;

    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/assets/sub/file.txt"))
        .send()
        .await
        .expect("sub request");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "from assets/sub");

    let resp = client
        .get(format!("http://127.0.0.1:{port}/assets/file.txt"))
        .send()
        .await
        .expect("assets request");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "from assets");
}

// ---------------------------------------------------------------------------
// Test 13: SPA fallback scoped to mount prefix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_spa_fallback_scoped_to_mount_prefix() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "index.html", "<div id=\"app\">Scoped SPA</div>");

    let port = free_port();
    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        mount_path: "/app".to_string(),
        spa_fallback: true,
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/app/some/route"))
        .header("Accept", "text/html")
        .send()
        .await
        .expect("spa scoped request");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("spa scoped body");
    assert!(body.contains("Scoped SPA"));

    let resp = client
        .get(format!("http://127.0.0.1:{port}/other/route"))
        .header("Accept", "text/html")
        .send()
        .await
        .expect("outside mount request");
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Test 14: Segment boundary: /asset does NOT match /assets/file.txt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_segment_boundary_prefix_match() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "file.txt", "from asset dir");

    let port = free_port();
    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        mount_path: "/asset".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/asset/file.txt"))
        .send()
        .await
        .expect("exact prefix request");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "from asset dir");

    let resp = client
        .get(format!("http://127.0.0.1:{port}/assets/file.txt"))
        .send()
        .await
        .expect("similar prefix request");
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Test 15: SPA + non-SPA same mount path rejected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_spa_and_non_spa_same_mount_path_rejected() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir1 = tempfile::TempDir::new().expect("tempdir1");
    let dir2 = tempfile::TempDir::new().expect("tempdir2");
    write_file(dir1.path(), "a.txt", "a");
    write_file(dir2.path(), "index.html", "<div>SPA</div>");

    let port = free_port();

    let config1 = HttpStaticConfig {
        dir: dir1.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        mount_path: "/shared".to_string(),
        ..HttpStaticConfig::default()
    };
    let (token1, _handle1) = start_static_consumer(config1).await;
    wait_for_server(port, 40).await;

    let registry = ServerRegistry::global()
        .get_or_spawn(
            "127.0.0.1",
            port,
            2 * 1024 * 1024,
            10 * 1024 * 1024,
            1024,
            Arc::new(camel_component_api::test_support::NoopRuntimeObservability),
            "test-route".to_string(),
            None,
        )
        .await
        .expect("get registry");

    let canonical_dir2 = std::fs::canonicalize(dir2.path()).unwrap();
    let serve_dir2 = ServeDir::new(&canonical_dir2)
        .precompressed_gzip()
        .precompressed_br()
        .append_index_html_on_directories(true);

    let spa_mount = StaticMount {
        mount_path: "/shared".to_string(),
        mode: MountMode::Spa,
        dir: canonical_dir2,
        cache_control: "public, max-age=0".to_string(),
        error_pages: std::collections::HashMap::new(),
        serve_dir: serve_dir2,
    };

    let result = registry.register_static_mount(spa_mount).await;
    assert!(
        result.is_err(),
        "Expected SPA mount with same path as non-SPA to be rejected"
    );
    if let Err(camel_component_api::CamelError::Config(msg)) = &result {
        assert!(msg.contains("duplicate static mount path"));
    }

    token1.cancel();
}

// ---------------------------------------------------------------------------
// Test 16: Consumer lifecycle — start registers mount, stop unregisters
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_consumer_lifecycle_start_stop() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "test.txt", "lifecycle test");

    let port = free_port();
    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        ..HttpStaticConfig::default()
    };

    let (token, handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    // Verify file is served
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/test.txt"))
        .send()
        .await
        .expect("request while running");
    assert_eq!(resp.status(), 200);

    // Stop the consumer
    token.cancel();
    let _ = handle.await;

    // After stop, the mount is unregistered — request should get 404
    let resp = client
        .get(format!("http://127.0.0.1:{port}/test.txt"))
        .send()
        .await
        .expect("request after stop");
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Test 17: Config resolution — TOML defaults + URI override
// ---------------------------------------------------------------------------

#[test]
fn http_static_config_toml_defaults_and_uri_override() {
    use camel_component_api::UriConfig;

    // URI defaults
    let config = HttpStaticConfig::from_uri("http-static:/var/www").unwrap();
    assert_eq!(config.dir, PathBuf::from("/var/www"));
    assert_eq!(config.port, 8080);
    assert_eq!(config.host, "0.0.0.0");
    assert!(!config.spa_fallback);
    assert_eq!(config.cache_control, "public, max-age=0");
    assert_eq!(config.mount_path, "/");

    // URI override
    let config = HttpStaticConfig::from_uri(
        "http-static:/app/dist?port=9090&host=127.0.0.1&spaFallback=true&cacheControl=no-cache",
    )
    .unwrap();
    assert_eq!(config.dir, PathBuf::from("/app/dist"));
    assert_eq!(config.port, 9090);
    assert_eq!(config.host, "127.0.0.1");
    assert!(config.spa_fallback);
    assert_eq!(config.cache_control, "no-cache");
}

#[test]
fn http_static_config_from_uri_with_defaults() {
    let toml_defaults = HttpStaticConfig {
        dir: PathBuf::from("/default/dir"),
        port: 8080,
        host: "0.0.0.0".to_string(),
        spa_fallback: false,
        cache_control: "public, max-age=0".to_string(),
        error_pages: HashMap::new(),
        ..HttpStaticConfig::default()
    };

    let config = HttpStaticConfig::from_uri_with_defaults(
        "http-static:/override/dir?port=3000&spaFallback=true",
        &toml_defaults,
    )
    .unwrap();

    assert_eq!(config.dir, PathBuf::from("/override/dir"));
    assert_eq!(config.port, 3000);
    assert!(config.spa_fallback);
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.cache_control, "public, max-age=0");
}

// ---------------------------------------------------------------------------
// Test 18: Bundle registration — http-static scheme registered
// ---------------------------------------------------------------------------

#[test]
fn http_static_bundle_registers_scheme() {
    struct TestRegistrar {
        schemes: Vec<String>,
    }
    impl camel_component_api::ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(
            &mut self,
            component: std::sync::Arc<dyn camel_component_api::Component>,
        ) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    let bundle = HttpStaticBundle::from_toml(toml::Value::Table(toml::map::Map::new())).unwrap();
    let mut registrar = TestRegistrar { schemes: vec![] };
    bundle.register_all(&mut registrar);
    assert_eq!(registrar.schemes, vec!["http-static"]);
}

#[test]
fn http_static_bundle_from_toml() {
    let toml_str = r#"
        dir = "/app/spa"
        port = 3000
        host = "127.0.0.1"
        spaFallback = true
        cacheControl = "no-cache"
    "#;
    let config: HttpStaticConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.dir, PathBuf::from("/app/spa"));
    assert_eq!(config.port, 3000);
    assert_eq!(config.host, "127.0.0.1");
    assert!(config.spa_fallback);
    assert_eq!(config.cache_control, "no-cache");
}

// ---------------------------------------------------------------------------
// Test 19: Error page returns original status code (404, not 200)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_error_page_returns_original_status() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "custom404.html", "<h1>Not Found</h1>");

    let port = free_port();

    let mut error_pages = HashMap::new();
    error_pages.insert(404u16, dir.path().join("custom404.html"));

    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        error_pages,
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://127.0.0.1:{port}/nonexistent.html"))
        .send()
        .await
        .expect("error page request");
    // Error page should return 404 status, not 200
    assert_eq!(resp.status(), 404);
    let body = resp.text().await.expect("error page body");
    assert!(body.contains("Not Found"));
}

// ---------------------------------------------------------------------------
// Test 20: Path traversal ../ rejected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_static_path_traversal_rejected() {
    install_crypto_provider();
    let _guard = TEST_MUTEX.lock().await;

    let dir = tempfile::TempDir::new().expect("tempdir");
    write_file(dir.path(), "safe.txt", "safe content");

    let port = free_port();
    let config = HttpStaticConfig {
        dir: dir.path().to_path_buf(),
        port,
        host: "127.0.0.1".to_string(),
        ..HttpStaticConfig::default()
    };
    let (_token, _handle) = start_static_consumer(config).await;
    wait_for_server(port, 40).await;

    let client = reqwest::Client::new();

    // Path traversal attempt should be rejected
    let resp = client
        .get(format!("http://127.0.0.1:{port}/../etc/passwd"))
        .send()
        .await
        .expect("traversal request");
    assert_eq!(resp.status(), 404);

    // Double encoding traversal
    let resp = client
        .get(format!("http://127.0.0.1:{port}/foo/../../etc/passwd"))
        .send()
        .await
        .expect("traversal request 2");
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Test 21: HttpStaticComponent scheme and endpoint creation
// ---------------------------------------------------------------------------

#[test]
fn http_static_component_scheme() {
    let component = HttpStaticComponent::new();
    assert_eq!(component.scheme(), "http-static");
}

#[test]
fn http_static_endpoint_creates_consumer() {
    use camel_component_api::{Component, NoOpComponentContext};

    let component = HttpStaticComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint("http-static:/tmp?port=19900", &ctx)
        .unwrap();
    assert!(
        endpoint
            .create_consumer(Arc::new(NoOpComponentContext))
            .is_ok()
    );
}

#[test]
fn http_static_endpoint_producer_not_supported() {
    use camel_component_api::{Component, NoOpComponentContext, ProducerContext};

    let component = HttpStaticComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint("http-static:/tmp?port=19900", &ctx)
        .unwrap();
    let producer_ctx = ProducerContext::new();
    let result = endpoint.create_producer(support::test_rt(), &producer_ctx);
    assert!(result.is_err());
    if let Err(camel_component_api::CamelError::Config(msg)) = result {
        assert!(msg.contains("does not support producers"));
    }
}

// ---------------------------------------------------------------------------
// Test 22: Mount path from URI (new style with dir param)
// ---------------------------------------------------------------------------

#[test]
fn http_static_mount_path_from_uri_with_dir_param() {
    use camel_component_api::UriConfig;

    let config = HttpStaticConfig::from_uri("http-static:/assets?dir=/var/www").unwrap();
    assert_eq!(config.dir, PathBuf::from("/var/www"));
    assert_eq!(config.mount_path, "/assets");
}

#[test]
fn http_static_mount_path_normalized() {
    use camel_component_api::UriConfig;

    // Leading slash added
    let config = HttpStaticConfig::from_uri("http-static:assets?dir=/var/www").unwrap();
    assert_eq!(config.mount_path, "/assets");

    // Trailing slash removed
    let config = HttpStaticConfig::from_uri("http-static:/assets/?dir=/var/www").unwrap();
    assert_eq!(config.mount_path, "/assets");

    // Root stays root
    let config = HttpStaticConfig::from_uri("http-static:/?dir=/var/www").unwrap();
    assert_eq!(config.mount_path, "/");
}

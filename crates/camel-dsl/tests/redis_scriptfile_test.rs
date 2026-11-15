//! `scriptFile` path resolution for redis script endpoints (redisfc 2.2).
//!
//! A route document authors `scriptFile=scripts/lock.lua`. The DSL resolves
//! that path against the route file's directory before the endpoint is built,
//! so the component reads exactly the file the document names, independent of
//! the process working directory. A missing file fails the LOAD and names the
//! resolved path.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use camel_api::{Body, Exchange, Message, ProducerContext};
use camel_component_api::{Component, NoOpComponentContext};
use camel_component_redis::RedisComponent;
use camel_core::route::{BuilderStep, RouteDefinition};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The `to:` URI of the route's only endpoint step.
fn to_uri(route: &RouteDefinition) -> &str {
    route
        .steps()
        .iter()
        .find_map(|step| match step {
            BuilderStep::To(uri) => Some(uri.as_str()),
            _ => None,
        })
        .expect("the route has a to step")
}

/// Write `route.yaml` in `dir` (pointing at `stub`) plus `script_rel`'s
/// contents, load the route, then build the redis endpoint and execute the
/// script. Returns the resolved `to:` URI and the stub frames.
async fn load_route_and_execute(
    dir: &Path,
    script_rel: &str,
    query: &str,
    lua: &str,
) -> (String, Vec<String>) {
    let script = dir.join(script_rel);
    std::fs::create_dir_all(script.parent().expect("script path has a parent"))
        .expect("create script dir");
    std::fs::write(&script, lua).expect("write script");

    let stub = RespStub::start().await;
    let route_file = dir.join("route.yaml");
    std::fs::write(
        &route_file,
        format!(
            "routes:\n  - id: redis-scriptfile\n    from: \"direct:start\"\n    steps:\n      - to: \"{}?{query}\"\n",
            stub.url()
        ),
    )
    .expect("write route file");

    let routes = camel_dsl::load_from_file(&route_file).expect("route loads");
    let uri = to_uri(&routes[0]).to_string();

    let component = RedisComponent::new();
    let endpoint = component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("endpoint builds from the resolved scriptFile");
    let mut producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ProducerContext::new())
        .expect("producer builds");
    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({
        "keys": [],
        "args": [],
    }))));
    tower::ServiceExt::oneshot(&mut producer, exchange)
        .await
        .expect("the script executes");

    (uri, stub.frames())
}

/// The resolved `scriptFile` value must round-trip through the Camel URI
/// parser as ONE opaque value, and it must be the file that ran.
fn assert_script_ran(frames: &[String], lua: &str, label: &str) {
    let sha = redis::Script::new(lua).get_hash().to_string();
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("EVALSHA") && frame.contains(&sha)),
        "{label}: the executed script must be scripts/lock.lua's; expected SHA {sha} in: {frames:?}"
    );
}

#[tokio::test]
async fn scriptfile_resolves_against_route_file_dir() {
    let dir = tempfile::tempdir().expect("create temp dir");

    // No CWD dependence: the route directory is a temp dir, not the test CWD.
    assert_ne!(
        std::env::current_dir().expect("read cwd"),
        dir.path(),
        "the test must not run inside the route directory"
    );

    let lua = "return 'lock'";
    let (_uri, frames) = load_route_and_execute(
        dir.path(),
        "scripts/lock.lua",
        "command=eval&scriptFile=scripts/lock.lua",
        lua,
    )
    .await;
    assert_script_ran(&frames, lua, "plain temp dir");
}

/// Loads `route.yaml` in `dir` whose only endpoint uses `scheme`, and returns
/// the resolved `to:` URI.
///
/// No endpoint is built and no server is contacted: the sentinel schemes need
/// a live Sentinel to run, but the DSL resolves `scriptFile` at route load,
/// which is the behavior under test.
fn load_route_resolving_script(dir: &Path, scheme: &str, query: &str) -> String {
    let script = dir.join("scripts/lock.lua");
    std::fs::create_dir_all(script.parent().expect("script path has a parent"))
        .expect("create script dir");
    std::fs::write(&script, "return 'lock'").expect("write script");

    let route_file = dir.join("route.yaml");
    std::fs::write(
        &route_file,
        format!(
            "routes:\n  - id: redis-sentinel-scriptfile\n    from: \"direct:start\"\n    steps:\n      - to: \"{scheme}://127.0.0.1:26379?{query}\"\n"
        ),
    )
    .expect("write route file");

    let routes = camel_dsl::load_from_file(&route_file).expect("route loads");
    to_uri(&routes[0]).to_string()
}

/// Asserts the resolved `scriptFile` value is anchored at `dir`.
fn assert_script_path_anchored(uri: &str, dir: &Path, label: &str) {
    let parsed = camel_endpoint::parse_uri(uri).expect("the rewritten URI parses");
    assert_eq!(
        parsed.params.get("scriptFile").map(String::as_str),
        Some(dir.join("scripts/lock.lua").to_string_lossy().as_ref()),
        "{label}: scriptFile must resolve against the route file directory, not the CWD: {uri}"
    );
}

/// A sentinel route's relative `scriptFile` must be anchored at the route
/// file's directory too.
///
/// `RedisEndpointConfig::from_uri` parses script parameters for
/// `redis-sentinel://`, so an unanchored value would reach the component
/// unchanged and resolve against the process CWD.
#[test]
fn scriptfile_resolves_against_route_file_dir_sentinel() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let uri = load_route_resolving_script(
        dir.path(),
        "redis-sentinel",
        "command=eval&scriptFile=scripts/lock.lua",
    );
    assert_script_path_anchored(&uri, dir.path(), "redis-sentinel");
}

/// TLS sentinel twin: `rediss-sentinel://` takes the same script parameters,
/// so it must anchor the path identically.
#[test]
fn scriptfile_resolves_against_route_file_dir_sentinel_tls() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let uri = load_route_resolving_script(
        dir.path(),
        "rediss-sentinel",
        "command=eval&scriptFile=scripts/lock.lua",
    );
    assert_script_path_anchored(&uri, dir.path(), "rediss-sentinel");
}

/// A non-ASCII route directory must not reach the byte-indexed query scanner
/// unescaped: `parse_uri` panics on a slice that ends inside a multi-byte
/// UTF-8 sequence.
#[tokio::test]
async fn scriptfile_unicode_dir_round_trips() {
    let root = tempfile::tempdir().expect("create temp dir");
    let dir = root.path().join("café");
    std::fs::create_dir_all(&dir).expect("create unicode dir");

    let lua = "return 'lock'";
    let script_rel = "scripts/lock.lua";
    let (uri, frames) = load_route_and_execute(
        &dir,
        script_rel,
        "command=eval&scriptFile=scripts/lock.lua",
        lua,
    )
    .await;

    assert!(
        uri.contains("%C3%A9"),
        "the non-ASCII directory must be percent-encoded, got: {uri}"
    );
    let parsed = camel_endpoint::parse_uri(&uri).expect("the rewritten URI parses");
    assert_eq!(
        parsed.params.get("scriptFile").map(String::as_str),
        Some(dir.join(script_rel).to_string_lossy().as_ref()),
        "scriptFile must decode back to the resolved path"
    );
    assert_script_ran(&frames, lua, "unicode dir");
}

/// A directory containing `RAW(`/`RAW(x)` must not activate Camel's `RAW(...)`
/// query syntax: the scanner would otherwise swallow the pairs that follow
/// `scriptFile` (an unbalanced `(` swallows the rest of the query, so the
/// trailing parameter never parses).
#[tokio::test]
async fn scriptfile_raw_paren_dir_round_trips() {
    let root = tempfile::tempdir().expect("create temp dir");
    // `RAW(x)` is the required literal directory; the nested unmatched `RAW(`
    // is the swallowing trigger, and it can only come from the RESOLVED
    // absolute path (an authored relative value is parsed before resolution).
    let dir = root.path().join("RAW(x)").join("RAW(");
    std::fs::create_dir_all(&dir).expect("create RAW(x)/RAW( dir");

    let lua = "return 'lock'";
    let script_rel = "scripts/lock.lua";
    let (uri, frames) = load_route_and_execute(
        &dir,
        script_rel,
        "command=eval&scriptFile=scripts/lock.lua&idempotent=true",
        lua,
    )
    .await;

    assert!(
        !uri.contains('(') && !uri.contains(')'),
        "no parenthesis may reach the rewritten query unescaped: {uri}"
    );

    let parsed = camel_endpoint::parse_uri(&uri).expect("the rewritten URI parses");
    assert_eq!(
        parsed.params.get("idempotent").map(String::as_str),
        Some("true"),
        "the trailing parameter must not be swallowed into the filename: {uri}"
    );
    assert_eq!(
        parsed.params.get("scriptFile").map(String::as_str),
        Some(dir.join(script_rel).to_string_lossy().as_ref()),
        "scriptFile must decode back to the resolved path (no RAW() wrapper): {uri}"
    );
    assert!(
        !parsed.params.contains_key("RAW"),
        "the path must not activate RAW(...) syntax: {uri}"
    );
    assert_script_ran(&frames, lua, "RAW(x) dir");
}

/// The rewrite replaces the `scriptFile` value span ONLY: authored empty
/// segments, odd separators, and a trailing `&` survive byte-for-byte.
#[test]
fn scriptfile_rewrite_preserves_query_bytes() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let scripts = dir.path().join("scripts");
    std::fs::create_dir_all(&scripts).expect("create scripts dir");
    std::fs::write(scripts.join("lock.lua"), "return 'lock'").expect("write lock.lua");

    let authored = "command=eval&&scriptFile=scripts/lock.lua&&flag=1&";
    let route_file = dir.path().join("route.yaml");
    std::fs::write(
        &route_file,
        format!(
            "routes:\n  - id: redis-scriptfile\n    from: \"direct:start\"\n    steps:\n      - to: \"redis://127.0.0.1:6379?{authored}\"\n"
        ),
    )
    .expect("write route file");

    let routes = camel_dsl::load_from_file(&route_file).expect("route loads");
    let uri = to_uri(&routes[0]).to_string();

    let query = uri
        .split_once('?')
        .map(|(_, query)| query)
        .expect("the rewritten URI keeps its query");
    let prefix = "command=eval&&scriptFile=";
    let suffix = "&&flag=1&";
    assert!(
        query.starts_with(prefix) && query.ends_with(suffix),
        "every authored separator must survive verbatim, got query: {query}"
    );
    let value = &query[prefix.len()..query.len() - suffix.len()];
    assert_eq!(
        value,
        dir.path()
            .join("scripts/lock.lua")
            .to_string_lossy()
            .as_ref(),
        "the value span carries exactly the resolved path"
    );
    let parsed = camel_endpoint::parse_uri(&uri).expect("the rewritten URI parses");
    assert_eq!(
        parsed.params.get("flag").map(String::as_str),
        Some("1"),
        "no authored pair may be lost or renamed"
    );
}

#[test]
fn scriptfile_missing_file_names_resolved_path() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let route_file = dir.path().join("route.yaml");
    std::fs::write(
        &route_file,
        "routes:\n  - id: redis-missing\n    from: \"direct:start\"\n    steps:\n      - to: \"redis://127.0.0.1:6379?command=eval&scriptFile=missing.lua\"\n",
    )
    .expect("write route file");

    let err = match camel_dsl::load_from_file(&route_file) {
        Err(err) => err.to_string(),
        Ok(_) => panic!("a missing scriptFile must fail the route load"),
    };
    let resolved = dir.path().join("missing.lua");
    assert!(
        err.contains(&resolved.display().to_string()),
        "the error must name the resolved path {}, got: {err}",
        resolved.display()
    );
}

/// Loopback RESP stub: answers the driver handshake, queues every command
/// inside `MULTI`, and replies to `EXEC` with the cached provenance pair
/// `(SCRIPT EXISTS <sha>, EVALSHA <sha>)`. Application frames are recorded
/// for the SHA assertion.
struct RespStub {
    addr: SocketAddr,
    frames: Arc<Mutex<Vec<Vec<u8>>>>,
    accept: tokio::task::JoinHandle<()>,
}

impl RespStub {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback stub listener");
        let addr = listener.local_addr().expect("stub listener local addr");
        let frames = Arc::new(Mutex::new(Vec::new()));
        let accept_frames = Arc::clone(&frames);
        let accept = tokio::spawn(async move {
            loop {
                // allow-test-wait: spawned test-server accept loop; teardown-bounded by the test runtime
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(serve(stream, Arc::clone(&accept_frames)));
            }
        });
        Self {
            addr,
            frames,
            accept,
        }
    }

    fn url(&self) -> String {
        format!("redis://{}", self.addr)
    }

    fn frames(&self) -> Vec<String> {
        self.frames
            .lock()
            .expect("stub frame registry poisoned")
            .iter()
            .map(|frame| String::from_utf8_lossy(frame).into_owned())
            .collect()
    }
}

impl Drop for RespStub {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// Driver handshake verbs; answered `+OK` and never recorded.
const HANDSHAKE_VERBS: [&str; 4] = ["CLIENT", "AUTH", "SELECT", "HELLO"];

/// The cached provenance pair: `SCRIPT EXISTS <sha>` answers `[1]` and the
/// `EVALSHA` result is the integer `1`.
const PAIR_CACHED: &[u8] = b"*2\r\n*1\r\n:1\r\n:1\r\n";

async fn serve(mut stream: TcpStream, frames: Arc<Mutex<Vec<Vec<u8>>>>) {
    let mut buf: Vec<u8> = Vec::with_capacity(128);
    let mut chunk = [0u8; 512];
    let mut in_multi = false;
    loop {
        let read = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..read]);
        while let Some((frame_len, verb)) = parse_frame(&buf) {
            let frame: Vec<u8> = buf.drain(..frame_len).collect();
            let reply = if HANDSHAKE_VERBS.contains(&verb.as_str()) {
                b"+OK\r\n".to_vec()
            } else {
                frames
                    .lock()
                    .expect("stub frame registry poisoned")
                    .push(frame);
                match verb.as_str() {
                    "MULTI" => {
                        in_multi = true;
                        b"+OK\r\n".to_vec()
                    }
                    "DISCARD" => {
                        in_multi = false;
                        b"+OK\r\n".to_vec()
                    }
                    "EXEC" => {
                        in_multi = false;
                        PAIR_CACHED.to_vec()
                    }
                    _ if in_multi => b"+QUEUED\r\n".to_vec(),
                    _ => b"+OK\r\n".to_vec(),
                }
            };
            if stream.write_all(&reply).await.is_err() {
                return;
            }
        }
    }
}

/// Parse `<marker><digits>\r\n` at `offset`: `(value, offset past the line)`.
fn parse_line_number(buf: &[u8], offset: usize, marker: u8) -> Option<(usize, usize)> {
    if buf.get(offset) != Some(&marker) {
        return None;
    }
    let line_start = offset + 1;
    let mut cursor = line_start;
    while let Some(&byte) = buf.get(cursor) {
        if byte == b'\r' {
            if buf.get(cursor + 1) != Some(&b'\n') {
                return None;
            }
            let digits = std::str::from_utf8(&buf[line_start..cursor]).ok()?;
            return Some((digits.parse().ok()?, cursor + 2));
        }
        if !byte.is_ascii_digit() {
            return None;
        }
        cursor += 1;
    }
    None
}

/// Parse one complete RESP array-of-bulk-strings frame: `(total_len, verb)`.
fn parse_frame(buf: &[u8]) -> Option<(usize, String)> {
    let (elements, mut cursor) = parse_line_number(buf, 0, b'*')?;
    let mut verb = String::new();
    for index in 0..elements {
        let (len, payload_at) = parse_line_number(buf, cursor, b'$')?;
        let end = payload_at.checked_add(len)?.checked_add(2)?;
        if buf.len() < end {
            return None;
        }
        if index == 0 {
            verb = String::from_utf8_lossy(&buf[payload_at..payload_at + len]).to_ascii_uppercase();
        }
        cursor = end;
    }
    Some((cursor, verb))
}

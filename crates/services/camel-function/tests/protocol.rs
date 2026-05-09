#![cfg(feature = "runner-tests")]

use camel_function::protocol::*;
use std::process::Stdio;
use std::time::Duration;

struct DenoRunner {
    port: u16,
    child: std::process::Child,
    client: reqwest::Client,
}

impl DenoRunner {
    async fn spawn() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let runner_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("runner")
            .join("main.ts");

        let child = std::process::Command::new("deno")
            .args([
                "run",
                "--allow-net=0.0.0.0",
                "--allow-env=PORT",
                runner_path.to_str().unwrap(),
            ])
            .env("PORT", port.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn deno");

        let runner = Self {
            port,
            child,
            client: reqwest::Client::new(),
        };

        runner.wait_for_health(Duration::from_secs(10)).await;
        runner
    }

    async fn wait_for_health(&self, timeout: Duration) {
        let start = std::time::Instant::now();
        loop {
            if let Ok(resp) = self.client.get(self.url("/health")).send().await {
                if resp.status().is_success() {
                    return;
                }
            }
            if start.elapsed() > timeout {
                panic!("runner did not become healthy within {:?}", timeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    async fn register(&self, req: &RegisterRequest) -> reqwest::Response {
        self.client
            .post(self.url("/register"))
            .json(req)
            .send()
            .await
            .expect("register request")
    }

    async fn invoke_raw(&self, wire: &ExchangeWire) -> reqwest::Response {
        self.client
            .post(self.url("/invoke"))
            .json(wire)
            .send()
            .await
            .expect("invoke request")
    }

    async fn invoke(&self, wire: &ExchangeWire) -> InvokeResponse {
        self.invoke_raw(wire)
            .await
            .json()
            .await
            .expect("decode invoke response")
    }

    async fn health(&self) -> HealthResponse {
        self.client
            .get(self.url("/health"))
            .send()
            .await
            .expect("health request")
            .json()
            .await
            .expect("decode health response")
    }
}

impl Drop for DenoRunner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn make_register_request(id: &str, source: &str) -> RegisterRequest {
    RegisterRequest {
        function_id: id.to_string(),
        runtime: "typescript".to_string(),
        source: source.to_string(),
        timeout_ms: 5000,
    }
}

fn make_exchange(id: &str) -> ExchangeWire {
    ExchangeWire {
        function_id: id.to_string(),
        correlation_id: "test-corr-id".to_string(),
        body: BodyWire::Empty,
        headers: Default::default(),
        properties: Default::default(),
        timeout_ms: 5000,
    }
}

#[tokio::test]
async fn test_health_empty() {
    let runner = DenoRunner::spawn().await;
    let health = runner.health().await;
    assert_eq!(health.status, "ready");
    assert!(health.registered.is_empty());
}

#[tokio::test]
async fn test_register_happy() {
    let runner = DenoRunner::spawn().await;
    let resp = runner.register(&make_register_request("fn1", "export default (c) => {}")).await;
    assert_eq!(resp.status().as_u16(), 204);
    let health = runner.health().await;
    assert!(health.registered.contains(&"fn1".to_string()));
}

#[tokio::test]
async fn test_register_duplicate_same_source() {
    let runner = DenoRunner::spawn().await;
    let req = make_register_request("fn1", "export default (c) => {}");
    let resp1 = runner.register(&req).await;
    assert_eq!(resp1.status().as_u16(), 204);
    let resp2 = runner.register(&req).await;
    assert_eq!(resp2.status().as_u16(), 200);
}

#[tokio::test]
async fn test_register_duplicate_different_source() {
    let runner = DenoRunner::spawn().await;
    let req1 = make_register_request("fn1", "export default (c) => { /* v1 */ }");
    let req2 = make_register_request("fn1", "export default (c) => { /* v2 */ }");
    let resp1 = runner.register(&req1).await;
    assert_eq!(resp1.status().as_u16(), 204);
    let resp2 = runner.register(&req2).await;
    assert_eq!(resp2.status().as_u16(), 400);
    let body: ErrorResponse = resp2.json().await.expect("decode error");
    assert_eq!(body.kind, "duplicate");
}

#[tokio::test]
async fn test_invoke_text_body() {
    let runner = DenoRunner::spawn().await;
    runner
        .register(&make_register_request(
            "text_transform",
            "export default (c) => { c.setBody(c.body().toString().toUpperCase()); }",
        ))
        .await;
    let mut wire = make_exchange("text_transform");
    wire.body = BodyWire::Text("hello".to_string());
    let result = runner.invoke(&wire).await;
    assert!(result.ok);
    let patch = result.patch.unwrap();
    assert!(patch.body.is_some());
    let body = patch.body.unwrap();
    match body {
        BodyWire::Text(s) => assert_eq!(s, "HELLO"),
        other => panic!("expected Text, got {:?}", other),
    }
}

#[tokio::test]
async fn test_invoke_json_body() {
    let runner = DenoRunner::spawn().await;
    runner
        .register(&make_register_request(
            "json_transform",
            "export default (c) => { const obj = c.body(); obj.processed = true; c.setBody(obj); }",
        ))
        .await;
    let mut wire = make_exchange("json_transform");
    wire.body = BodyWire::Json(serde_json::json!({"input": "data"}));
    let result = runner.invoke(&wire).await;
    assert!(result.ok);
    let patch = result.patch.unwrap();
    assert!(patch.body.is_some());
    let body = patch.body.unwrap();
    match body {
        BodyWire::Json(v) => {
            assert_eq!(v["input"], "data");
            assert_eq!(v["processed"], true);
        }
        other => panic!("expected Json, got {:?}", other),
    }
}

#[tokio::test]
async fn test_invoke_headers() {
    let runner = DenoRunner::spawn().await;
    runner
        .register(&make_register_request(
            "header_fn",
            "export default (c) => { c.setHeader('X-Added', 'yes'); c.removeHeader('X-Remove-Me'); }",
        ))
        .await;
    let mut wire = make_exchange("header_fn");
    wire.headers.insert("X-Remove-Me".to_string(), serde_json::json!("bye"));
    let result = runner.invoke(&wire).await;
    assert!(result.ok);
    let patch = result.patch.unwrap();
    assert!(patch.headers_set.iter().any(|(k, v)| k == "X-Added" && v == "yes"));
    assert!(patch.headers_removed.contains(&"X-Remove-Me".to_string()));
}

#[tokio::test]
async fn test_invoke_properties() {
    let runner = DenoRunner::spawn().await;
    runner
        .register(&make_register_request(
            "prop_fn",
            "export default (c) => { c.setProperty('my_prop', 42); }",
        ))
        .await;
    let result = runner.invoke(&make_exchange("prop_fn")).await;
    assert!(result.ok);
    let patch = result.patch.unwrap();
    assert!(patch.properties_set.iter().any(|(k, v)| k == "my_prop" && v == 42));
}

#[tokio::test]
async fn test_invoke_user_error() {
    let runner = DenoRunner::spawn().await;
    runner
        .register(&make_register_request(
            "error_fn",
            "export default (c) => { throw new Error('boom'); }",
        ))
        .await;
    let result = runner.invoke(&make_exchange("error_fn")).await;
    assert!(!result.ok);
    let err = result.error.unwrap();
    assert_eq!(err.kind, "user_error");
    assert!(err.message.contains("boom"));
    assert!(err.stack.is_some());
}

#[tokio::test]
async fn test_invoke_timeout() {
    let runner = DenoRunner::spawn().await;
    let mut req = make_register_request(
        "to1",
        "export default () => { while(true) {} }",
    );
    req.timeout_ms = 300;
    runner.register(&req).await;

    let mut wire = make_exchange("to1");
    wire.timeout_ms = 300;
    let result = runner.invoke(&wire).await;
    assert!(!result.ok);
    let err = result.error.unwrap();
    assert_eq!(err.kind, "timeout");
}

#[tokio::test]
async fn test_invoke_not_registered() {
    let runner = DenoRunner::spawn().await;
    let result = runner.invoke(&make_exchange("nonexistent")).await;
    assert!(!result.ok);
    let err = result.error.unwrap();
    assert_eq!(err.kind, "not_registered");
}

#[tokio::test]
async fn test_shutdown() {
    let runner = DenoRunner::spawn().await;
    let resp = runner
        .client
        .post(runner.url("/shutdown"))
        .send()
        .await
        .expect("shutdown request");
    assert_eq!(resp.status().as_u16(), 204);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(runner.child.try_wait().expect("wait").is_some());
}

#[tokio::test]
async fn test_health_shows_registered() {
    let runner = DenoRunner::spawn().await;
    runner
        .register(&make_register_request(
            "h1",
            "export default (c) => {}",
        ))
        .await;
    let health = runner.health().await;
    assert!(health.registered.contains(&"h1".to_string()));
}

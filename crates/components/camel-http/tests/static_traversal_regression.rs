use std::fs;
use tempfile::TempDir;
use tower::ServiceExt;
use tower_http::services::ServeDir;

#[tokio::test]
async fn servedir_rejects_percent_encoded_traversal() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("ok.txt"), b"x").unwrap();
    let service = ServeDir::new(dir.path());
    for vector in &["/../", "/%2e%2e/", "/%2e%2e%2f"] {
        let req = http::Request::builder()
            .uri(*vector)
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = service.clone().oneshot(req).await.unwrap();
        assert_ne!(
            resp.status(),
            axum::http::StatusCode::OK,
            "traversal succeeded for {vector}"
        );
    }
}

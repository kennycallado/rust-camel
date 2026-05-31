use std::collections::HashMap;
use std::path::PathBuf;

use axum::body::Body as AxumBody;
use axum::extract::Request;
use axum::http::{Response, StatusCode};
use axum::response::IntoResponse;
use tower::ServiceExt as TowerServiceExt;
use tower_http::services::ServeDir;

use crate::AppState;
use crate::registry::MountMode;

/// Check that `path` starts with `prefix` at a segment boundary.
/// Mount at `/asset` must NOT match `/assets/file.txt`.
fn prefix_matches_segment(path: &str, prefix: &str) -> bool {
    if prefix == "/" {
        return true; // root matches everything
    }
    if !path.starts_with(prefix) {
        return false;
    }
    path.len() == prefix.len() || path.as_bytes()[prefix.len()] == b'/'
}

/// Attempt to serve a request from static mounts, SPA fallback, or error pages.
///
/// Mounts are tried in descending order by `mount_path` length (longest prefix
/// wins). For each matching mount, ServeDir is tried first; if that fails and
/// the mount is an SPA mount, the SPA fallback is attempted. Error pages are
/// scoped to the best-matching mount — not cross-contaminated from other mounts.
pub(crate) async fn dispatch_static(
    state: &AppState,
    req: Request,
    path: &str,
) -> axum::response::Response {
    // Reject path traversal attempts early (string-only, no FS call)
    let relative = path.trim_start_matches('/');
    let has_traversal = relative.split('/').any(|seg| seg == "..");
    if has_traversal {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    // Extract request parts once so we can rebuild for each mount attempt
    let (parts, _body) = req.into_parts();

    // Collect mount info while lock is held
    let mounts: Vec<_> = {
        let inner = state.registry.inner.read().await;
        inner
            .mounts
            .iter()
            .map(|m| {
                (
                    m.mount_path.clone(),
                    m.mode,
                    m.serve_dir.clone(),
                    m.cache_control.clone(),
                    m.error_pages.clone(),
                )
            })
            .collect()
    }; // lock released

    // Sort mounts by mount_path length DESC (longest prefix wins)
    let mut indexed: Vec<_> = mounts.into_iter().collect();
    indexed.sort_by_key(|a| std::cmp::Reverse(a.0.len()));

    // Track best-matching mount for error page scoping (Fix 3)
    let mut best_match: Option<(String, HashMap<u16, PathBuf>)> = None;

    // Try each mount — longest prefix first
    for (mp, mode, serve_dir, cache_control, error_pages) in &indexed {
        if !prefix_matches_segment(path, mp.as_str()) {
            continue;
        }

        // Record best match (only on first/longest match)
        if best_match.is_none() {
            best_match = Some((mp.clone(), error_pages.clone()));
        }

        // Strip the mount_path prefix and rebuild the request URI for ServeDir
        let stripped_path = if mp == "/" {
            relative.to_string()
        } else {
            let remainder = path.strip_prefix(mp.as_str()).unwrap_or("");
            remainder.trim_start_matches('/').to_string()
        };

        let req = rebuild_request_with_path(&parts, &stripped_path);
        let resp = serve_via_serve_dir(serve_dir.clone(), req, cache_control).await;
        if resp.status().is_success() {
            return resp;
        }

        // ServeDir returned non-2xx
        // For SPA mounts, try SPA fallback BEFORE error pages (SPA wins).
        // SPA fallback means all unknown routes are handled by the SPA's
        // index.html. Explicit error pages are only used when SPA fallback
        // is disabled (MountMode::Static), when the request is not
        // SPA-qualified (e.g., has a file extension), or when the SPA
        // fallback itself fails.
        if *mode == MountMode::Spa && resp.status() == StatusCode::NOT_FOUND {
            let spa_req = rebuild_request(&parts);
            if is_spa_qualified(&spa_req) {
                return serve_spa_index(serve_dir.clone(), spa_req, cache_control).await;
            }
        }

        // Try error page for this mount (for Static mode or non-qualified SPA requests)
        if let Some(error_resp) = try_serve_error_page(error_pages, resp.status().as_u16()).await {
            return error_resp;
        }
    }

    // No mount matched — use best-match error pages only (Fix 3)
    if let Some((_, error_pages)) = &best_match
        && let Some(error_resp) =
            try_serve_error_page(error_pages, StatusCode::NOT_FOUND.as_u16()).await
    {
        return error_resp;
    }

    // Optional: try root-mount ("/") error pages as last resort
    let root_error_pages: Option<HashMap<u16, PathBuf>> = {
        let inner = state.registry.inner.read().await;
        inner
            .mounts
            .iter()
            .find(|m| m.mount_path == "/")
            .map(|m| m.error_pages.clone())
    };
    if let Some(pages) = root_error_pages
        && let Some(error_resp) = try_serve_error_page(&pages, StatusCode::NOT_FOUND.as_u16()).await
    {
        return error_resp;
    }

    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

/// Rebuild a request from captured parts (Request is not Clone).
/// Uses empty body since static serving is always GET/HEAD.
fn rebuild_request(parts: &http::request::Parts) -> Request {
    let mut builder = http::Request::builder()
        .method(parts.method.clone())
        .uri(parts.uri.clone())
        .version(parts.version);
    for (k, v) in &parts.headers {
        builder = builder.header(k, v);
    }
    builder
        .extension(parts.extensions.clone())
        .body(AxumBody::empty())
        .expect("valid request rebuild") // allow-unwrap
}

/// Rebuild a request from captured parts, but with a rewritten URI path.
/// Used when stripping mount_path prefix before delegating to ServeDir.
fn rebuild_request_with_path(parts: &http::request::Parts, path: &str) -> Request {
    let uri = format!("/{path}");
    let mut builder = http::Request::builder()
        .method(parts.method.clone())
        .uri(&uri)
        .version(parts.version);
    for (k, v) in &parts.headers {
        builder = builder.header(k, v);
    }
    builder
        .extension(parts.extensions.clone())
        .body(AxumBody::empty())
        .expect("valid request rebuild") // allow-unwrap
}

/// Serve a static file by delegating to `ServeDir`, adding `Cache-Control` on success.
async fn serve_via_serve_dir(
    serve_dir: ServeDir,
    req: Request,
    cache_control: &str,
) -> axum::response::Response {
    match serve_dir.oneshot(req).await {
        Ok(res) => {
            let (mut parts, body) = res.into_parts();
            if parts.status.is_success() {
                parts.headers.insert(
                    http::header::CACHE_CONTROL,
                    http::HeaderValue::from_str(cache_control)
                        .unwrap_or_else(|_| http::HeaderValue::from_static("public, max-age=0")),
                );
            }
            Response::from_parts(parts, AxumBody::new(body))
        }
        Err(_) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

/// Check whether a request qualifies for SPA fallback.
///
/// SPA fallback only triggers when ALL conditions are met:
/// - Request method is GET or HEAD
/// - `Accept` header includes `text/html` or `*/*`
/// - Request path has no file extension
fn is_spa_qualified(req: &Request) -> bool {
    let method = req.method();
    if method != http::Method::GET && method != http::Method::HEAD {
        return false;
    }
    let accept = req
        .headers()
        .get(http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !accept.contains("text/html") && !accept.contains("*/*") {
        return false;
    }
    let path = req.uri().path();
    let has_extension = std::path::Path::new(path).extension().is_some();
    !has_extension
}

/// Serve the SPA index.html by rewriting the request URI to "/".
/// ServeDir serves from the root of the configured directory — the mount_path
/// is only for URL routing, not file paths.
async fn serve_spa_index(
    serve_dir: ServeDir,
    req: Request,
    cache_control: &str,
) -> axum::response::Response {
    let method = req.method().clone();
    let headers = req.headers().clone();
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(axum::http::Uri::from_static("/"));
    for (k, v) in headers.iter() {
        builder = builder.header(k, v);
    }
    let rewrite = builder.body(AxumBody::empty()).expect("valid request"); // allow-unwrap
    serve_via_serve_dir(serve_dir, rewrite, cache_control).await
}

/// Attempt to serve a custom error page for the given status code.
/// Returns `Some(response)` if an error page is configured and served, `None` otherwise.
///
/// The response status is overridden to the original error status so that
/// serving a 404.html returns 404 (not 200).
async fn try_serve_error_page(
    error_pages: &HashMap<u16, PathBuf>,
    status: u16,
) -> Option<axum::response::Response> {
    if let Some(error_path) = error_pages.get(&status) {
        // Build a ServeDir for the error page's parent directory
        if let Some(parent) = error_path.parent() {
            let file_name = error_path.file_name()?.to_str()?;
            let error_serve_dir = ServeDir::new(parent);
            let error_req = axum::http::Request::builder()
                .method(http::Method::GET)
                .uri(format!("/{file_name}"))
                .body(AxumBody::empty())
                .expect("valid request"); // allow-unwrap
            let mut resp = serve_via_serve_dir(error_serve_dir, error_req, "no-cache").await;
            // Override status: ServeDir returns 200 for the file, but we want the original error code.
            if resp.status().is_success()
                && let Ok(code) = StatusCode::from_u16(status)
            {
                *resp.status_mut() = code;
            }
            return Some(resp);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::prefix_matches_segment;

    #[test]
    fn root_prefix_matches_everything() {
        assert!(prefix_matches_segment("/", "/"));
        assert!(prefix_matches_segment("/foo", "/"));
        assert!(prefix_matches_segment("/foo/bar", "/"));
    }

    #[test]
    fn exact_prefix_match() {
        assert!(prefix_matches_segment("/asset", "/asset"));
        assert!(prefix_matches_segment("/asset/file.txt", "/asset"));
    }

    #[test]
    fn segment_boundary_rejects_partial_match() {
        // Mount at /asset must NOT match /assets
        assert!(!prefix_matches_segment("/assets", "/asset"));
        assert!(!prefix_matches_segment("/assets/file.txt", "/asset"));
        assert!(!prefix_matches_segment("/assetx", "/asset"));
    }

    #[test]
    fn non_matching_prefix() {
        assert!(!prefix_matches_segment("/other", "/asset"));
        assert!(!prefix_matches_segment("/other/file.txt", "/asset"));
    }

    #[test]
    fn nested_prefix() {
        assert!(prefix_matches_segment(
            "/assets/sub/file.txt",
            "/assets/sub"
        ));
        assert!(!prefix_matches_segment(
            "/assets/other/file.txt",
            "/assets/sub"
        ));
    }
}

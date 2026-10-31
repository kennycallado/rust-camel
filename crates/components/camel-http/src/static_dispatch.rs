use std::collections::HashMap;
use std::path::PathBuf;

use axum::body::Body as AxumBody;
use axum::extract::Request;
use axum::http::{Response, StatusCode};
use axum::response::IntoResponse;
use tower::ServiceExt as TowerServiceExt;
use tower_http::services::ServeDir;

#[cfg(test)]
use crate::AppState;
use crate::registry::HttpRouteRegistryInner;
use crate::registry::MountMode;
use crate::registry::StaticMount;

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
#[cfg(test)]
pub(crate) async fn dispatch_static(
    state: &AppState,
    req: Request,
    path: &str,
) -> axum::response::Response {
    let snapshot = state.registry.inner.load();
    dispatch_static_with(&snapshot, req, path).await
}

/// Serve a static request against a caller-supplied registry snapshot.
///
/// The caller loads the snapshot once per request and passes the same Arc
/// reference here, so mount selection, ServeDir serving, and the root
/// error-page fallback all observe one consistent registry generation. The
/// reference is an `Arc`, not a lock: writers never block readers and vice
/// versa.
pub(crate) async fn dispatch_static_with(
    inner: &HttpRouteRegistryInner,
    req: Request,
    path: &str,
) -> axum::response::Response {
    // Path traversal is contained by ServeDir (tower-http 0.7.0), which
    // canonicalizes and rejects `..` / percent-encoded sequences. The static
    // root directory and its symlink targets must be trusted by the operator.
    // (R4-L1: removed dead-weight string guard that missed %2e%2e.)
    let relative = path.trim_start_matches('/');

    // Extract request parts once so we can rebuild for each mount attempt
    let (parts, _body) = req.into_parts();

    // Content-aware HTML 404 gate (rc-n7bgz): reuse the SPA-qualification rule
    // (GET/HEAD, Accept text/html or */*, no file extension) so the custom HTML
    // error page is only served for navigation requests, not missing assets.
    let html_qualified = is_spa_qualified(&rebuild_request(&parts));

    // One caller-supplied snapshot serves both mount selection and the root
    // error-page fallback; it holds no lock, so readers never block a
    // registering writer.
    // Sort mounts by mount_path length DESC (longest prefix wins)
    let mut indexed: Vec<&StaticMount> = inner.mounts.iter().collect();
    indexed.sort_by_key(|mount| std::cmp::Reverse(mount.mount_path.len()));

    // Track best-matching mount for error page scoping (Fix 3)
    let mut best_match: Option<&StaticMount> = None;

    // Try each mount — longest prefix first
    for mount in indexed {
        if !prefix_matches_segment(path, mount.mount_path.as_str()) {
            continue;
        }

        // Record best match (only on first/longest match)
        if best_match.is_none() {
            best_match = Some(mount);
        }

        // Strip the mount_path prefix and rebuild the request URI for ServeDir
        let stripped_path = if mount.mount_path == "/" {
            relative.to_string()
        } else {
            let remainder = path.strip_prefix(mount.mount_path.as_str()).unwrap_or("");
            remainder.trim_start_matches('/').to_string()
        };

        let req = rebuild_request_with_path(&parts, &stripped_path);
        let resp = serve_via_serve_dir(mount.serve_dir.clone(), req, &mount.cache_control).await;
        // Forward ServeDir's success, its legitimate 304 Not Modified for
        // conditional GETs, and its 3xx directory redirects. Redirect targets
        // were built from the prefix-stripped URI, so re-add the mount prefix
        // before returning them to the client.
        if resp.status().is_success() || resp.status().is_redirection() {
            // 304 is in the 3xx class but is not a redirect — return untouched.
            let resp =
                if resp.status().is_redirection() && resp.status() != StatusCode::NOT_MODIFIED {
                    rewrite_redirect_location(resp, &mount.mount_path)
                } else {
                    resp
                };
            return resp;
        }

        // ServeDir returned non-2xx
        // For SPA mounts, try SPA fallback BEFORE error pages (SPA wins).
        // SPA fallback means all unknown routes are handled by the SPA's
        // index.html. Explicit error pages are only used when SPA fallback
        // is disabled (MountMode::Static), when the request is not
        // SPA-qualified (e.g., has a file extension), or when the SPA
        // fallback itself fails.
        if mount.mode == MountMode::Spa && resp.status() == StatusCode::NOT_FOUND {
            let spa_req = rebuild_request(&parts);
            if is_spa_qualified(&spa_req) {
                return serve_spa_index(mount.serve_dir.clone(), spa_req, &mount.cache_control)
                    .await;
            }
        }

        // Try error page for this mount (for Static mode or non-qualified SPA
        // requests). A 404 is content-gated: only HTML navigation gets the
        // custom page; missing assets get the plain 404 below.
        if (resp.status() != StatusCode::NOT_FOUND || html_qualified)
            && let Some(error_resp) =
                try_serve_error_page(&mount.error_pages, resp.status().as_u16()).await
        {
            return error_resp;
        }
    }

    // No mount matched — use best-match error pages only (Fix 3)
    if let Some(mount) = best_match
        && html_qualified
        && let Some(error_resp) =
            try_serve_error_page(&mount.error_pages, StatusCode::NOT_FOUND.as_u16()).await
    {
        return error_resp;
    }

    // Optional: try root-mount ("/") error pages as last resort
    let root_error_pages = inner
        .mounts
        .iter()
        .find(|mount| mount.mount_path == "/")
        .map(|mount| &mount.error_pages);
    if let Some(pages) = root_error_pages
        && html_qualified
        && let Some(error_resp) = try_serve_error_page(pages, StatusCode::NOT_FOUND.as_u16()).await
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
/// The original query string is preserved so ServeDir's directory redirect
/// `Location` already carries it.
fn rebuild_request_with_path(parts: &http::request::Parts, path: &str) -> Request {
    let uri = match parts.uri.query() {
        Some(query) => format!("/{path}?{query}"),
        None => format!("/{path}"),
    };
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

/// Rewrite a ServeDir redirect `Location` for the client-facing URL space.
///
/// ServeDir built the Location from the request it received, whose path had
/// the mount prefix stripped. Authority-form (`//host/…`) and absolute-URI
/// (`scheme://…`) targets are external and forwarded untouched. Every other
/// Location — root-relative, path-relative, or unparseable by `http::Uri` —
/// is treated as an internal relative reference and gets the mount prefix
/// re-added. The query string rides inside the Location value and is
/// preserved.
fn rewrite_redirect_location(
    resp: axum::response::Response,
    mount_path: &str,
) -> axum::response::Response {
    let Some(location) = resp
        .headers()
        .get(http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
    else {
        return resp;
    };
    // Authority-form (`//host/…`) is scheme-relative and therefore external.
    // `http::Uri` cannot distinguish this from a path, so detect it textually.
    if location.starts_with("//") {
        return resp;
    }
    // An absolute URI (scheme://…) is external. Uri-authority alone is NOT
    // evidence of externality: `http::Uri` parses path-relative references
    // such as `about` as an authority, and rejects others such as `about/`.
    // Both are internal relative references and must fall through to the
    // rewrite below; parse failure is a crate limitation, not externality.
    if let Ok(target) = location.parse::<http::Uri>()
        && target.scheme().is_some()
    {
        return resp;
    }
    let prefix = if mount_path == "/" { "" } else { mount_path };
    let rewritten = if location.starts_with('/') {
        format!("{prefix}{location}")
    } else {
        format!("{prefix}/{location}")
    };
    let Ok(value) = http::HeaderValue::from_str(&rewritten) else {
        return resp;
    };
    let (mut parts, body) = resp.into_parts();
    parts.headers.insert(http::header::LOCATION, value);
    Response::from_parts(parts, body)
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
            // Attach Cache-Control on 2xx success AND on 304 Not Modified.
            // Per RFC 7232 §4.1, a 304 response SHOULD include Cache-Control
            // (and the validators ETag/Last-Modified, which ServeDir already
            // carries over from the original 200). Without this gate, a
            // conditional GET that correctly returns 304 would be missing
            // the caching directive.
            if parts.status.is_success() || parts.status == StatusCode::NOT_MODIFIED {
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
    use super::rewrite_redirect_location;
    use axum::body::Body as AxumBody;
    use axum::http::Response;
    use axum::http::StatusCode;

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

    fn redirect_with_location(location: &str) -> axum::response::Response {
        Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(http::header::LOCATION, location)
            .body(AxumBody::empty())
            .expect("valid redirect response")
    }

    fn location_of(resp: &axum::response::Response) -> Option<&str> {
        resp.headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok())
    }

    #[test]
    fn rewrite_adds_mount_prefix_to_root_relative_location() {
        let resp = rewrite_redirect_location(redirect_with_location("/about/"), "/docs");
        assert_eq!(location_of(&resp), Some("/docs/about/"));
    }

    #[test]
    fn rewrite_preserves_query_string_in_location() {
        let resp = rewrite_redirect_location(redirect_with_location("/about/?a=1"), "/docs");
        assert_eq!(location_of(&resp), Some("/docs/about/?a=1"));
    }

    #[test]
    fn rewrite_root_mount_does_not_double_prefix() {
        let resp = rewrite_redirect_location(redirect_with_location("/about/"), "/");
        assert_eq!(location_of(&resp), Some("/about/"));
    }

    #[test]
    fn rewrite_adds_mount_prefix_to_path_relative_location() {
        // `about` parses as an authority via http::Uri but is path-relative.
        let resp = rewrite_redirect_location(redirect_with_location("about"), "/docs");
        assert_eq!(location_of(&resp), Some("/docs/about"));
    }

    #[test]
    fn rewrite_adds_mount_prefix_to_path_relative_location_with_query() {
        // `about/` fails to parse via http::Uri; still an internal relative ref.
        let resp = rewrite_redirect_location(redirect_with_location("about/?a=1"), "/docs");
        assert_eq!(location_of(&resp), Some("/docs/about/?a=1"));
    }

    #[test]
    fn rewrite_path_relative_location_on_root_mount() {
        let resp = rewrite_redirect_location(redirect_with_location("about"), "/");
        assert_eq!(location_of(&resp), Some("/about"));
    }

    #[test]
    fn rewrite_leaves_absolute_uri_location_untouched() {
        let resp =
            rewrite_redirect_location(redirect_with_location("https://example.com/x/"), "/docs");
        assert_eq!(location_of(&resp), Some("https://example.com/x/"));
    }

    #[test]
    fn rewrite_leaves_authority_form_location_untouched() {
        let resp = rewrite_redirect_location(redirect_with_location("//example.com/x/"), "/docs");
        assert_eq!(location_of(&resp), Some("//example.com/x/"));
    }

    #[test]
    fn rewrite_without_location_returns_response_untouched() {
        let resp = Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .body(AxumBody::empty())
            .expect("valid redirect response");
        let resp = rewrite_redirect_location(resp, "/docs");
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(location_of(&resp), None);
    }
}

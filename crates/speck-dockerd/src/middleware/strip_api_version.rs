use axum::body::Body;
use axum::http::{Request, Uri};
use axum::middleware::Next;
use axum::response::Response;

/// Build a new `Uri` with the path replaced by `new_path`, preserving query string.
/// Returns `None` if the URI cannot be reconstructed (logged as a warning by callers).
pub fn rewrite_uri(uri: &Uri, new_path: &str) -> Option<Uri> {
    let mut parts = uri.clone().into_parts();
    let pq = match parts.path_and_query.as_ref().and_then(|pq| pq.query()) {
        Some(q) => format!("{new_path}?{q}").parse().ok()?,
        None => new_path.parse().ok()?,
    };
    parts.path_and_query = Some(pq);
    Uri::from_parts(parts).ok()
}

/// Strip the Docker API version prefix (`/v1.44/`, `/v1.24/`, etc.) from
/// the request URI path before it reaches the router.
///
/// Docker CLI always sends versioned paths (e.g. `/v1.44/info`). Our router
/// registers unversioned routes (`/info`). This middleware normalises the path
/// so both forms work transparently.
pub async fn strip_api_version(mut req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    if let Some(stripped) = strip_version_prefix(&path)
        && let Some(new_uri) = rewrite_uri(req.uri(), stripped)
    {
        *req.uri_mut() = new_uri;
    }
    next.run(req).await
}

/// Return the path with the leading `/v{digits}[.{digits}]/` segment removed,
/// or `None` if the path does not start with such a segment.
pub fn strip_version_prefix(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/v")?;
    let slash = rest.find('/')?;
    let version_part = &rest[..slash];
    // Accept only numeric version strings like "1", "1.44", "1.44.0".
    if version_part
        .split('.')
        .all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()))
    {
        Some(&rest[slash..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::strip_version_prefix;

    #[test]
    fn strips_minor_version() {
        assert_eq!(strip_version_prefix("/v1.44/info"), Some("/info"));
    }

    #[test]
    fn strips_major_only_version() {
        assert_eq!(
            strip_version_prefix("/v1/containers/json"),
            Some("/containers/json")
        );
    }

    #[test]
    fn strips_patch_version() {
        assert_eq!(
            strip_version_prefix("/v1.44.0/images/json"),
            Some("/images/json")
        );
    }

    #[test]
    fn passes_through_unversioned() {
        assert_eq!(strip_version_prefix("/info"), None);
        assert_eq!(strip_version_prefix("/_ping"), None);
    }

    #[test]
    fn does_not_strip_non_numeric() {
        assert_eq!(strip_version_prefix("/vbeta/info"), None);
        assert_eq!(strip_version_prefix("/v1.x/info"), None);
    }

    #[test]
    fn preserves_trailing_path() {
        assert_eq!(
            strip_version_prefix("/v1.44/containers/abc123/json"),
            Some("/containers/abc123/json")
        );
    }
}

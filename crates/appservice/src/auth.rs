use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use subtle::ConstantTimeEq;

/// Extract a Bearer token from the request.
///
/// Checks two locations (in order):
/// 1. `Authorization: Bearer {token}` header
/// 2. `access_token={token}` query parameter (legacy)
fn extract_bearer_token(request: &Request) -> Option<String> {
    let from_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let from_query = {
        let query = request.uri().query().unwrap_or("");
        parse_access_token(query).map(|s| s.to_string())
    };

    from_header.or(from_query)
}

/// Verify that the request carries a valid token (constant-time comparison).
///
/// Uses HMAC-based comparison to avoid leaking the expected token's length
/// through timing side-channels. Both inputs are hashed with SHA-256 before
/// comparison, so the ct_eq always operates on fixed-size (32-byte) digests.
pub(crate) fn verify_token(provided: &str, expected: &str) -> bool {
    use sha2::{Digest, Sha256};
    let h_provided = Sha256::digest(provided.as_bytes());
    let h_expected = Sha256::digest(expected.as_bytes());
    h_provided.ct_eq(&h_expected).into()
}

/// Middleware: verify the homeserver token (`hs_token`) on Matrix appservice routes.
///
/// This authenticates requests from the Matrix homeserver (Synapse).
/// Uses constant-time comparison to prevent timing side-channel attacks.
pub async fn verify_hs_token(request: Request, next: Next) -> Result<Response, StatusCode> {
    let Some(HsToken(expected)) = request.extensions().get::<HsToken>().cloned() else {
        tracing::error!("HsToken extension missing — route misconfiguration");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    };

    match extract_bearer_token(&request) {
        Some(t) if verify_token(&t, &expected) => Ok(next.run(request).await),
        Some(_) => {
            tracing::warn!("invalid hs_token in request");
            Err(StatusCode::FORBIDDEN)
        }
        None => {
            tracing::warn!("missing hs_token in request");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Middleware: verify the API key on Bridge HTTP API routes.
///
/// This authenticates requests from external platform services.
/// Separate from `hs_token` which is a Matrix protocol secret.
pub async fn verify_api_key(request: Request, next: Next) -> Result<Response, StatusCode> {
    let Some(ApiKey(expected)) = request.extensions().get::<ApiKey>().cloned() else {
        tracing::error!("ApiKey extension missing — route misconfiguration");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    };

    match extract_bearer_token(&request) {
        Some(t) if verify_token(&t, &expected) => Ok(next.run(request).await),
        Some(_) => {
            tracing::warn!("invalid api_key in bridge API request");
            Err(StatusCode::FORBIDDEN)
        }
        None => {
            tracing::warn!("missing api_key in bridge API request");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Newtype for the homeserver token (Synapse ↔ appservice), stored in axum extensions.
#[derive(Clone)]
pub struct HsToken(pub String);

/// Newtype for the Bridge API key (external services ↔ bridge), stored in axum extensions.
#[derive(Clone)]
pub struct ApiKey(pub String);

fn parse_access_token(query: &str) -> Option<&str> {
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("access_token=") {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_access_token() {
        assert_eq!(
            parse_access_token("access_token=abc123&foo=bar"),
            Some("abc123")
        );
        assert_eq!(parse_access_token("foo=bar"), None);
        assert_eq!(parse_access_token(""), None);
        assert_eq!(parse_access_token("x=1&access_token=tok"), Some("tok"));
    }

    #[test]
    fn test_parse_bearer_token() {
        assert_eq!(
            "Bearer mytoken123"
                .strip_prefix("Bearer ")
                .map(|s| s.to_string()),
            Some("mytoken123".to_string())
        );
        assert_eq!(
            "bearer mytoken123".strip_prefix("Bearer "),
            None,
            "Bearer prefix is case-sensitive"
        );
        assert_eq!("Basic abc123".strip_prefix("Bearer "), None,);
    }

    #[test]
    fn test_verify_token() {
        assert!(verify_token("secret123", "secret123"));
        assert!(!verify_token("secret123", "wrong"));
        assert!(!verify_token("short", "longer_token"));
        assert!(!verify_token("", "notempty"));
        assert!(verify_token("", ""));
    }
}

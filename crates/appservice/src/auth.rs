use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use subtle::ConstantTimeEq;

/// Extract and verify the hs_token from the request.
///
/// Accepts two formats (checked in order):
/// 1. `Authorization: Bearer {token}` header (Synapse 1.149+)
/// 2. `access_token={token}` query parameter (legacy)
///
/// Uses constant-time comparison to prevent timing side-channel attacks.
pub async fn verify_hs_token(request: Request, next: Next) -> Result<Response, StatusCode> {
    let expected_token = request
        .extensions()
        .get::<HsToken>()
        .map(|t| t.0.clone())
        .unwrap_or_default();

    // Try Authorization: Bearer header first, then fall back to query param.
    let token_from_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let token_from_query = {
        let query = request.uri().query().unwrap_or("");
        parse_access_token(query).map(|s| s.to_string())
    };

    let token = token_from_header.or(token_from_query);

    match token {
        Some(t) if constant_time_eq(t.as_bytes(), expected_token.as_bytes()) => {
            Ok(next.run(request).await)
        }
        Some(_) => {
            tracing::warn!("invalid hs_token in request");
            Err(StatusCode::FORBIDDEN)
        }
        None => {
            tracing::warn!("missing access_token in request");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// Newtype for the homeserver token, stored in axum extensions.
#[derive(Clone)]
pub struct HsToken(pub String);

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
}

use futures_util::StreamExt;
use secrecy::ExposeSecret;
use serde_json::Value;

use super::MatrixClient;

/// Maximum media size we will download from the homeserver (200 MB).
const MAX_MEDIA_DOWNLOAD_SIZE: u64 = 200 * 1024 * 1024;

impl MatrixClient {
    /// Upload media to the homeserver. Returns the mxc:// URI.
    pub async fn upload_media(
        &self,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String> {
        let url = format!("{}/_matrix/media/v3/upload", self.homeserver_url);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.as_token.expose_secret())
            .query(&[("filename", filename)])
            .header("content-type", content_type)
            .body(data)
            .send()
            .await?;

        let resp = Self::check_response(resp, "media upload").await?;
        let body: Value = resp.json().await?;
        body.get("content_uri")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("upload response missing content_uri"))
    }

    /// Download media from the homeserver by mxc:// URI.
    ///
    /// Returns the raw bytes and the Content-Type header value.
    /// Enforces a maximum download size to prevent unbounded memory allocation.
    pub async fn download_media(&self, mxc_uri: &str) -> anyhow::Result<(Vec<u8>, String)> {
        let (server_name, media_id) = parse_mxc(mxc_uri)?;
        let url = format!(
            "{}/_matrix/media/v3/download/{}/{}",
            self.homeserver_url, server_name, media_id,
        );

        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.as_token.expose_secret())
            .send()
            .await?;

        let resp = Self::check_response(resp, "media download").await?;

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        // Check Content-Length header before reading body.
        if let Some(len) = resp.content_length() {
            if len > MAX_MEDIA_DOWNLOAD_SIZE {
                anyhow::bail!(
                    "media too large: Content-Length {len} exceeds {} byte limit",
                    MAX_MEDIA_DOWNLOAD_SIZE
                );
            }
        }

        // Stream with incremental size enforcement.
        let mut data = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if data.len() + chunk.len() > MAX_MEDIA_DOWNLOAD_SIZE as usize {
                anyhow::bail!(
                    "media too large: exceeded {} byte limit during download",
                    MAX_MEDIA_DOWNLOAD_SIZE
                );
            }
            data.extend_from_slice(&chunk);
        }

        Ok((data, content_type))
    }

    /// Convert an `mxc://` URI to a publicly downloadable HTTP URL on the
    /// homeserver. External platforms can use this URL to fetch the media.
    pub fn mxc_to_download_url(&self, mxc_uri: &str) -> Option<String> {
        let (server_name, media_id) = parse_mxc(mxc_uri).ok()?;
        Some(format!(
            "{}/_matrix/media/v3/download/{}/{}",
            self.homeserver_url, server_name, media_id,
        ))
    }
}

/// Parse an `mxc://server_name/media_id` URI into its components.
///
/// Validates that both components contain only safe characters to prevent
/// path traversal or URL injection when interpolated into HTTP URLs.
fn parse_mxc(mxc_uri: &str) -> anyhow::Result<(&str, &str)> {
    let rest = mxc_uri
        .strip_prefix("mxc://")
        .ok_or_else(|| anyhow::anyhow!("invalid mxc URI: {mxc_uri}"))?;
    let (server_name, media_id) = rest
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("invalid mxc URI (no media_id): {mxc_uri}"))?;
    if server_name.is_empty() || media_id.is_empty() {
        anyhow::bail!("invalid mxc URI (empty component): {mxc_uri}");
    }
    // Reject path traversal and unsafe characters. Server names may contain
    // alphanumeric, hyphens, dots, colons (port). Media IDs may contain
    // alphanumeric and a limited set of URL-safe characters.
    if !is_safe_mxc_component(server_name) || !is_safe_mxc_component(media_id) {
        anyhow::bail!("invalid mxc URI (unsafe characters): {mxc_uri}");
    }
    Ok((server_name, media_id))
}

/// Check that an mxc URI component contains only safe characters.
/// Rejects path traversal sequences (.., /, \, %, NUL).
fn is_safe_mxc_component(s: &str) -> bool {
    if s.contains("..") {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mxc_valid() {
        let (server, media) = parse_mxc("mxc://example.com/abc123").unwrap();
        assert_eq!(server, "example.com");
        assert_eq!(media, "abc123");
    }

    #[test]
    fn parse_mxc_invalid_scheme() {
        assert!(parse_mxc("https://example.com/abc").is_err());
    }

    #[test]
    fn parse_mxc_missing_media_id() {
        assert!(parse_mxc("mxc://example.com").is_err());
    }

    #[test]
    fn parse_mxc_empty_components() {
        assert!(parse_mxc("mxc:///abc").is_err());
        assert!(parse_mxc("mxc://example.com/").is_err());
    }
}

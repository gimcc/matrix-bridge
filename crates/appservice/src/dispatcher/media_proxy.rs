//! Download external media (http/https URLs) and re-upload to the Matrix
//! media repository so that messages always reference `mxc://` URIs.
//!
//! When the target room uses encryption, files are AES-256-CTR encrypted
//! before uploading, and the resulting `EncryptedAttachment` metadata is
//! stored so that `to_matrix_content` can emit a `file` object instead of
//! a plain `url` field.

use futures_util::StreamExt;
use matrix_bridge_core::config::MAX_MEDIA_SIZE;
use matrix_bridge_core::message::MessageContent;
use tracing::{debug, warn};

use super::Dispatcher;
use super::attachment_crypto::{EncryptedAttachment, encrypt_attachment_full};

/// The result of reuploading media: the updated `MessageContent` plus
/// optional encryption metadata for each media field.
pub(super) struct ReuploadResult {
    pub content: MessageContent,
    /// Encryption metadata (set when the target room is encrypted).
    /// Used by `to_matrix_content` to emit `"file"` instead of `"url"`.
    pub encrypted_file: Option<EncryptedAttachment>,
}

impl Dispatcher {
    /// Download external media and reupload to Matrix. If `encrypt` is true,
    /// the file is AES-256-CTR encrypted before uploading per the Matrix spec.
    pub(super) async fn reupload_external_media(
        &self,
        content: MessageContent,
        encrypt: bool,
    ) -> ReuploadResult {
        match content {
            MessageContent::Image {
                url,
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
            } => {
                let fallback = filename.as_deref().unwrap_or("image");
                let result = self.ensure_mxc(&url, &mimetype, fallback, encrypt).await;
                ReuploadResult {
                    content: MessageContent::Image {
                        url: result.mxc_uri,
                        caption,
                        mimetype,
                        filename,
                        width,
                        height,
                        size,
                    },
                    encrypted_file: result.encrypted,
                }
            }
            MessageContent::File {
                url,
                filename,
                mimetype,
                size,
            } => {
                let result = self
                    .ensure_mxc_with_filename(&url, &mimetype, &filename, encrypt)
                    .await;
                ReuploadResult {
                    content: MessageContent::File {
                        url: result.mxc_uri,
                        filename,
                        mimetype,
                        size,
                    },
                    encrypted_file: result.encrypted,
                }
            }
            MessageContent::Video {
                url,
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
                duration,
            } => {
                let fallback = filename.as_deref().unwrap_or("video");
                let result = self.ensure_mxc(&url, &mimetype, fallback, encrypt).await;
                ReuploadResult {
                    content: MessageContent::Video {
                        url: result.mxc_uri,
                        caption,
                        mimetype,
                        filename,
                        width,
                        height,
                        size,
                        duration,
                    },
                    encrypted_file: result.encrypted,
                }
            }
            MessageContent::Audio {
                url,
                mimetype,
                filename,
                size,
                duration,
            } => {
                let fallback = filename.as_deref().unwrap_or("audio");
                let result = self.ensure_mxc(&url, &mimetype, fallback, encrypt).await;
                ReuploadResult {
                    content: MessageContent::Audio {
                        url: result.mxc_uri,
                        mimetype,
                        filename,
                        size,
                        duration,
                    },
                    encrypted_file: result.encrypted,
                }
            }
            other => ReuploadResult {
                content: other,
                encrypted_file: None,
            },
        }
    }

    /// If `url` is an external HTTP(S) URL, download and reupload it.
    async fn ensure_mxc(
        &self,
        url: &str,
        mimetype: &str,
        fallback_name: &str,
        encrypt: bool,
    ) -> MediaUploadResult {
        if !is_external_url(url) {
            return MediaUploadResult {
                mxc_uri: url.to_string(),
                encrypted: None,
            };
        }

        let filename = filename_from_url(url).unwrap_or_else(|| fallback_name.to_string());
        self.download_and_upload(url, mimetype, &filename, encrypt)
            .await
    }

    /// Same as `ensure_mxc` but uses an explicit filename.
    async fn ensure_mxc_with_filename(
        &self,
        url: &str,
        mimetype: &str,
        filename: &str,
        encrypt: bool,
    ) -> MediaUploadResult {
        if !is_external_url(url) {
            return MediaUploadResult {
                mxc_uri: url.to_string(),
                encrypted: None,
            };
        }

        self.download_and_upload(url, mimetype, filename, encrypt)
            .await
    }

    /// Download from `url` using the SSRF-safe HTTP client and upload to Matrix.
    /// On failure, returns the original URL (message is still sent with a
    /// potentially broken media link).
    async fn download_and_upload(
        &self,
        url: &str,
        content_type: &str,
        filename: &str,
        encrypt: bool,
    ) -> MediaUploadResult {
        match self
            .download_and_upload_inner(url, content_type, filename, encrypt)
            .await
        {
            Ok(result) => {
                debug!(
                    external_url = url,
                    mxc_uri = result.mxc_uri,
                    filename,
                    encrypted = result.encrypted.is_some(),
                    "external media reuploaded to matrix"
                );
                result
            }
            Err(e) => {
                warn!(
                    external_url = url,
                    error = %e,
                    "failed to reupload external media, using original URL"
                );
                MediaUploadResult {
                    mxc_uri: url.to_string(),
                    encrypted: None,
                }
            }
        }
    }

    async fn download_and_upload_inner(
        &self,
        url: &str,
        content_type: &str,
        filename: &str,
        encrypt: bool,
    ) -> anyhow::Result<MediaUploadResult> {
        if encrypt && url.starts_with("http://") {
            warn!(
                external_url = url,
                "downloading media over plaintext HTTP for an encrypted room — \
                 file contents are exposed in transit before encryption"
            );
        }

        let resp = self.media_client.get(url).send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("download failed: HTTP {}", resp.status());
        }

        // Check Content-Length header before reading body.
        if let Some(len) = resp.content_length() {
            if len > MAX_MEDIA_SIZE as u64 {
                anyhow::bail!(
                    "file too large: Content-Length {len} exceeds {MAX_MEDIA_SIZE} byte limit"
                );
            }
        }

        // Use the server's Content-Type if available, fall back to the one from the message.
        let actual_content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| content_type.to_string());

        // Stream the response body with incremental size enforcement so a
        // server that omits Content-Length cannot force unbounded allocation.
        let mut data = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if data.len() + chunk.len() > MAX_MEDIA_SIZE {
                anyhow::bail!(
                    "file too large: exceeded {MAX_MEDIA_SIZE} byte limit during download"
                );
            }
            data.extend_from_slice(&chunk);
        }

        if encrypt {
            // Encrypt the file before uploading.
            let enc_data = encrypt_attachment_full(&data)?;

            // Upload the ciphertext (use application/octet-stream for encrypted data).
            let mxc_uri = self
                .matrix_client
                .upload_media(enc_data.ciphertext, "application/octet-stream", filename)
                .await?;

            Ok(MediaUploadResult {
                encrypted: Some(EncryptedAttachment {
                    mxc_uri: mxc_uri.clone(),
                    key_b64url: enc_data.key_b64url,
                    iv_b64: enc_data.iv_b64,
                    sha256_b64: enc_data.sha256_b64,
                }),
                mxc_uri,
            })
        } else {
            let mxc_uri = self
                .matrix_client
                .upload_media(data, &actual_content_type, filename)
                .await?;

            Ok(MediaUploadResult {
                mxc_uri,
                encrypted: None,
            })
        }
    }
}

struct MediaUploadResult {
    mxc_uri: String,
    encrypted: Option<EncryptedAttachment>,
}

fn is_external_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Extract a filename from a URL path, stripping query strings and fragments.
/// The result is sanitized: path traversal characters are removed and the
/// length is capped at 255 characters (not bytes) to avoid Unicode panics.
fn filename_from_url(url: &str) -> Option<String> {
    // Strip scheme and authority: "https://host/path" → "/path"
    let after_scheme = url.split("://").nth(1)?;
    let path = after_scheme.find('/').map(|i| &after_scheme[i..])?;
    let path = path.split('?').next()?.split('#').next()?;
    let segment = path.rsplit('/').next()?;
    let decoded = urlencoding::decode(segment).ok()?;
    let name = decoded.trim();
    if name.is_empty() {
        return None;
    }
    // Sanitize: strip path traversal characters and control chars.
    let sanitized: String = name
        .replace(['/', '\\', '\0'], "_")
        .trim_start_matches('.')
        .to_string();
    if sanitized.is_empty() {
        return None;
    }
    // Truncate at char boundary (not byte boundary) to avoid panic.
    let truncated: String = sanitized.chars().take(255).collect();
    Some(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_from_simple_url() {
        assert_eq!(
            filename_from_url("https://example.com/photos/cat.jpg"),
            Some("cat.jpg".to_string())
        );
    }

    #[test]
    fn filename_from_url_with_query() {
        assert_eq!(
            filename_from_url("https://cdn.example.com/file/doc.pdf?token=abc123"),
            Some("doc.pdf".to_string())
        );
    }

    #[test]
    fn filename_from_url_with_fragment() {
        assert_eq!(
            filename_from_url("https://example.com/image.png#section"),
            Some("image.png".to_string())
        );
    }

    #[test]
    fn filename_from_url_encoded() {
        assert_eq!(
            filename_from_url("https://example.com/my%20photo.jpg"),
            Some("my photo.jpg".to_string())
        );
    }

    #[test]
    fn filename_from_url_no_name() {
        assert_eq!(filename_from_url("https://example.com/"), None);
    }

    #[test]
    fn filename_from_url_empty_path() {
        assert_eq!(filename_from_url("https://example.com"), None);
    }

    #[test]
    fn external_url_detection() {
        assert!(is_external_url("https://example.com/file.jpg"));
        assert!(is_external_url("http://example.com/file.jpg"));
        assert!(!is_external_url("mxc://example.com/abc"));
        assert!(!is_external_url("ftp://example.com/file"));
        assert!(!is_external_url(""));
    }
}

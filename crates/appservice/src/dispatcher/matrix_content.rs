use serde_json::Value;

use matrix_bridge_core::message::MessageContent;

use super::Dispatcher;

/// Result of parsing a Matrix message content, including optional encryption
/// metadata when the media was an encrypted attachment (`content["file"]`).
pub(super) struct ParsedContent {
    pub content: MessageContent,
    /// When `true`, the media URL points to encrypted ciphertext on the
    /// homeserver and must be decrypted before forwarding to external platforms.
    pub encrypted_media: bool,
    /// Encryption key material extracted from `content["file"]`.
    pub file_encryption: Option<FileEncryption>,
}

/// Encryption metadata extracted from a Matrix `content["file"]` object.
/// Used to decrypt attachments before forwarding outbound.
///
/// Does NOT derive `Debug` to prevent accidental logging of AES key material.
#[derive(Clone)]
pub(super) struct FileEncryption {
    /// Base64url-encoded AES-256 key (JWK `k` field).
    pub key_b64url: String,
    /// Base64-encoded IV.
    pub iv_b64: String,
    /// Unpadded base64-encoded SHA-256 of the ciphertext.
    pub sha256_b64: String,
}

impl Dispatcher {
    /// Parse a Matrix message content JSON into a `MessageContent` with
    /// optional encryption metadata for encrypted attachments.
    pub(super) fn parse_message_content(
        msgtype: &str,
        body: &str,
        content: &Value,
    ) -> Option<ParsedContent> {
        match msgtype {
            "m.text" => {
                let formatted = content
                    .get("formatted_body")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Some(ParsedContent {
                    content: MessageContent::Text {
                        body: body.to_string(),
                        formatted_body: formatted,
                    },
                    encrypted_media: false,
                    file_encryption: None,
                })
            }
            "m.notice" => Some(ParsedContent {
                content: MessageContent::Notice {
                    body: body.to_string(),
                },
                encrypted_media: false,
                file_encryption: None,
            }),
            "m.emote" => Some(ParsedContent {
                content: MessageContent::Emote {
                    body: body.to_string(),
                },
                encrypted_media: false,
                file_encryption: None,
            }),
            "m.image" => Some(Self::parse_media_content(content, body, "image/png", true)),
            "m.file" => {
                let (url, encrypted_media, file_encryption) = Self::extract_media_url(content);
                let mimetype = Self::extract_mimetype(content, "application/octet-stream");
                let size = content.get("info").and_then(|i| i.get("size")).and_then(|v| v.as_u64());
                Some(ParsedContent {
                    content: MessageContent::File {
                        url,
                        filename: body.to_string(),
                        mimetype,
                        size,
                    },
                    encrypted_media,
                    file_encryption,
                })
            }
            "m.video" => Some(Self::parse_media_content(content, body, "video/mp4", true)),
            "m.audio" => {
                let (url, encrypted_media, file_encryption) = Self::extract_media_url(content);
                let mimetype = Self::extract_mimetype(content, "audio/ogg");
                let info = content.get("info");
                let size = info.and_then(|i| i.get("size")).and_then(|v| v.as_u64());
                let duration = info.and_then(|i| i.get("duration")).and_then(|v| v.as_u64());
                let filename = content
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Some(ParsedContent {
                    content: MessageContent::Audio {
                        url,
                        mimetype,
                        filename,
                        size,
                        duration,
                    },
                    encrypted_media,
                    file_encryption,
                })
            }
            _ => None,
        }
    }

    fn parse_media_content(
        content: &Value,
        body: &str,
        default_mime: &str,
        is_visual: bool,
    ) -> ParsedContent {
        let (url, encrypted_media, file_encryption) = Self::extract_media_url(content);
        let mimetype = Self::extract_mimetype(content, default_mime);
        let caption = Some(body.to_string()).filter(|s| !s.is_empty());
        let info = content.get("info");
        let width = info.and_then(|i| i.get("w")).and_then(|v| v.as_u64()).map(|v| v as u32);
        let height = info.and_then(|i| i.get("h")).and_then(|v| v.as_u64()).map(|v| v as u32);
        let size = info.and_then(|i| i.get("size")).and_then(|v| v.as_u64());
        let duration = info.and_then(|i| i.get("duration")).and_then(|v| v.as_u64());
        let filename = content
            .get("filename")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mc = if is_visual && default_mime.starts_with("video") {
            MessageContent::Video {
                url,
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
                duration,
            }
        } else {
            MessageContent::Image {
                url,
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
            }
        };
        ParsedContent {
            content: mc,
            encrypted_media,
            file_encryption,
        }
    }

    /// Extract the media URL and optional encryption metadata from content.
    ///
    /// Unencrypted media has `content["url"]`.
    /// Encrypted media has `content["file"]["url"]` plus key/iv/hash.
    fn extract_media_url(content: &Value) -> (String, bool, Option<FileEncryption>) {
        // Try plain `url` first (unencrypted media).
        if let Some(url) = content.get("url").and_then(|v| v.as_str()) {
            if !url.is_empty() {
                return (url.to_string(), false, None);
            }
        }

        // Fall back to `file.url` (encrypted media).
        if let Some(file) = content.get("file") {
            let url = file
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !url.is_empty() {
                let encryption = Self::extract_file_encryption(file);
                return (url, true, encryption);
            }
        }

        (String::new(), false, None)
    }

    /// Extract encryption key material from a Matrix `file` JSON object.
    fn extract_file_encryption(file: &Value) -> Option<FileEncryption> {
        let key_b64url = file
            .get("key")
            .and_then(|k| k.get("k"))
            .and_then(|v| v.as_str())?
            .to_string();
        let iv_b64 = file.get("iv").and_then(|v| v.as_str())?.to_string();
        let sha256_b64 = file
            .get("hashes")
            .and_then(|h| h.get("sha256"))
            .and_then(|v| v.as_str())?
            .to_string();

        Some(FileEncryption {
            key_b64url,
            iv_b64,
            sha256_b64,
        })
    }

    fn extract_mimetype(content: &Value, default: &str) -> String {
        content
            .get("info")
            .and_then(|i| i.get("mimetype"))
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_string()
    }
}

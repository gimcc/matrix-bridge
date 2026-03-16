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
            "m.image" => Some(Self::parse_media_content(content, body, "image/png")),
            "m.file" => {
                let (url, encrypted_media, file_encryption) = Self::extract_media_url(content);
                let mimetype = Self::extract_mimetype(content, "application/octet-stream");
                let size = content
                    .get("info")
                    .and_then(|i| i.get("size"))
                    .and_then(|v| v.as_u64());
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
            "m.video" => Some(Self::parse_media_content(content, body, "video/mp4")),
            "m.audio" => {
                let (url, encrypted_media, file_encryption) = Self::extract_media_url(content);
                let mimetype = Self::extract_mimetype(content, "audio/ogg");
                let info = content.get("info");
                let size = info.and_then(|i| i.get("size")).and_then(|v| v.as_u64());
                let duration = info
                    .and_then(|i| i.get("duration"))
                    .and_then(|v| v.as_u64());
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
            "m.location" => {
                let geo_uri = content
                    .get("geo_uri")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Self::parse_geo_uri(geo_uri).map(|(lat, lon)| ParsedContent {
                    content: MessageContent::Location {
                        latitude: lat,
                        longitude: lon,
                    },
                    encrypted_media: false,
                    file_encryption: None,
                })
            }
            _ => None,
        }
    }

    fn parse_media_content(
        content: &Value,
        body: &str,
        default_mime: &str,
    ) -> ParsedContent {
        let (url, encrypted_media, file_encryption) = Self::extract_media_url(content);
        let mimetype = Self::extract_mimetype(content, default_mime);
        let caption = Some(body.to_string()).filter(|s| !s.is_empty());
        let info = content.get("info");
        let width = info
            .and_then(|i| i.get("w"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let height = info
            .and_then(|i| i.get("h"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let size = info.and_then(|i| i.get("size")).and_then(|v| v.as_u64());
        let duration = info
            .and_then(|i| i.get("duration"))
            .and_then(|v| v.as_u64());
        let filename = content
            .get("filename")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mc = if default_mime.starts_with("video") {
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

    /// Parse a `geo:` URI into (latitude, longitude).
    ///
    /// Accepts formats like `geo:48.8566,2.3522` and `geo:48.8566,2.3522;u=10`.
    fn parse_geo_uri(uri: &str) -> Option<(f64, f64)> {
        let coords = uri.strip_prefix("geo:")?;
        // Strip optional parameters after `;`
        let coords = coords.split(';').next()?;
        let mut parts = coords.splitn(3, ',');
        let lat: f64 = parts.next()?.parse().ok()?;
        let lon: f64 = parts.next()?.parse().ok()?;
        // Third component (altitude) is intentionally ignored.
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return None;
        }
        Some((lat, lon))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── parse_geo_uri ────────────────────────────────────────────────────

    #[test]
    fn geo_uri_basic() {
        let (lat, lon) = Dispatcher::parse_geo_uri("geo:48.8566,2.3522").unwrap();
        assert!((lat - 48.8566).abs() < 1e-6);
        assert!((lon - 2.3522).abs() < 1e-6);
    }

    #[test]
    fn geo_uri_with_uncertainty() {
        let (lat, lon) = Dispatcher::parse_geo_uri("geo:37.7749,-122.4194;u=10").unwrap();
        assert!((lat - 37.7749).abs() < 1e-6);
        assert!((lon - (-122.4194)).abs() < 1e-6);
    }

    #[test]
    fn geo_uri_negative_coords() {
        let (lat, lon) = Dispatcher::parse_geo_uri("geo:-33.8688,151.2093").unwrap();
        assert!((lat - (-33.8688)).abs() < 1e-6);
        assert!((lon - 151.2093).abs() < 1e-6);
    }

    #[test]
    fn geo_uri_with_altitude() {
        let (lat, lon) = Dispatcher::parse_geo_uri("geo:48.8566,2.3522,35").unwrap();
        assert!((lat - 48.8566).abs() < 1e-6);
        assert!((lon - 2.3522).abs() < 1e-6);
    }

    #[test]
    fn geo_uri_invalid() {
        assert!(Dispatcher::parse_geo_uri("").is_none());
        assert!(Dispatcher::parse_geo_uri("not-a-geo-uri").is_none());
        assert!(Dispatcher::parse_geo_uri("geo:").is_none());
        assert!(Dispatcher::parse_geo_uri("geo:abc,def").is_none());
        assert!(Dispatcher::parse_geo_uri("geo:48.8566").is_none());
    }

    #[test]
    fn geo_uri_out_of_range() {
        assert!(Dispatcher::parse_geo_uri("geo:91.0,0.0").is_none());
        assert!(Dispatcher::parse_geo_uri("geo:0.0,181.0").is_none());
        assert!(Dispatcher::parse_geo_uri("geo:-91.0,0.0").is_none());
    }

    // ── parse_message_content: m.location ────────────────────────────────

    #[test]
    fn parse_location_content() {
        let content = json!({
            "msgtype": "m.location",
            "body": "Location",
            "geo_uri": "geo:48.8566,2.3522"
        });
        let parsed = Dispatcher::parse_message_content("m.location", "Location", &content).unwrap();
        match parsed.content {
            MessageContent::Location {
                latitude,
                longitude,
            } => {
                assert!((latitude - 48.8566).abs() < 1e-6);
                assert!((longitude - 2.3522).abs() < 1e-6);
            }
            other => panic!("expected Location, got {:?}", other),
        }
        assert!(!parsed.encrypted_media);
    }

    #[test]
    fn parse_location_bad_geo_uri_returns_none() {
        let content = json!({
            "msgtype": "m.location",
            "body": "Location",
            "geo_uri": "invalid"
        });
        assert!(Dispatcher::parse_message_content("m.location", "Location", &content).is_none());
    }

    #[test]
    fn parse_location_missing_geo_uri_returns_none() {
        let content = json!({
            "msgtype": "m.location",
            "body": "Location"
        });
        assert!(Dispatcher::parse_message_content("m.location", "Location", &content).is_none());
    }

    // ── parse_message_content: existing types still work ─────────────────

    #[test]
    fn parse_text_content() {
        let content = json!({
            "msgtype": "m.text",
            "body": "hello",
            "formatted_body": "<b>hello</b>"
        });
        let parsed = Dispatcher::parse_message_content("m.text", "hello", &content).unwrap();
        match parsed.content {
            MessageContent::Text {
                body,
                formatted_body,
            } => {
                assert_eq!(body, "hello");
                assert_eq!(formatted_body.as_deref(), Some("<b>hello</b>"));
            }
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn parse_unknown_msgtype_returns_none() {
        let content = json!({"msgtype": "m.custom", "body": "x"});
        assert!(Dispatcher::parse_message_content("m.custom", "x", &content).is_none());
    }
}

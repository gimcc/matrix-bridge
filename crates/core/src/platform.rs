/// Check if a string is a valid Matrix user ID (`@localpart:domain`).
///
/// Enforces the Matrix user ID grammar: localpart may contain `a-z A-Z 0-9 . _ - = + /`,
/// and the server name may contain `a-z A-Z 0-9 . - : [ ]`.
pub fn is_valid_matrix_user_id(user_id: &str) -> bool {
    let Some(rest) = user_id.strip_prefix('@') else {
        return false;
    };
    let Some((localpart, server)) = rest.split_once(':') else {
        return false;
    };
    !localpart.is_empty()
        && localpart
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-=+/".contains(&b))
        && !server.is_empty()
        && server
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
}

/// Build a puppet user localpart from prefix, platform, and external user ID.
///
/// Format: `{prefix}_{platform}_{user_id}` -> e.g. `bot_telegram_12345`
/// If the user_id contains invalid Matrix localpart characters, a SHA-256 hash
/// fallback is used automatically.
pub fn puppet_localpart(prefix: &str, platform: &str, user_id: &str) -> String {
    let safe_user_id = sanitize_for_localpart(user_id);
    format!("{prefix}_{platform}_{safe_user_id}")
}

/// Sanitize an input string for use in a Matrix user localpart.
///
/// Allowed characters: `a-z 0-9 . _ - = /`
/// If the input is empty, too long (>200), or contains disallowed characters,
/// returns a SHA-256 hash-based fallback: `h_{hex(sha256[:16])}` (128-bit).
fn sanitize_for_localpart(input: &str) -> String {
    let lowered = input.to_ascii_lowercase();
    let is_valid = !lowered.is_empty()
        && lowered.len() <= 200
        && lowered
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-=/".contains(&b));
    if is_valid {
        return lowered;
    }
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(input.as_bytes());
    format!("h_{}", hex_encode(&hash[..16]))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Sanitize an external ID (room or sender) by stripping control characters.
///
/// If the result is empty after sanitization, returns a SHA-256 hash-based
/// fallback: `h_{hex(sha256[:16])}` (128-bit).
pub fn sanitize_external_id(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_control())
        .take(255)
        .collect();
    if cleaned.is_empty() {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(input.as_bytes());
        format!("h_{}", hex_encode(&hash[..16]))
    } else {
        cleaned
    }
}

/// Extract the source platform from a puppet user's Matrix ID.
///
/// Given `@bot_telegram_12345:domain` with prefix `"bot"`, returns `Some("telegram")`.
/// Returns `None` for non-puppet users.
pub fn puppet_source_platform(sender: &str, prefix: &str) -> Option<String> {
    let localpart = sender
        .strip_prefix('@')
        .and_then(|s| s.split(':').next())
        .unwrap_or("");

    let rest = localpart.strip_prefix(prefix)?.strip_prefix('_')?;

    let pos = rest.find('_')?;
    let platform = &rest[..pos];
    let user_part = &rest[pos + 1..];

    if !platform.is_empty()
        && platform.bytes().all(|b| b.is_ascii_lowercase())
        && !user_part.is_empty()
    {
        Some(platform.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_matrix_user_id() {
        // Valid IDs
        assert!(is_valid_matrix_user_id("@alice:example.com"));
        assert!(is_valid_matrix_user_id("@bob:matrix.org"));
        assert!(is_valid_matrix_user_id(
            "@user-name.test:server.example.com"
        ));
        assert!(is_valid_matrix_user_id("@user+tag:example.com"));
        assert!(is_valid_matrix_user_id("@user=id:example.com"));
        assert!(is_valid_matrix_user_id("@user/path:example.com"));
        assert!(is_valid_matrix_user_id("@u:s"));
        // IPv6 server name with brackets
        assert!(is_valid_matrix_user_id("@user:[::1]:8448"));

        // Invalid IDs
        assert!(!is_valid_matrix_user_id("alice:example.com")); // missing @
        assert!(!is_valid_matrix_user_id("@aliceexample.com")); // missing :
        assert!(!is_valid_matrix_user_id("@:example.com")); // empty localpart
        assert!(!is_valid_matrix_user_id("@alice:")); // empty server
        assert!(!is_valid_matrix_user_id("@user name:example.com")); // space in localpart
        assert!(!is_valid_matrix_user_id("@user\x00id:example.com")); // control char
        assert!(!is_valid_matrix_user_id("@user\u{1F600}:example.com")); // emoji
        assert!(!is_valid_matrix_user_id("@alice:server name")); // space in server
        assert!(!is_valid_matrix_user_id("")); // empty string
        assert!(!is_valid_matrix_user_id("@")); // just @
        assert!(!is_valid_matrix_user_id("@:")); // empty localpart and server
    }

    #[test]
    fn test_puppet_localpart() {
        assert_eq!(
            puppet_localpart("bot", "telegram", "12345"),
            "bot_telegram_12345"
        );
        // uppercase gets lowered
        assert_eq!(puppet_localpart("bot", "slack", "u001"), "bot_slack_u001");
    }

    #[test]
    fn test_puppet_localpart_sanitization() {
        // special chars trigger hash fallback
        let result = puppet_localpart("bot", "telegram", "user@name!");
        assert!(result.starts_with("bot_telegram_h_"));
        assert_eq!(result.len(), "bot_telegram_h_".len() + 32);

        // empty user_id triggers hash fallback
        let result = puppet_localpart("bot", "telegram", "");
        assert!(result.starts_with("bot_telegram_h_"));

        // valid chars pass through lowered
        assert_eq!(
            puppet_localpart("bot", "discord", "abc.def-123"),
            "bot_discord_abc.def-123"
        );
    }

    #[test]
    fn test_sanitize_external_id() {
        // normal input passes through
        assert_eq!(sanitize_external_id("room-123"), "room-123");

        // control chars stripped
        assert_eq!(sanitize_external_id("room\x00\x01-123"), "room-123");

        // only control chars -> hash fallback
        let result = sanitize_external_id("\x00\x01\x02");
        assert!(result.starts_with("h_"));
        assert_eq!(result.len(), 2 + 32);

        // unicode preserved
        assert_eq!(sanitize_external_id("房间-42"), "房间-42");
    }

    #[test]
    fn test_puppet_source_platform() {
        assert_eq!(
            puppet_source_platform("@bot_telegram_12345:example.com", "bot"),
            Some("telegram".to_string())
        );
        assert_eq!(
            puppet_source_platform("@bot_slack_U001:example.com", "bot"),
            Some("slack".to_string())
        );
        assert_eq!(puppet_source_platform("@alice:example.com", "bot"), None);
        assert_eq!(
            puppet_source_platform("@other_telegram_12345:example.com", "bot"),
            None
        );
        assert_eq!(puppet_source_platform("@bot:example.com", "bot"), None);
    }
}

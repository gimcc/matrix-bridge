/// Build a puppet user localpart from prefix, platform, and external user ID.
///
/// Format: `{prefix}_{platform}_{user_id}` -> e.g. `bot_telegram_12345`
pub fn puppet_localpart(prefix: &str, platform: &str, user_id: &str) -> String {
    format!("{prefix}_{platform}_{user_id}")
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
    fn test_puppet_localpart() {
        assert_eq!(
            puppet_localpart("bot", "telegram", "12345"),
            "bot_telegram_12345"
        );
        assert_eq!(puppet_localpart("bot", "slack", "U001"), "bot_slack_U001");
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

use serde::Serialize;

use crate::config::AppserviceConfig;

#[derive(Debug, Serialize)]
pub struct Registration {
    pub id: String,
    pub url: String,
    pub as_token: String,
    pub hs_token: String,
    pub sender_localpart: String,
    pub namespaces: Namespaces,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocols: Option<Vec<String>>,
    /// MSC2409: receive to-device events via /transactions
    #[serde(
        rename = "de.sorunome.msc2409.push_ephemeral",
        skip_serializing_if = "Option::is_none"
    )]
    pub push_ephemeral: Option<bool>,
    /// MSC3202: receive device list changes and OTK counts via /transactions
    #[serde(
        rename = "de.sorunome.msc3202",
        skip_serializing_if = "Option::is_none"
    )]
    pub msc3202: Option<bool>,
    /// MSC4190: device management for appservices
    #[serde(rename = "org.matrix.msc4190", skip_serializing_if = "Option::is_none")]
    pub msc4190: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct Namespaces {
    pub users: Vec<NamespaceEntry>,
    pub aliases: Vec<NamespaceEntry>,
    pub rooms: Vec<NamespaceEntry>,
}

#[derive(Debug, Serialize)]
pub struct NamespaceEntry {
    pub exclusive: bool,
    pub regex: String,
}

/// Build a registration from appservice config and platform namespace regexes.
pub fn build_registration(
    config: &AppserviceConfig,
    user_regexes: Vec<String>,
    alias_regexes: Vec<String>,
    encryption_enabled: bool,
) -> Registration {
    let url = format!("http://bridge:{}", config.port);

    let mut users: Vec<NamespaceEntry> = user_regexes
        .into_iter()
        .map(|regex| NamespaceEntry {
            exclusive: true,
            regex,
        })
        .collect();

    // Always include the sender localpart as a managed user.
    users.push(NamespaceEntry {
        exclusive: true,
        regex: format!("@{}:.*", regex_escape(&config.sender_localpart)),
    });

    let aliases = alias_regexes
        .into_iter()
        .map(|regex| NamespaceEntry {
            exclusive: true,
            regex,
        })
        .collect();

    Registration {
        id: config.id.clone(),
        url,
        as_token: config.as_token.clone(),
        hs_token: config.hs_token.clone(),
        sender_localpart: config.sender_localpart.clone(),
        namespaces: Namespaces {
            users,
            aliases,
            rooms: vec![],
        },
        rate_limited: Some(false),
        protocols: None,
        push_ephemeral: if encryption_enabled { Some(true) } else { None },
        msc3202: if encryption_enabled { Some(true) } else { None },
        msc4190: if encryption_enabled { Some(true) } else { None },
    }
}

/// Generate the registration YAML string.
pub fn to_yaml(registration: &Registration) -> Result<String, serde_yaml::Error> {
    serde_yaml::to_string(registration)
}

fn regex_escape(s: &str) -> String {
    let special = [
        '.', '*', '+', '?', '(', ')', '[', ']', '{', '}', '\\', '^', '$', '|',
    ];
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        if special.contains(&c) {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppserviceConfig;

    #[test]
    fn test_build_registration() {
        let config = AppserviceConfig {
            id: "test-bridge".to_string(),
            address: "0.0.0.0".to_string(),
            port: 29320,
            sender_localpart: "bridge_bot".to_string(),
            as_token: "as_token_123".to_string(),
            hs_token: "hs_token_456".to_string(),
            puppet_prefix: "bot".to_string(),
            api_key: None,
            webhook_ssrf_protection: false,
        };

        let reg = build_registration(
            &config,
            vec!["@telegram_.*:example\\.com".to_string()],
            vec!["#telegram_.*:example\\.com".to_string()],
            false,
        );

        assert_eq!(reg.id, "test-bridge");
        assert_eq!(reg.namespaces.users.len(), 2); // telegram + sender
        assert_eq!(reg.namespaces.aliases.len(), 1);

        let yaml = to_yaml(&reg).unwrap();
        assert!(yaml.contains("as_token_123"));
        assert!(yaml.contains("bridge_bot"));
    }
}

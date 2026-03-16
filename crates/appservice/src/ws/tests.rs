use super::*;

/// Test helper: register with no limit (always succeeds).
impl WsRegistry {
    fn try_register_test(
        &self,
        platform_id: &str,
        forward_sources: Vec<String>,
    ) -> (String, tokio::sync::mpsc::Receiver<String>) {
        self.try_register(platform_id, forward_sources, vec![], usize::MAX)
            .expect("test registration should not fail")
    }
}

#[test]
fn test_register_and_unregister() {
    let registry = WsRegistry::new();
    let (id, _rx) = registry.try_register_test("telegram", vec![]);
    assert_eq!(registry.total_clients(), 1);

    registry.unregister("telegram", &id);
    assert_eq!(registry.total_clients(), 0);
}

#[test]
fn test_broadcast_delivers_to_matching_platform() {
    let registry = WsRegistry::new();
    let (_id1, mut rx1) = registry.try_register_test("telegram", vec!["*".to_string()]);
    let (_id2, mut rx2) = registry.try_register_test("telegram", vec!["*".to_string()]);
    let (_id3, mut rx3) = registry.try_register_test("slack", vec!["*".to_string()]);

    registry.broadcast("telegram", r#"{"event":"message"}"#, None);

    assert!(rx1.try_recv().is_ok());
    assert!(rx2.try_recv().is_ok());
    assert!(rx3.try_recv().is_err()); // slack client should not receive
}

#[test]
fn test_broadcast_forward_sources() {
    let registry = WsRegistry::new();
    // Client 1: only forwards "slack" -> should still receive "matrix" (always forwarded)
    let (_id1, mut rx1) = registry.try_register_test("telegram", vec!["slack".to_string()]);
    // Client 2: forwards all -> should receive "matrix"
    let (_id2, mut rx2) = registry.try_register_test("telegram", vec!["*".to_string()]);
    // Client 3: empty forward_sources -> should still receive "matrix" (always forwarded)
    let (_id3, mut rx3) = registry.try_register_test("telegram", vec![]);

    registry.broadcast("telegram", r#"{"event":"message"}"#, Some("matrix"));

    assert!(rx1.try_recv().is_ok()); // matrix always forwarded
    assert!(rx2.try_recv().is_ok()); // wildcard allows all
    assert!(rx3.try_recv().is_ok()); // matrix always forwarded

    // Non-matrix source respects forward_sources allowlist
    registry.broadcast("telegram", r#"{"event":"message"}"#, Some("discord"));
    // rx1 had "slack" only, not "discord"
    assert!(rx1.try_recv().is_err());
    // rx2 has wildcard
    assert!(rx2.try_recv().is_ok());
    // rx3 has empty = deny non-matrix
    assert!(rx3.try_recv().is_err());
}

#[test]
fn test_broadcast_no_source_defaults_to_matrix() {
    let registry = WsRegistry::new();
    let (_id1, mut rx1) = registry.try_register_test("telegram", vec!["matrix".to_string()]);
    let (_id2, mut rx2) = registry.try_register_test("telegram", vec!["slack".to_string()]);

    // No source_platform -> treated as "matrix", which is always forwarded.
    registry.broadcast("telegram", r#"{"event":"message"}"#, None);

    assert!(rx1.try_recv().is_ok()); // matrix always forwarded
    assert!(rx2.try_recv().is_ok()); // matrix always forwarded
}

#[test]
fn test_slow_consumer_does_not_block() {
    let registry = WsRegistry::new();
    let (_id, _rx) = registry.try_register_test("test", vec!["*".to_string()]);

    // Fill the channel beyond capacity — should not panic or block.
    for i in 0..CLIENT_CHANNEL_CAPACITY + 10 {
        registry.broadcast("test", &format!(r#"{{"n":{i}}}"#), None);
    }

    assert_eq!(registry.total_clients(), 1);
}

#[test]
fn test_closed_client_is_cleaned_up() {
    let registry = WsRegistry::new();
    let (id, rx) = registry.try_register_test("test", vec!["*".to_string()]);
    assert_eq!(registry.total_clients(), 1);

    // Drop the receiver to simulate a disconnected client.
    drop(rx);

    // Broadcast should detect the closed channel and remove the client.
    registry.broadcast("test", r#"{"event":"cleanup"}"#, None);
    assert_eq!(registry.total_clients(), 0);
    // Idempotent unregister should not panic.
    registry.unregister("test", &id);
}

#[test]
fn test_valid_platform_id() {
    assert!(is_valid_platform_id("telegram"));
    assert!(is_valid_platform_id("my-app_v2"));
    assert!(is_valid_platform_id("a.b.c"));
    assert!(!is_valid_platform_id(""));
    assert!(!is_valid_platform_id("has space"));
    assert!(!is_valid_platform_id("has/slash"));
    assert!(!is_valid_platform_id(&"x".repeat(65)));
}

#[test]
fn test_total_clients_atomic_counter() {
    let registry = WsRegistry::new();
    let (_id1, _rx1) = registry.try_register_test("a", vec![]);
    let (_id2, _rx2) = registry.try_register_test("b", vec![]);
    let (id3, _rx3) = registry.try_register_test("a", vec![]);
    assert_eq!(registry.total_clients(), 3);

    registry.unregister("a", &id3);
    assert_eq!(registry.total_clients(), 2);
}

#[test]
fn test_default_trait() {
    let registry = WsRegistry::default();
    assert_eq!(registry.total_clients(), 0);
}

#[test]
fn test_parse_forward_sources() {
    assert_eq!(parse_forward_sources(None), Vec::<String>::new());
    assert_eq!(parse_forward_sources(Some("")), Vec::<String>::new());
    assert_eq!(
        parse_forward_sources(Some("matrix, slack")),
        vec!["matrix", "slack"]
    );
    assert_eq!(parse_forward_sources(Some("*")), vec!["*"]);

    // Oversized entries are filtered out.
    let long = "x".repeat(MAX_FORWARD_SOURCE_LEN + 1);
    let input = format!("ok,,{long},valid");
    assert_eq!(parse_forward_sources(Some(&input)), vec!["ok", "valid"]);
}

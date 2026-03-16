use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::bridge_api::build_bridge_api_router;
use crate::server::{AppState, BridgeInfo};

/// Build a test AppState backed by an in-memory database.
async fn build_test_state() -> Arc<AppState> {
    let db = matrix_bridge_store::Database::open_in_memory().unwrap();
    db.migrate().await.unwrap();

    let matrix_client = crate::matrix_client::MatrixClient::new(
        "http://localhost:8008",
        "test_as_token",
        "example.com",
    )
    .expect("failed to build test HTTP client");

    let puppet_manager = Arc::new(crate::puppet_manager::PuppetManager::new(
        matrix_client.clone(),
        db.clone(),
        None,
    ));

    let ws_registry = Arc::new(crate::ws::WsRegistry::default());

    let dispatcher = crate::dispatcher::Dispatcher::new(
        puppet_manager,
        matrix_client,
        db,
        "example.com",
        "bridge",
        "bot",
        matrix_bridge_core::config::PermissionsConfig::default(),
        ws_registry.clone(),
        false,
        false,
        vec![],
    )
    .expect("failed to create test dispatcher");

    Arc::new(AppState {
        dispatcher: Arc::new(tokio::sync::RwLock::new(dispatcher)),
        processed_txns: tokio::sync::Mutex::new(indexmap::IndexSet::new()),
        crypto_pool: None,
        webhook_ssrf_protection: true,
        auto_invite: vec![],
        allow_api_invite: false,
        encryption_default: false,
        bridge_info: BridgeInfo {
            homeserver_url: "http://localhost:8008".to_string(),
            homeserver_domain: "example.com".to_string(),
            bot_user_id: "@bridge:example.com".to_string(),
            puppet_prefix: "bot".to_string(),
            encryption_enabled: false,
            encryption_default: false,
            webhook_ssrf_protection: true,
            api_key_required: false,
            configured_platforms: vec!["telegram".to_string()],
            admin_users: vec![],
            relay_users: vec![],
        },
        ws_registry,
        api_key: None,
    })
}

async fn build_test_app() -> (axum::Router, Arc<AppState>) {
    let state = build_test_state().await;
    let app = build_bridge_api_router().with_state(state.clone());
    (app, state)
}

async fn send_request(app: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::Null);
    (status, body)
}

fn json_post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

fn get_req(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn delete_req(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn handler_list_rooms_empty() {
    let (app, _) = build_test_app().await;
    let (status, body) = send_request(app, get_req("/api/v1/admin/rooms")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rooms"], json!([]));
}

#[tokio::test]
async fn handler_create_room_mapping_with_matrix_id() {
    let (app, _) = build_test_app().await;
    let req = json_post(
        "/api/v1/rooms",
        json!({ "platform": "telegram", "external_room_id": "chat_100", "matrix_room_id": "!test:example.com" }),
    );
    let (status, body) = send_request(app, req).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body["id"].is_number());
    assert_eq!(body["matrix_room_id"], "!test:example.com");
}

#[tokio::test]
async fn handler_create_room_mapping_idempotent() {
    let (app, state) = build_test_app().await;
    let body_json = json!({ "platform": "telegram", "external_room_id": "chat_200", "matrix_room_id": "!room200:example.com" });

    let (status1, body1) = send_request(app, json_post("/api/v1/rooms", body_json.clone())).await;
    assert_eq!(status1, StatusCode::CREATED);

    let app2 = build_bridge_api_router().with_state(state);
    let (status2, body2) = send_request(app2, json_post("/api/v1/rooms", body_json)).await;
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(body1["id"], body2["id"]);
}

#[tokio::test]
async fn handler_delete_room_mapping() {
    let (app, state) = build_test_app().await;
    let req = json_post(
        "/api/v1/rooms",
        json!({ "platform": "telegram", "external_room_id": "chat_300", "matrix_room_id": "!room300:example.com" }),
    );
    let (_, body) = send_request(app, req).await;
    let id = body["id"].as_i64().unwrap();

    let app2 = build_bridge_api_router().with_state(state.clone());
    let (status, body) = send_request(app2, delete_req(&format!("/api/v1/rooms/{id}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], true);

    let app3 = build_bridge_api_router().with_state(state);
    let (status, _) = send_request(app3, delete_req(&format!("/api/v1/rooms/{id}"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn handler_list_rooms_filtered_by_platform() {
    let (app, state) = build_test_app().await;
    send_request(app, json_post("/api/v1/rooms", json!({ "platform": "telegram", "external_room_id": "tg_1", "matrix_room_id": "!tg1:example.com" }))).await;

    let app2 = build_bridge_api_router().with_state(state.clone());
    send_request(app2, json_post("/api/v1/rooms", json!({ "platform": "discord", "external_room_id": "dc_1", "matrix_room_id": "!dc1:example.com" }))).await;

    let app3 = build_bridge_api_router().with_state(state.clone());
    let (status, body) = send_request(app3, get_req("/api/v1/admin/rooms?platform=telegram")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rooms"].as_array().unwrap().len(), 1);

    let app4 = build_bridge_api_router().with_state(state);
    let (status, body) = send_request(app4, get_req("/api/v1/admin/rooms")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rooms"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn handler_create_webhook_valid() {
    let (app, _) = build_test_app().await;
    let req = json_post(
        "/api/v1/webhooks",
        json!({ "platform": "telegram", "url": "https://hooks.example.com/tg" }),
    );
    let (status, body) = send_request(app, req).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body["id"].is_number());
}

#[tokio::test]
async fn handler_create_webhook_ssrf_blocked() {
    let (app, _) = build_test_app().await;
    let req = json_post(
        "/api/v1/webhooks",
        json!({ "platform": "telegram", "url": "http://127.0.0.1/evil" }),
    );
    let (status, body) = send_request(app, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("blocked"));
}

#[tokio::test]
async fn handler_create_webhook_invalid_scheme() {
    let (app, _) = build_test_app().await;
    let req = json_post(
        "/api/v1/webhooks",
        json!({ "platform": "telegram", "url": "ftp://files.example.com/hook" }),
    );
    let (status, body) = send_request(app, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("unsupported scheme")
    );
}

#[tokio::test]
async fn handler_list_webhooks_empty() {
    let (app, _) = build_test_app().await;
    let (status, body) = send_request(app, get_req("/api/v1/admin/webhooks")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["webhooks"], json!([]));
}

#[tokio::test]
async fn handler_webhook_crud_cycle() {
    let (app, state) = build_test_app().await;

    let (status, body) = send_request(app, json_post("/api/v1/webhooks", json!({ "platform": "discord", "url": "https://hooks.example.com/discord", "forward_sources": ["*"] }))).await;
    assert_eq!(status, StatusCode::CREATED);
    let wh_id = body["id"].as_i64().unwrap();

    let app2 = build_bridge_api_router().with_state(state.clone());
    let (status, body) =
        send_request(app2, get_req("/api/v1/admin/webhooks?platform=discord")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["webhooks"].as_array().unwrap().len(), 1);

    let app3 = build_bridge_api_router().with_state(state.clone());
    let (status, _) = send_request(app3, delete_req(&format!("/api/v1/webhooks/{wh_id}"))).await;
    assert_eq!(status, StatusCode::OK);

    let app4 = build_bridge_api_router().with_state(state);
    let (status, body) =
        send_request(app4, get_req("/api/v1/admin/webhooks?platform=discord")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["webhooks"], json!([]));
}

#[tokio::test]
async fn handler_delete_webhook_not_found() {
    let (app, _) = build_test_app().await;
    let (status, body) = send_request(app, delete_req("/api/v1/webhooks/99999")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not found");
}

#[tokio::test]
async fn handler_server_info() {
    let (app, _) = build_test_app().await;
    let (status, body) = send_request(app, get_req("/api/v1/admin/info")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["homeserver"]["domain"], "example.com");
    assert_eq!(body["bot"]["user_id"], "@bridge:example.com");
    assert_eq!(body["features"]["encryption_enabled"], false);
    assert!(body["stats"]["room_mappings"].is_number());
}

#[tokio::test]
async fn handler_list_puppets_empty() {
    let (app, _) = build_test_app().await;
    let (status, body) = send_request(app, get_req("/api/v1/admin/puppets")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["puppets"], json!([]));
}

#[tokio::test]
async fn handler_list_messages_empty() {
    let (app, _) = build_test_app().await;
    let (status, body) = send_request(app, get_req("/api/v1/admin/messages")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["messages"], json!([]));
}

#[tokio::test]
async fn handler_crypto_status_disabled() {
    let (app, _) = build_test_app().await;
    let (status, body) = send_request(app, get_req("/api/v1/admin/crypto")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], false);
    assert_eq!(body["per_user_crypto"], false);
}

#[tokio::test]
async fn handler_room_name_too_long() {
    let (app, _) = build_test_app().await;
    let long_name = "x".repeat(256);
    let req = json_post(
        "/api/v1/rooms",
        json!({ "platform": "telegram", "external_room_id": "ch_long", "room_name": long_name, "matrix_room_id": "!long:example.com" }),
    );
    let (status, body) = send_request(app, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("room_name exceeds 255")
    );
}

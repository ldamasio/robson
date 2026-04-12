//! API contract tests for the Robson daemon HTTP API.
//!
//! These tests verify the shape and status codes of every REST endpoint using
//! an in-memory stub daemon (no PostgreSQL, no Binance API required).
//!
//! Run with: `cargo test -p robsond api_contract`
//!
//! # What is verified
//!
//! - Correct HTTP status codes for happy paths and key error cases.
//! - Response bodies deserialize correctly into the expected types.
//! - No regressions in the public API contract visible to the CLI and frontend.
//!
//! # What is NOT verified
//!
//! - Behavior that requires a real exchange (order execution, fills).
//! - Behavior that requires PostgreSQL (projection, event log queries).
//! - SSE stream content (only the connection is checked).

use reqwest::Client;
use robsond::{Config, Daemon, api};
use serde_json::{Value, json};
use std::net::SocketAddr;
use uuid::Uuid;

// =============================================================================
// Test Harness
// =============================================================================

/// Spin up an in-memory stub daemon and return its base URL.
///
/// The server binds to port 0 (OS-assigned) so tests never conflict.
async fn start_test_server() -> (String, SocketAddr) {
    let config = Config::test();
    let daemon = Daemon::new_stub(config);
    let addr = daemon.start_api_server(None).await.expect("failed to start test server");
    let base_url = format!("http://{}", addr);
    (base_url, addr)
}

fn client() -> Client {
    Client::builder().timeout(std::time::Duration::from_secs(5)).build().unwrap()
}

// =============================================================================
// Health / Readiness
// =============================================================================

#[tokio::test]
async fn test_healthz_returns_ok() {
    let (base, _) = start_test_server().await;
    let resp = client().get(format!("{}/healthz", base)).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    let body: api::HealthResponse = resp.json().await.unwrap();
    assert_eq!(body.status, "ok");
    assert!(!body.version.is_empty());
}

#[tokio::test]
async fn test_health_returns_healthy() {
    let (base, _) = start_test_server().await;
    let resp = client().get(format!("{}/health", base)).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    let body: api::HealthResponse = resp.json().await.unwrap();
    assert_eq!(body.status, "healthy");
}

#[tokio::test]
async fn test_readyz_returns_ready_in_stub_mode() {
    let (base, _) = start_test_server().await;
    let resp = client().get(format!("{}/readyz", base)).send().await.unwrap();

    // Stub mode: no real DB, no Binance — both checks default to ok
    assert_eq!(resp.status(), 200);
    let body: api::ReadinessResponse = resp.json().await.unwrap();
    assert_eq!(body.status, "ready");
    assert_eq!(body.checks.database, "ok");
    assert_eq!(body.checks.binance_api, "ok");
    assert!(!body.timestamp.is_empty());
}

// =============================================================================
// Status
// =============================================================================

#[tokio::test]
async fn test_status_empty_on_fresh_daemon() {
    let (base, _) = start_test_server().await;
    let resp = client().get(format!("{}/status", base)).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    let body: api::StatusResponse = resp.json().await.unwrap();
    assert_eq!(body.active_positions, 0);
    assert!(body.positions.is_empty());
    assert!(body.pending_approvals.is_empty());
}

// =============================================================================
// Arm / Disarm
// =============================================================================

async fn arm_btcusdt(base: &str) -> api::ArmResponse {
    let resp = client()
        .post(format!("{}/positions", base))
        .json(&json!({
            "symbol": "BTCUSDT",
            "side": "LONG",
            "capital": "10000"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201, "arm should return 201 Created");
    resp.json::<api::ArmResponse>().await.unwrap()
}

#[tokio::test]
async fn test_arm_position_returns_created() {
    let (base, _) = start_test_server().await;
    let arm = arm_btcusdt(&base).await;

    assert!(!arm.position_id.is_nil());
    assert_eq!(arm.symbol, "BTCUSDT");
    assert_eq!(arm.side, "Long");
    // State is Armed immediately after arm
    assert!(arm.state.contains("Armed"), "state was: {}", arm.state);
}

#[tokio::test]
async fn test_status_shows_armed_position() {
    let (base, _) = start_test_server().await;
    let arm = arm_btcusdt(&base).await;

    let resp = client().get(format!("{}/status", base)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: api::StatusResponse = resp.json().await.unwrap();

    assert_eq!(body.active_positions, 1);
    assert_eq!(body.positions[0].id, arm.position_id);
    assert_eq!(body.positions[0].symbol, "BTCUSDT");
}

#[tokio::test]
async fn test_get_position_by_id() {
    let (base, _) = start_test_server().await;
    let arm = arm_btcusdt(&base).await;

    let resp = client()
        .get(format!("{}/positions/{}", base, arm.position_id))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: api::PositionSummary = resp.json().await.unwrap();
    assert_eq!(body.id, arm.position_id);
    assert_eq!(body.symbol, "BTCUSDT");
}

#[tokio::test]
async fn test_get_position_not_found() {
    let (base, _) = start_test_server().await;
    let unknown_id = Uuid::now_v7();

    let resp = client().get(format!("{}/positions/{}", base, unknown_id)).send().await.unwrap();

    assert_eq!(resp.status(), 404);
    let body: api::ErrorResponse = resp.json().await.unwrap();
    assert!(body.error.contains(&unknown_id.to_string()));
}

#[tokio::test]
async fn test_disarm_position() {
    let (base, _) = start_test_server().await;
    let arm = arm_btcusdt(&base).await;

    let resp = client()
        .delete(format!("{}/positions/{}", base, arm.position_id))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 204, "disarm should return 204 No Content");

    // Position should be gone from status
    let status: api::StatusResponse = client()
        .get(format!("{}/status", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status.active_positions, 0);
}

#[tokio::test]
async fn test_disarm_nonexistent_returns_error() {
    let (base, _) = start_test_server().await;
    let unknown_id = Uuid::now_v7();

    let resp = client()
        .delete(format!("{}/positions/{}", base, unknown_id))
        .send()
        .await
        .unwrap();

    assert!(
        resp.status().is_client_error(),
        "disarming non-existent position should return 4xx, got: {}",
        resp.status()
    );
}

// =============================================================================
// Arm — validation errors
// =============================================================================

#[tokio::test]
async fn test_arm_invalid_side_returns_400() {
    let (base, _) = start_test_server().await;

    let resp = client()
        .post(format!("{}/positions", base))
        .json(&json!({
            "symbol": "BTCUSDT",
            "side": "sideways",
            "capital": "10000"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let body: api::ErrorResponse = resp.json().await.unwrap();
    assert!(body.error.to_lowercase().contains("side"), "error: {}", body.error);
}

#[tokio::test]
async fn test_arm_invalid_symbol_returns_400() {
    let (base, _) = start_test_server().await;

    let resp = client()
        .post(format!("{}/positions", base))
        .json(&json!({
            "symbol": "",
            "side": "LONG",
            "capital": "10000"
        }))
        .send()
        .await
        .unwrap();

    assert!(
        resp.status().is_client_error(),
        "empty symbol should fail, got: {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_arm_missing_required_field_returns_422() {
    let (base, _) = start_test_server().await;

    // Missing capital
    let resp = client()
        .post(format!("{}/positions", base))
        .json(&json!({ "symbol": "BTCUSDT", "side": "LONG" }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        422,
        "missing required fields should return 422 Unprocessable Entity"
    );
}

// =============================================================================
// Signal injection
// =============================================================================

#[tokio::test]
async fn test_signal_on_armed_position_is_accepted() {
    let (base, _) = start_test_server().await;
    let arm = arm_btcusdt(&base).await;

    let resp = client()
        .post(format!("{}/positions/{}/signal", base, arm.position_id))
        .json(&json!({
            "position_id": arm.position_id,
            "entry_price": "95000",
            "stop_loss": "93500"
        }))
        .send()
        .await
        .unwrap();

    // Signal may result in 200 (accepted) or 200 with state change
    assert_eq!(resp.status(), 200, "signal injection should return 200, got: {}", resp.status());
}

#[tokio::test]
async fn test_signal_on_nonexistent_position_returns_404() {
    let (base, _) = start_test_server().await;
    let unknown_id = Uuid::now_v7();

    let resp = client()
        .post(format!("{}/positions/{}/signal", base, unknown_id))
        .json(&json!({
            "position_id": unknown_id,
            "entry_price": "95000",
            "stop_loss": "93500"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
}

// =============================================================================
// Panic
// =============================================================================

#[tokio::test]
async fn test_panic_with_no_positions() {
    let (base, _) = start_test_server().await;

    let resp = client().post(format!("{}/panic", base)).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    let body: api::PanicResponse = resp.json().await.unwrap();
    assert_eq!(body.count, 0);
    assert!(body.closed_positions.is_empty());
}

#[tokio::test]
async fn test_panic_closes_armed_positions() {
    let (base, _) = start_test_server().await;

    // Arm two positions
    arm_btcusdt(&base).await;
    let resp = client()
        .post(format!("{}/positions", base))
        .json(&json!({
            "symbol": "ETHUSDT",
            "side": "SHORT",
            "capital": "5000"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Panic should close both
    let panic_resp = client().post(format!("{}/panic", base)).send().await.unwrap();
    assert_eq!(panic_resp.status(), 200);
    let body: api::PanicResponse = panic_resp.json().await.unwrap();
    assert_eq!(body.count, 2, "panic should have closed 2 positions");
    assert_eq!(body.closed_positions.len(), 2);

    // Status should be empty
    let status: api::StatusResponse = client()
        .get(format!("{}/status", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status.active_positions, 0);
}

// =============================================================================
// Query approval (MIG-v2.5#4 — QE-P3)
// =============================================================================

#[tokio::test]
async fn test_approve_nonexistent_query_returns_404() {
    let (base, _) = start_test_server().await;
    let unknown_id = Uuid::now_v7();

    let resp = client()
        .post(format!("{}/queries/{}/approve", base, unknown_id))
        .send()
        .await
        .unwrap();

    // No query with this ID — should be not found or bad request
    assert!(
        resp.status().is_client_error(),
        "approving non-existent query should return 4xx, got: {}",
        resp.status()
    );
}

// =============================================================================
// Safety net
// =============================================================================

#[tokio::test]
async fn test_safety_status_returns_disabled_in_stub_mode() {
    let (base, _) = start_test_server().await;

    let resp = client().get(format!("{}/safety/status", base)).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    let body: api::SafetyStatusResponse = resp.json().await.unwrap();
    // Stub mode: position monitor not configured → enabled=false
    assert!(!body.enabled, "safety net should be disabled in stub mode");
    assert!(body.tracked_positions.is_empty());
}

// =============================================================================
// SSE events stream
// =============================================================================

#[tokio::test]
async fn test_events_endpoint_establishes_connection() {
    let (base, _) = start_test_server().await;

    // Just verify the endpoint exists and opens an SSE connection.
    // We don't wait for events — just check the response headers.
    let resp = client()
        .get(format!("{}/events", base))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let content_type = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(
        content_type.contains("text/event-stream"),
        "events endpoint should return SSE content-type, got: {}",
        content_type
    );
}

// =============================================================================
// Response shape (JSON field presence)
// =============================================================================

#[tokio::test]
async fn test_arm_response_has_required_fields() {
    let (base, _) = start_test_server().await;

    let resp = client()
        .post(format!("{}/positions", base))
        .json(&json!({
            "symbol": "BTCUSDT",
            "side": "LONG",
            "capital": "10000"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();

    // Contract: these fields must always be present
    assert!(body["position_id"].is_string(), "position_id missing");
    assert!(body["symbol"].is_string(), "symbol missing");
    assert!(body["side"].is_string(), "side missing");
    assert!(body["state"].is_string(), "state missing");
}

#[tokio::test]
async fn test_status_response_has_required_fields() {
    let (base, _) = start_test_server().await;

    let resp = client().get(format!("{}/status", base)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Contract: these fields must always be present
    assert!(body["active_positions"].is_number(), "active_positions missing");
    assert!(body["positions"].is_array(), "positions missing");
    assert!(body["pending_approvals"].is_array(), "pending_approvals missing");
}

#[tokio::test]
async fn test_panic_response_has_required_fields() {
    let (base, _) = start_test_server().await;

    let resp = client().post(format!("{}/panic", base)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert!(body["count"].is_number(), "count missing");
    assert!(body["closed_positions"].is_array(), "closed_positions missing");
}

// =============================================================================
// MonthlyHalt (v3 policy: binary, 4% monthly drawdown)
// =============================================================================

#[tokio::test]
async fn test_monthly_halt_status_is_active_on_fresh_daemon() {
    let (base, _) = start_test_server().await;

    let resp = client().get(format!("{}/monthly-halt", base)).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert_eq!(body["state"].as_str().unwrap(), "active");
    assert_eq!(body["blocks_new_entries"].as_bool().unwrap(), false);
    assert_eq!(body["blocks_signals"].as_bool().unwrap(), false);
    assert!(body["description"].is_string(), "description missing");
}

#[tokio::test]
async fn test_monthly_halt_status_has_required_fields() {
    let (base, _) = start_test_server().await;

    let resp = client().get(format!("{}/monthly-halt", base)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert!(body["state"].is_string(), "state missing");
    assert!(body["description"].is_string(), "description missing");
    assert!(body["blocks_new_entries"].is_boolean(), "blocks_new_entries missing");
    assert!(body["blocks_signals"].is_boolean(), "blocks_signals missing");
}

#[tokio::test]
async fn test_monthly_halt_trigger_transitions_to_halted() {
    let (base, _) = start_test_server().await;

    let resp = client()
        .post(format!("{}/monthly-halt", base))
        .json(&json!({ "reason": "operator test" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert_eq!(body["state"].as_str().unwrap(), "monthly_halt");
    assert_eq!(body["blocks_new_entries"].as_bool().unwrap(), true);
    assert_eq!(body["blocks_signals"].as_bool().unwrap(), true);
    assert_eq!(body["reason"].as_str().unwrap(), "operator test");
}

#[tokio::test]
async fn test_arm_blocked_when_monthly_halt_active() {
    let (base, _) = start_test_server().await;

    // Trigger MonthlyHalt
    client()
        .post(format!("{}/monthly-halt", base))
        .json(&json!({ "reason": "4% monthly limit hit" }))
        .send()
        .await
        .unwrap();

    // Arming should now be blocked
    let resp = client()
        .post(format!("{}/positions", base))
        .json(&json!({
            "symbol": "BTCUSDT",
            "side": "LONG",
            "capital": "10000"
        }))
        .send()
        .await
        .unwrap();

    // 503 Service Unavailable — MonthlyHalt active
    assert_eq!(resp.status(), 503, "arm should be blocked during MonthlyHalt, got: {}", resp.status());
    let body: Value = resp.json().await.unwrap();
    assert!(body["error"].is_string(), "error field missing");
}

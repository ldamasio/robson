//! HTTP API for the Robson daemon.
//!
//! Provides REST endpoints for:
//! - Health check
//! - Status (active positions)
//! - Arm position
//! - Cancel/close a single position
//! - Panic (emergency close all)
//! - Safety net (rogue position monitoring)
//! - SSE events for operator-facing runtime updates

use std::{convert::Infallible, sync::Arc, time::Duration};

use async_stream::stream;
use axum::{
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{
        sse::{KeepAlive, Sse},
        IntoResponse,
    },
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Datelike;
use robson_domain::{
    ApprovalPolicy as DomainApprovalPolicy, DetectorSignal, EntryPolicy, EntryPolicyConfig,
    Position, PositionState, Price, RiskConfig, Side, Symbol, TradingPolicy,
};
use robson_exec::ExchangePort;
#[cfg(feature = "postgres")]
use robson_store::find_positions_overlapping_month;
use robson_store::Store;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::warn;
use uuid::Uuid;

use crate::{
    circuit_breaker::{CircuitBreaker, HaltState, MonthlyHaltSnapshot},
    config::FundingConfig,
    error::DaemonError,
    event_bus::EventBus,
    funding::{CapitalRefreshResponse, ExecuteFundingRequest},
    position_manager::PositionManager,
    position_monitor::PositionMonitor,
    sse::{map_daemon_event, resync_required_event},
};

// =============================================================================
// API State
// =============================================================================

/// Shared state for API handlers.
pub struct ApiState<E: ExchangePort + 'static, S: Store + 'static> {
    pub exchange: Arc<E>,
    pub position_manager: Arc<RwLock<PositionManager<E, S>>>,
    pub event_bus: Arc<EventBus>,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub position_monitor: Option<Arc<PositionMonitor>>,
    /// PostgreSQL pool for liveness check. Present only when DATABASE_URL is
    /// configured.
    #[cfg(feature = "postgres")]
    pub pg_pool: Option<std::sync::Arc<sqlx::PgPool>>,
    #[cfg(feature = "postgres")]
    pub tenant_id: Option<Uuid>,
    /// Bearer token for authenticating mutating routes. `None` means auth is
    /// disabled (non-production environments only).
    pub api_token: Option<String>,
    pub funding: FundingConfig,
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Health check response.
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

/// Readiness check response.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReadinessResponse {
    pub status: String,
    pub checks: ReadinessChecks,
    pub timestamp: String,
}

/// Individual readiness checks.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReadinessChecks {
    pub database: String,
    pub binance_api: String,
}

/// Status response.
///
/// `active_positions` is a historical field name. It currently counts open core
/// positions returned by `/status` (Armed, Entering, Active, Exiting).
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub active_positions: usize,
    pub positions: Vec<PositionSummary>,
    pub pending_approvals: Vec<PendingApprovalSummary>,
    /// Slots available for new entries under the current monthly risk budget.
    pub new_slots_available: u32,
    /// Slots currently occupied by open core positions.
    pub occupied_slots: usize,
    /// Total cells the UI should render: occupied slots plus new slots.
    pub slot_cells_total: usize,
    /// Absolute realized loss for the current month. Winning trades do not
    /// offset this value.
    pub monthly_realized_loss: Decimal,
    /// Current-month realized loss as a percentage of capital_base.
    pub monthly_realized_loss_pct: Decimal,
    /// Starting capital basis for the current month.
    pub capital_base: Decimal,
}

/// Historical monthly positions response.
#[derive(Debug, Serialize, Deserialize)]
pub struct MonthlyPositionsResponse {
    pub month: String,
    pub positions: Vec<PositionSummary>,
    pub occupied_slots: usize,
    pub slot_cells_total: usize,
}

/// Summary of a position.
#[derive(Debug, Serialize, Deserialize)]
pub struct PositionSummary {
    pub id: Uuid,
    pub symbol: String,
    pub side: String,
    pub state: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_price: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailing_stop: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_price: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pnl: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variation_pct: Option<Decimal>,
}

/// Summary of a pending approval query for REST bootstrap.
#[derive(Debug, Serialize, Deserialize)]
pub struct PendingApprovalSummary {
    pub query_id: Uuid,
    pub position_id: Option<Uuid>,
    pub state: String,
    pub reason: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
struct MonthQuery {
    month: Option<String>,
}

/// Entry policy sub-object for `ArmRequest`.
///
/// Both fields default so the entire `entry_policy` object and each field are
/// optional in JSON. Omitting the object is equivalent to
/// `{ "mode": "confirmed_trend", "approval": "automatic" }`.
///
/// Valid `mode` values:
/// - `"confirmed_trend"` (default) — SMA crossover confirmation strategy.
/// - `"confirmed_reversal"` — reversal pattern confirmation strategy.
/// - `"confirmed_key_level"` — key level confirmation strategy.
/// - `"immediate"` — no strategy; system stop + risk only.
///
/// Valid `approval` values:
/// - `"automatic"` (default) — execution proceeds without human review.
/// - `"human_confirmation"` — execution always waits for operator approval.
#[derive(Debug, Deserialize)]
pub struct ArmEntryPolicyRequest {
    #[serde(default)]
    pub mode: EntryPolicy,
    #[serde(default)]
    pub approval: DomainApprovalPolicy,
}

impl Default for ArmEntryPolicyRequest {
    fn default() -> Self {
        Self {
            mode: EntryPolicy::default(),
            approval: DomainApprovalPolicy::default(),
        }
    }
}

/// Request to arm a new position.
///
/// Note: risk per trade is fixed at 1% by v3 policy. Not configurable via API.
///
/// Omitting `entry_policy` is equivalent to:
/// ```json
/// { "mode": "confirmed_trend", "approval": "automatic" }
/// ```
/// See `ArmEntryPolicyRequest` for valid mode and approval values.
#[derive(Debug, Deserialize)]
pub struct ArmRequest {
    pub symbol: String,
    pub side: String,
    #[serde(default = "default_account_id")]
    pub account_id: Uuid,
    #[serde(default)]
    pub entry_policy: ArmEntryPolicyRequest,
}

fn default_account_id() -> Uuid {
    Uuid::nil()
}

/// Response after arming a position.
#[derive(Debug, Serialize, Deserialize)]
pub struct ArmResponse {
    pub position_id: Uuid,
    pub symbol: String,
    pub side: String,
    pub state: String,
}

/// Request to inject a signal (for testing).
#[derive(Debug, Deserialize)]
pub struct SignalRequest {
    pub position_id: Uuid,
    pub entry_price: Decimal,
    pub stop_loss: Decimal,
}

/// Panic response.
#[derive(Debug, Serialize, Deserialize)]
pub struct PanicResponse {
    pub closed_positions: Vec<Uuid>,
    pub count: usize,
}

/// Response after approving a query.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApproveQueryResponse {
    pub query_id: Uuid,
    pub state: String,
}

/// MonthlyHalt status response — returned by GET and trigger endpoints.
///
/// `state` serializes as snake_case: `"active"` | `"monthly_halt"`.
#[derive(Debug, Serialize, Deserialize)]
pub struct MonthlyHaltStatusResponse {
    /// Current state (snake_case: "active" | "monthly_halt").
    pub state: HaltState,
    pub description: &'static str,
    pub reason: Option<String>,
    pub triggered_at: Option<chrono::DateTime<chrono::Utc>>,
    pub blocks_new_entries: bool,
    pub blocks_signals: bool,
}

impl From<MonthlyHaltSnapshot> for MonthlyHaltStatusResponse {
    fn from(s: MonthlyHaltSnapshot) -> Self {
        Self {
            state: s.state,
            description: s.description,
            reason: s.reason,
            triggered_at: s.triggered_at,
            blocks_new_entries: s.blocks_new_entries,
            blocks_signals: s.blocks_signals,
        }
    }
}

/// Request body for `POST /monthly-halt` (conservative operator trigger).
#[derive(Debug, Deserialize)]
pub struct MonthlyHaltTriggerRequest {
    /// Human-readable reason for triggering MonthlyHalt.
    pub reason: String,
}

/// Error response.
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

// =============================================================================
// Reconcile Close Types (Slice 5B1)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ReconcileCloseRequest {
    pub position_id: Uuid,
    pub evidence: robson_domain::ReconciliationEvidence,
}

#[derive(Debug, Serialize)]
pub struct ReconcileCloseSuccessResponse {
    pub status: String,
    pub position_id: Uuid,
    pub realized_pnl: String,
    pub exit_price: String,
    pub closure_evidence: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ReconcileCloseErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_state: Option<String>,
}

// =============================================================================
// Safety Net Types
// =============================================================================

/// Safety net status response.
#[derive(Debug, Serialize, Deserialize)]
pub struct SafetyStatusResponse {
    /// Whether the safety net is enabled
    pub enabled: bool,
    /// Symbols being monitored
    pub symbols: Vec<String>,
    /// Polling interval in seconds
    pub poll_interval_secs: u64,
    /// Currently tracked rogue positions
    pub tracked_positions: Vec<DetectedPositionSummary>,
    /// Number of execution attempts (failed)
    pub pending_executions: usize,
}

/// Summary of a detected rogue position.
#[derive(Debug, Serialize, Deserialize)]
pub struct DetectedPositionSummary {
    /// Position ID (symbol:side)
    pub id: String,
    /// Trading symbol
    pub symbol: String,
    /// Position side
    pub side: String,
    /// Entry price
    pub entry_price: Decimal,
    /// Quantity
    pub quantity: Decimal,
    /// Calculated stop price
    pub stop_price: Decimal,
    /// Stop distance percentage
    pub stop_distance_pct: Decimal,
    /// When position was first detected
    pub detected_at: String,
}

/// Request to enable/disable safety net.
#[derive(Debug, Deserialize)]
pub struct SafetyEnableRequest {
    /// Whether to enable or disable
    pub enabled: bool,
}

/// Safety net test response.
#[derive(Debug, Serialize)]
pub struct SafetyTestResponse {
    /// Whether the test was successful
    pub success: bool,
    /// Message describing the result
    pub message: String,
    /// Current positions from Binance (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub positions: Option<Vec<BinancePositionInfo>>,
}

/// Info about a Binance position (for testing).
#[derive(Debug, Serialize)]
pub struct BinancePositionInfo {
    pub symbol: String,
    pub side: String,
    pub quantity: Decimal,
    pub entry_price: Decimal,
    pub calculated_stop: Decimal,
}

// =============================================================================
// Router
// =============================================================================

/// Create the API router.
///
/// Read-only routes are mounted without authentication.
/// Mutating routes are wrapped in a bearer-token auth middleware.
pub fn create_router<E, S>(state: Arc<ApiState<E, S>>) -> Router
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    // Read-only routes — no auth required
    let read_only = Router::new()
        // Kubernetes health probes
        .route("/healthz", get(health_liveness))
        .route("/readyz", get(health_readiness))
        // Standard read-only endpoints
        .route("/health", get(health_handler))
        .route("/events", get(events_handler))
        .route("/status", get(status_handler))
        .route("/positions", get(month_positions_handler))
        .route("/positions/:id", get(get_position_handler))
        // Prometheus metrics
        .route("/metrics", get(metrics_handler))
        // Safety net read-only endpoints
        .route("/safety/status", get(safety_status_handler))
        .route("/safety/test", get(safety_test_handler))
        // MonthlyHalt status (read-only)
        .route("/monthly-halt", get(monthly_halt_status_handler))
        .with_state(state.clone());

    // Mutating routes — bearer token required
    let token = state.api_token.clone();
    let auth_layer = axum::middleware::from_fn(move |req: Request, next: Next| {
        let expected = token.clone();
        async move {
            // No token configured — auth disabled
            let Some(expected) = expected else {
                return next.run(req).await;
            };

            // Extract Authorization header
            let auth_header =
                req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok());

            match auth_header {
                Some(value) if value.starts_with("Bearer ") => {
                    let provided = &value[7..];
                    if provided == expected {
                        next.run(req).await
                    } else {
                        (
                            StatusCode::UNAUTHORIZED,
                            Json(ErrorResponse {
                                error: "Invalid bearer token".to_string(),
                            }),
                        )
                            .into_response()
                    }
                },
                _ => (
                    StatusCode::UNAUTHORIZED,
                    Json(ErrorResponse {
                        error: "Missing or invalid Authorization header".to_string(),
                    }),
                )
                    .into_response(),
            }
        }
    });
    let mutating = Router::new()
        .route("/positions", post(arm_handler))
        .route("/positions/:id", delete(cancel_or_close_handler))
        .route("/positions/:id/signal", post(signal_handler))
        .route("/queries/:id/approve", post(approve_query_handler))
        .route("/panic", post(panic_handler))
        // MonthlyHalt trigger (mutating)
        .route("/monthly-halt", post(monthly_halt_trigger_handler))
        // Manual reconciliation close (Slice 5B1)
        .route("/reconcile-close", post(reconcile_close_handler))
        .route("/funding/quote", post(funding_quote_handler::<E, S>))
        .route("/funding/execute", post(funding_execute_handler::<E, S>))
        .route("/funding/:id", get(funding_get_handler::<E, S>))
        .route("/funding", get(funding_list_handler::<E, S>))
        .route("/capital/refresh", post(capital_refresh_handler::<E, S>))
        .layer(auth_layer)
        .with_state(state);

    read_only.merge(mutating).layer(build_cors_layer())
}

// =============================================================================
// Handlers
// =============================================================================

/// Build a CORS layer from the `ROBSON_CORS_ALLOWED_ORIGINS` env var.
/// Comma-separated origin URLs. Empty/unset → no CORS headers (preserves test
/// behavior).
fn build_cors_layer() -> CorsLayer {
    let raw = std::env::var("ROBSON_CORS_ALLOWED_ORIGINS").unwrap_or_default();
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| HeaderValue::from_str(s).ok())
        .collect();
    if origins.is_empty() {
        return CorsLayer::new();
    }
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

/// Prometheus metrics endpoint.
///
/// Returns all registered metrics in the Prometheus exposition format.
/// Unauthenticated (read-only).
async fn metrics_handler() -> impl IntoResponse {
    let body = crate::metrics::render();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

#[cfg(feature = "postgres")]
fn funding_service<E, S>(
    state: &ApiState<E, S>,
) -> Result<crate::funding::FundingService<E, S>, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let (Some(pool), Some(tenant_id)) = (&state.pg_pool, state.tenant_id) else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "funding_store_unavailable".to_string(),
            }),
        ));
    };
    Ok(crate::funding::FundingService::new(
        pool.clone(),
        tenant_id,
        state.exchange.clone(),
        state.position_manager.clone(),
        state.funding.clone(),
    ))
}

#[cfg(feature = "postgres")]
async fn funding_quote_handler<E, S>(State(state): State<Arc<ApiState<E, S>>>) -> impl IntoResponse
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    match funding_service(&state) {
        Ok(service) => match service.quote().await {
            Ok(quote) => (StatusCode::OK, Json(quote)).into_response(),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: error.to_string() }),
            )
                .into_response(),
        },
        Err(response) => response.into_response(),
    }
}

#[cfg(not(feature = "postgres"))]
async fn funding_quote_handler<E, S>(State(_state): State<Arc<ApiState<E, S>>>) -> impl IntoResponse
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: "funding_store_unavailable".to_string(),
        }),
    )
}

#[cfg(feature = "postgres")]
async fn funding_execute_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    headers: HeaderMap,
    Json(request): Json<ExecuteFundingRequest>,
) -> impl IntoResponse
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    if !state.funding.enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "funding_disabled" })),
        )
            .into_response();
    }
    let Some(idempotency_key) =
        headers.get("Idempotency-Key").and_then(|value| value.to_str().ok())
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "missing_idempotency_key".to_string(),
            }),
        )
            .into_response();
    };

    match funding_service(&state) {
        Ok(service) => match service.execute(request.quote_id, idempotency_key).await {
            Ok(response) => (StatusCode::OK, Json(response)).into_response(),
            Err(error) if error.to_string().contains("quote_expired") => (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse { error: "quote_expired".to_string() }),
            )
                .into_response(),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: error.to_string() }),
            )
                .into_response(),
        },
        Err(response) => response.into_response(),
    }
}

#[cfg(not(feature = "postgres"))]
async fn funding_execute_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    _headers: HeaderMap,
    Json(_request): Json<ExecuteFundingRequest>,
) -> impl IntoResponse
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    if !state.funding.enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "funding_disabled" })),
        )
            .into_response();
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "error": "funding_store_unavailable" })),
    )
        .into_response()
}

#[cfg(feature = "postgres")]
async fn funding_get_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    match funding_service(&state) {
        Ok(service) => match service.get(id).await {
            Ok(view) => (StatusCode::OK, Json(view)).into_response(),
            Err(error) => (StatusCode::NOT_FOUND, Json(ErrorResponse { error: error.to_string() }))
                .into_response(),
        },
        Err(response) => response.into_response(),
    }
}

#[cfg(not(feature = "postgres"))]
async fn funding_get_handler<E, S>(
    State(_state): State<Arc<ApiState<E, S>>>,
    Path(_id): Path<Uuid>,
) -> impl IntoResponse
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: "funding_store_unavailable".to_string(),
        }),
    )
}

#[cfg(feature = "postgres")]
async fn funding_list_handler<E, S>(State(state): State<Arc<ApiState<E, S>>>) -> impl IntoResponse
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    match funding_service(&state) {
        Ok(service) => match service.list().await {
            Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: error.to_string() }),
            )
                .into_response(),
        },
        Err(response) => response.into_response(),
    }
}

#[cfg(not(feature = "postgres"))]
async fn funding_list_handler<E, S>(State(_state): State<Arc<ApiState<E, S>>>) -> impl IntoResponse
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: "funding_store_unavailable".to_string(),
        }),
    )
}

async fn capital_refresh_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
) -> impl IntoResponse
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    match refresh_capital_from_exchange(&state.exchange, &state.position_manager).await {
        Ok(capital) => (StatusCode::OK, Json(CapitalRefreshResponse { capital })).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: error.to_string() }),
        )
            .into_response(),
    }
}

pub(crate) async fn refresh_capital_from_exchange<E, S>(
    exchange: &Arc<E>,
    position_manager: &Arc<RwLock<PositionManager<E, S>>>,
) -> Result<Decimal, DaemonError>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let balance = exchange.get_futures_balance().await?;
    let capital = balance.wallet_balance;
    if capital <= Decimal::ZERO {
        return Err(DaemonError::Config(format!(
            "Exchange reports zero or negative wallet balance: {capital}"
        )));
    }
    let manager = position_manager.read().await;
    manager.update_engine_capital(capital);
    Ok(capital)
}

/// Liveness probe for Kubernetes - checks if the process is alive.
///
/// This endpoint always returns 200 OK if the process is running.
/// Used by Kubernetes to determine if the pod should be restarted.
async fn health_liveness() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// Readiness probe for Kubernetes - checks if the service is ready to accept
/// traffic.
///
/// Checks:
/// - Database connectivity (via store)
/// - Binance API reachability
///
/// Returns 200 OK if all checks pass, 503 Service Unavailable otherwise.
async fn health_readiness<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
) -> Result<Json<ReadinessResponse>, (StatusCode, Json<ReadinessResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let database_ok;

    // Check PostgreSQL connectivity with a real ping when pool is configured.
    // Falls back to MemoryStore check (always OK) when Postgres is not wired.
    #[cfg(feature = "postgres")]
    {
        if let Some(pool) = &state.pg_pool {
            database_ok = sqlx::query("SELECT 1").execute(pool.as_ref()).await.is_ok();
        } else {
            // No PG configured: readiness passes (foundation mode without DB)
            database_ok = true;
        }
    }
    #[cfg(not(feature = "postgres"))]
    {
        database_ok = true;
    }

    // Check Binance API reachability via position monitor
    // (Safety Net uses Binance REST client which can ping the API)
    // For now, we'll mark it as OK if position monitor is configured
    // TODO: Add actual ping check via BinanceRestClient
    let binance_ok = true;

    let checks = ReadinessChecks {
        database: if database_ok {
            "ok".to_string()
        } else {
            "failed".to_string()
        },
        binance_api: if binance_ok {
            "ok".to_string()
        } else {
            "failed".to_string()
        },
    };

    let response = ReadinessResponse {
        status: if database_ok && binance_ok {
            "ready".to_string()
        } else {
            "not_ready".to_string()
        },
        checks,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    if database_ok && binance_ok {
        Ok(Json(response))
    } else {
        Err((StatusCode::SERVICE_UNAVAILABLE, Json(response)))
    }
}

/// Health check endpoint.
async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// Stream public operator events over Server-Sent Events.
///
/// Clients should bootstrap state via REST and use this stream only for
/// incremental updates. `event_id` is emitted for uniqueness and client-side
/// deduplication, but v2.5 does not implement replay or `Last-Event-ID` resume.
async fn events_handler<E, S>(State(state): State<Arc<ApiState<E, S>>>) -> impl IntoResponse
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let mut receiver = state.event_bus.subscribe();

    let event_stream = stream! {
        loop {
            match receiver.recv().await {
                Some(Ok(event)) => {
                    if let Some(public_event) = map_daemon_event(&event) {
                        yield Ok::<_, Infallible>(public_event.into_sse_event());
                    }
                }
                Some(Err(lag_message)) => {
                    warn!(message = %lag_message, "SSE receiver lagged; operator client must re-sync");
                    yield Ok::<_, Infallible>(
                        resync_required_event("lagged", lag_message).into_sse_event()
                    );
                    break;
                }
                None => break,
            }
        }
    };

    let sse = Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("heartbeat"));

    (
        [
            (header::CACHE_CONTROL, header::HeaderValue::from_static("no-cache")),
            (
                header::HeaderName::from_static("x-accel-buffering"),
                header::HeaderValue::from_static("no"),
            ),
        ],
        sse,
    )
}

/// Get status (all open core positions).
async fn status_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
) -> Result<Json<StatusResponse>, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let manager = state.position_manager.read().await;
    let positions = manager.get_open_positions().await.map_err(|e| to_error_response(e))?;
    let pending_approvals = manager.get_pending_approvals().await;
    let monthly = manager
        .load_monthly_state(chrono::Utc::now())
        .await
        .map_err(|e| to_error_response(e))?;
    let new_slots_available =
        manager.compute_slots_available().await.map_err(|e| to_error_response(e))?;
    let occupied_slots = positions.len();
    let slot_cells_total = occupied_slots.saturating_add(new_slots_available as usize);
    let monthly_realized_loss_pct = if monthly.capital_base > Decimal::ZERO {
        monthly.realized_loss / monthly.capital_base * Decimal::from(100u32)
    } else {
        Decimal::ZERO
    };

    let mut summaries: Vec<PositionSummary> = Vec::with_capacity(positions.len());
    for position in &positions {
        summaries.push(position_to_summary_with_live_price(&manager, position).await);
    }

    // Update active positions gauge
    crate::metrics::ACTIVE_POSITIONS.set(summaries.len() as f64);
    let pending_summaries: Vec<PendingApprovalSummary> = pending_approvals
        .into_iter()
        .filter_map(|query| {
            let approval = query.approval?;
            Some(PendingApprovalSummary {
                query_id: query.id,
                position_id: query.position_id,
                state: format!("{:?}", query.state),
                reason: approval.reason,
                expires_at: approval.expires_at,
            })
        })
        .collect();

    Ok(Json(StatusResponse {
        active_positions: summaries.len(),
        positions: summaries,
        pending_approvals: pending_summaries,
        new_slots_available,
        occupied_slots,
        slot_cells_total,
        monthly_realized_loss: monthly.realized_loss,
        monthly_realized_loss_pct,
        capital_base: monthly.capital_base,
    }))
}

/// Get positions that were alive during a given month.
async fn month_positions_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    Query(query): Query<MonthQuery>,
) -> Result<Json<MonthlyPositionsResponse>, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let requested = parse_month_query(query.month.as_deref())?;
    let manager = state.position_manager.read().await;
    let positions = load_positions_for_month(&manager, requested, &state)
        .await
        .map_err(|e| to_error_response(e))?;
    let monthly = manager.load_monthly_state(requested).await.map_err(|e| to_error_response(e))?;
    let occupied_slots = positions.len();
    let inherited_slots =
        positions.iter().filter(|position| position.created_at < requested).count();
    let base_slots_available = TradingPolicy::default().slots_available(
        monthly.capital_base,
        monthly.realized_loss,
        Decimal::ZERO,
    ) as usize;
    let slot_cells_total = base_slots_available.saturating_add(inherited_slots);

    Ok(Json(MonthlyPositionsResponse {
        month: format!("{:04}-{:02}", requested.year(), requested.month()),
        positions,
        occupied_slots,
        slot_cells_total,
    }))
}

/// Get a single position.
async fn get_position_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    Path(id): Path<Uuid>,
) -> Result<Json<PositionSummary>, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let manager = state.position_manager.read().await;
    let position =
        manager
            .get_position(id)
            .await
            .map_err(|e| to_error_response(e))?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("Position not found: {}", id),
                    }),
                )
            })?;

    Ok(Json(position_to_summary_with_live_price(&manager, &position).await))
}

/// Arm a new position.
async fn arm_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    Json(req): Json<ArmRequest>,
) -> Result<(StatusCode, Json<ArmResponse>), (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    // Parse symbol
    let symbol = Symbol::from_pair(&req.symbol).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse { error: format!("Invalid symbol: {}", e) }),
        )
    })?;

    // Parse side
    let side = match req.side.to_uppercase().as_str() {
        "LONG" | "BUY" => Side::Long,
        "SHORT" | "SELL" => Side::Short,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Invalid side: {}. Expected: LONG or SHORT", req.side),
                }),
            ));
        },
    };

    // Capital base for position sizing.
    //
    // Source: `monthly_state` table, populated by `MonthBoundaryReset` event.
    // Falls back to engine startup value when DB is unavailable.
    //
    // Policy (ADR-0024 §6, v3-risk-engine-spec §Month Boundary Rule):
    //   capital_base = current_equity − latent_risk_carried
    //
    // The capital base is pessimistic: it assumes every inherited open position
    // will hit its current stop. Inherited risk is absorbed into the base
    // rather than carried as debt against the new month's budget. If a carried
    // position's trailing stop advances (winning trade), the freed risk
    // increments the capital base of the following month — never the current
    // one. This guarantees every month starts with 4 available slots.
    //
    // The caller never supplies capital. Position sizing is derived:
    //   position_size = (capital_base × 1%) / technical_stop_distance
    let capital = {
        let manager = state.position_manager.read().await;
        manager.load_capital_base_for_month(chrono::Utc::now()).await
    };
    let capital = capital.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to load capital base: {}", e),
            }),
        )
    })?;
    let risk_config = RiskConfig::new(capital).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Capital base invalid: {}", e),
            }),
        )
    })?;

    // Build entry policy from request (defaults to ConfirmedTrend + Automatic)
    let entry_policy = EntryPolicyConfig::new(req.entry_policy.mode, req.entry_policy.approval);

    // Arm position
    let manager = state.position_manager.write().await;
    let position = manager
        .arm_position_with_policy(
            symbol.clone(),
            side,
            risk_config,
            None,
            req.account_id,
            entry_policy,
        )
        .await
        .map_err(|e| to_error_response(e))?;

    Ok((
        StatusCode::CREATED,
        Json(ArmResponse {
            position_id: position.id,
            symbol: symbol.as_pair(),
            side: format!("{:?}", side),
            state: format!("{:?}", position.state),
        }),
    ))
}

/// Cancel an Armed position or close an Active position.
async fn cancel_or_close_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let manager = state.position_manager.write().await;
    manager.cancel_or_close_position(id).await.map_err(|e| to_error_response(e))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Inject a signal (for testing).
async fn signal_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    Path(id): Path<Uuid>,
    Json(req): Json<SignalRequest>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let manager = state.position_manager.write().await;

    // Load position to get symbol and side
    let position =
        manager
            .get_position(id)
            .await
            .map_err(|e| to_error_response(e))?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("Position not found: {}", id),
                    }),
                )
            })?;

    // Create signal
    let signal = DetectorSignal {
        signal_id: Uuid::now_v7(),
        position_id: id,
        symbol: position.symbol.clone(),
        side: position.side,
        entry_price: Price::new(req.entry_price).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Invalid entry price: {}", e),
                }),
            )
        })?,
        stop_loss: Price::new(req.stop_loss).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Invalid stop loss: {}", e),
                }),
            )
        })?,
        technical_stop_analysis: None,
        timestamp: chrono::Utc::now(),
    };

    manager.handle_signal(signal).await.map_err(|e| to_error_response(e))?;

    Ok(StatusCode::OK)
}

/// Emergency close all positions.
async fn panic_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
) -> Result<Json<PanicResponse>, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let manager = state.position_manager.write().await;
    let closed = manager.panic_close_all().await.map_err(|e| to_error_response(e))?;

    Ok(Json(PanicResponse {
        count: closed.len(),
        closed_positions: closed,
    }))
}

/// Approve a pending query and resume execution.
async fn approve_query_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApproveQueryResponse>, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let manager = state.position_manager.write().await;
    let query = manager.approve_query(id).await.map_err(|e| to_error_response(e))?;

    Ok(Json(ApproveQueryResponse {
        query_id: query.id,
        state: format!("{:?}", query.state),
    }))
}

/// `POST /reconcile-close` — operator-driven manual reconciliation close.
///
/// Accepts real evidence (`OrderFillRecord` or `UserTradeRecord`) and closes
/// a stale-Active position. Rejects `AccountSnapshot` and `Estimated` in
/// Slice 5B1.
async fn reconcile_close_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    Json(req): Json<ReconcileCloseRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    use robson_domain::ReconciliationEvidence;

    // Reject unsupported evidence types before touching position manager.
    match &req.evidence {
        ReconciliationEvidence::AccountSnapshot(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::to_value(ReconcileCloseErrorResponse {
                        error: "unsupported_evidence".to_string(),
                        details: Some(
                            "account_snapshot evidence is not supported in Slice 5B1".to_string(),
                        ),
                        position_id: None,
                        current_state: None,
                    })
                    .unwrap(),
                ),
            ));
        },
        ReconciliationEvidence::Estimated(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::to_value(ReconcileCloseErrorResponse {
                        error: "unsupported_evidence".to_string(),
                        details: Some(
                            "estimated evidence is not supported in Slice 5B1".to_string(),
                        ),
                        position_id: None,
                        current_state: None,
                    })
                    .unwrap(),
                ),
            ));
        },
        _ => {},
    }

    if let Err(details) = validate_reconcile_close_evidence(&req.evidence) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::to_value(ReconcileCloseErrorResponse {
                    error: "invalid_evidence".to_string(),
                    details: Some(details),
                    position_id: Some(req.position_id),
                    current_state: None,
                })
                .unwrap(),
            ),
        ));
    }

    // Build ReconciledCloseInput from the request evidence.
    let (exit_price, filled_quantity, fee, fee_asset, closed_at) = match &req.evidence {
        ReconciliationEvidence::OrderFillRecord(e) => {
            (e.fill_price, e.filled_quantity, e.fee, e.fee_asset.clone(), e.filled_at)
        },
        ReconciliationEvidence::UserTradeRecord(e) => {
            (e.fill_price, e.filled_quantity, e.fee, e.fee_asset.clone(), e.filled_at)
        },
        _ => unreachable!("guarded above"),
    };

    let closure_evidence_for_response =
        robson_domain::ClosureEvidence::Reconciled(req.evidence.clone());

    let input = crate::position_manager::ReconciledCloseInput {
        position_id: req.position_id,
        exit_price,
        filled_quantity,
        fee,
        fee_asset,
        closed_at,
        evidence: req.evidence,
    };

    let manager = state.position_manager.write().await;
    let outcome = manager.reconcile_close(input).await.map_err(|e| match e {
        DaemonError::PositionNotFound(id) => (
            StatusCode::NOT_FOUND,
            Json(
                serde_json::to_value(ReconcileCloseErrorResponse {
                    error: "position_not_found".to_string(),
                    details: None,
                    position_id: Some(id),
                    current_state: None,
                })
                .unwrap(),
            ),
        ),
        other => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": other.to_string() })),
        ),
    })?;

    match outcome {
        crate::position_manager::ReconcileCloseOutcome::Closed => {
            // Reload position to get the realized PnL from state.
            let position = manager.get_position(req.position_id).await.ok().flatten();
            let (realized_pnl, exit_price_val, closure_evidence) = match &position {
                Some(p) => match &p.state {
                    PositionState::Closed { realized_pnl, exit_price: ep, .. } => {
                        let ce = serde_json::to_value(&closure_evidence_for_response).unwrap();
                        (*realized_pnl, ep.as_decimal(), ce)
                    },
                    _ => (
                        rust_decimal::Decimal::ZERO,
                        rust_decimal::Decimal::ZERO,
                        serde_json::json!({}),
                    ),
                },
                None => (
                    rust_decimal::Decimal::ZERO,
                    rust_decimal::Decimal::ZERO,
                    serde_json::json!({}),
                ),
            };
            let resp = ReconcileCloseSuccessResponse {
                status: "closed".to_string(),
                position_id: req.position_id,
                realized_pnl: realized_pnl.to_string(),
                exit_price: exit_price_val.to_string(),
                closure_evidence,
            };
            Ok((StatusCode::OK, Json(serde_json::to_value(resp).unwrap())))
        },
        crate::position_manager::ReconcileCloseOutcome::AlreadyTerminal => {
            // Reload to report current state.
            let current_state = manager
                .get_position(req.position_id)
                .await
                .ok()
                .flatten()
                .map(|p| p.state.name().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            Err((
                StatusCode::CONFLICT,
                Json(
                    serde_json::to_value(ReconcileCloseErrorResponse {
                        error: "position_not_active".to_string(),
                        details: Some("Position is already terminal".to_string()),
                        position_id: Some(req.position_id),
                        current_state: Some(current_state),
                    })
                    .unwrap(),
                ),
            ))
        },
        crate::position_manager::ReconcileCloseOutcome::SkippedNonActive { state } => Err((
            StatusCode::CONFLICT,
            Json(
                serde_json::to_value(ReconcileCloseErrorResponse {
                    error: "position_not_active".to_string(),
                    details: Some(format!("Position is in {} state, expected Active", state)),
                    position_id: Some(req.position_id),
                    current_state: Some(state),
                })
                .unwrap(),
            ),
        )),
        crate::position_manager::ReconcileCloseOutcome::RejectedUnsupportedEvidence { source } => {
            Err((
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::to_value(ReconcileCloseErrorResponse {
                        error: "unsupported_evidence".to_string(),
                        details: Some(format!("{} evidence is not supported in Slice 5B1", source)),
                        position_id: None,
                        current_state: None,
                    })
                    .unwrap(),
                ),
            ))
        },
        crate::position_manager::ReconcileCloseOutcome::RejectedInconsistentEvidence { field } => {
            Err((
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::to_value(ReconcileCloseErrorResponse {
                        error: "inconsistent_evidence".to_string(),
                        details: Some(format!("field={} mismatches evidence", field)),
                        position_id: Some(req.position_id),
                        current_state: None,
                    })
                    .unwrap(),
                ),
            ))
        },
    }
}

fn validate_reconcile_close_evidence(
    evidence: &robson_domain::ReconciliationEvidence,
) -> Result<(), String> {
    use robson_domain::ReconciliationEvidence;

    match evidence {
        ReconciliationEvidence::OrderFillRecord(e) => validate_reconcile_close_fields(
            e.fill_price.as_decimal(),
            e.filled_quantity.as_decimal(),
            e.fee,
            &e.fee_asset,
            &e.exchange_order_id,
            None,
        ),
        ReconciliationEvidence::UserTradeRecord(e) => validate_reconcile_close_fields(
            e.fill_price.as_decimal(),
            e.filled_quantity.as_decimal(),
            e.fee,
            &e.fee_asset,
            &e.exchange_order_id,
            Some(&e.exchange_trade_id),
        ),
        ReconciliationEvidence::AccountSnapshot(_) | ReconciliationEvidence::Estimated(_) => Ok(()),
    }
}

fn validate_reconcile_close_fields(
    fill_price: Decimal,
    filled_quantity: Decimal,
    fee: Decimal,
    fee_asset: &str,
    exchange_order_id: &str,
    exchange_trade_id: Option<&str>,
) -> Result<(), String> {
    if fill_price <= Decimal::ZERO {
        return Err("fill_price must be > 0".to_string());
    }
    if filled_quantity <= Decimal::ZERO {
        return Err("filled_quantity must be > 0".to_string());
    }
    if fee < Decimal::ZERO {
        return Err("fee must be >= 0".to_string());
    }
    if fee_asset.is_empty() {
        return Err("fee_asset must not be empty".to_string());
    }
    if exchange_order_id.is_empty() {
        return Err("exchange_order_id must not be empty".to_string());
    }
    if matches!(exchange_trade_id, Some("")) {
        return Err("exchange_trade_id must not be empty".to_string());
    }
    Ok(())
}

// =============================================================================
// Safety Net Handlers
// =============================================================================

/// Get safety net status.
async fn safety_status_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
) -> Result<Json<SafetyStatusResponse>, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    match &state.position_monitor {
        Some(monitor) => {
            let tracked = monitor.get_tracked_positions().await;
            let attempts_count = monitor.get_pending_execution_count().await;

            let summaries: Vec<DetectedPositionSummary> = tracked
                .iter()
                .map(|pos| {
                    let stop = pos.calculated_stop.as_ref();
                    DetectedPositionSummary {
                        id: format!("{}:{}", pos.symbol.as_pair(), pos.side),
                        symbol: pos.symbol.as_pair(),
                        side: format!("{:?}", pos.side),
                        entry_price: pos.entry_price.as_decimal(),
                        quantity: pos.quantity.as_decimal(),
                        stop_price: stop.map(|s| s.stop_price.as_decimal()).unwrap_or_default(),
                        stop_distance_pct: stop.map(|s| s.distance_pct).unwrap_or_default(),
                        detected_at: pos.detected_at.to_rfc3339(),
                    }
                })
                .collect();

            Ok(Json(SafetyStatusResponse {
                enabled: true,
                symbols: vec!["BTCUSDT".to_string()], // TODO: Get from config
                poll_interval_secs: 20,               // TODO: Get from config
                tracked_positions: summaries,
                pending_executions: attempts_count,
            }))
        },
        None => Ok(Json(SafetyStatusResponse {
            enabled: false,
            symbols: vec![],
            poll_interval_secs: 0,
            tracked_positions: vec![],
            pending_executions: 0,
        })),
    }
}

/// Test safety net (dry run).
async fn safety_test_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
) -> Result<Json<SafetyTestResponse>, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    match &state.position_monitor {
        Some(_monitor) => {
            // TODO: Actually test the Binance connection and show positions
            // For now, return a simple success message
            Ok(Json(SafetyTestResponse {
                success: true,
                message:
                    "Safety net is running. Use 'robson safety-status' to see tracked positions."
                        .to_string(),
                positions: None,
            }))
        },
        None => Ok(Json(SafetyTestResponse {
            success: false,
            message: "Safety net is not enabled.".to_string(),
            positions: None,
        })),
    }
}

// =============================================================================
// Helpers
// =============================================================================
// MonthlyHalt handlers (v3 policy)
// =============================================================================

/// GET /monthly-halt — current MonthlyHalt status.
async fn monthly_halt_status_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
) -> Json<MonthlyHaltStatusResponse>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let snap = state.circuit_breaker.snapshot().await;
    Json(snap.into())
}

/// POST /monthly-halt — operator-initiated conservative MonthlyHalt trigger.
///
/// Idempotent: if already in MonthlyHalt, returns current status without
/// mutating state or emitting an event. Closes all open positions on
/// transition.
///
/// Note: there is no reset endpoint. MonthlyHalt persists until next calendar
/// month. Allowing a reset without explicit policy evidence is not permitted.
async fn monthly_halt_trigger_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    Json(req): Json<MonthlyHaltTriggerRequest>,
) -> Result<Json<MonthlyHaltStatusResponse>, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let manager = state.position_manager.write().await;
    manager
        .trigger_monthly_halt(req.reason)
        .await
        .map_err(|e| to_error_response(e))?;

    let snap = state.circuit_breaker.snapshot().await;
    Ok(Json(snap.into()))
}

// =============================================================================

fn to_error_response(error: DaemonError) -> (StatusCode, Json<ErrorResponse>) {
    let status = match &error {
        DaemonError::PositionNotFound(_) => StatusCode::NOT_FOUND,
        DaemonError::QueryNotFound(_) => StatusCode::NOT_FOUND,
        DaemonError::InvalidPositionState { .. } => StatusCode::CONFLICT,
        DaemonError::ApprovalExpired(_) => StatusCode::GONE,
        DaemonError::ApprovalDenied { .. } => StatusCode::CONFLICT,
        DaemonError::MonthlyHaltActive { .. } => StatusCode::SERVICE_UNAVAILABLE,
        DaemonError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };

    (status, Json(ErrorResponse { error: error.to_string() }))
}

async fn position_to_summary_with_live_price<E, S>(
    manager: &PositionManager<E, S>,
    position: &Position,
) -> PositionSummary
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let live_price = match &position.state {
        PositionState::Active { .. } | PositionState::Exiting { .. } => {
            match manager.get_market_price(&position.symbol).await {
                Ok(price) => Some(price),
                Err(error) => {
                    warn!(
                        position_id = %position.id,
                        symbol = %position.symbol,
                        %error,
                        "failed to fetch live market price for position summary"
                    );
                    None
                },
            }
        },
        _ => None,
    };

    let entry_policy = manager.entry_policy_for_position(position.id).await;
    let entry_mode = Some(
        match entry_policy.mode {
            robson_domain::EntryPolicy::Immediate => "immediate",
            robson_domain::EntryPolicy::ConfirmedTrend => "confirmed_trend",
            robson_domain::EntryPolicy::ConfirmedReversal => "confirmed_reversal",
            robson_domain::EntryPolicy::ConfirmedKeyLevel => "confirmed_key_level",
        }
        .to_string(),
    );
    let approval_mode = Some(
        match entry_policy.approval {
            robson_domain::ApprovalPolicy::Automatic => "automatic",
            robson_domain::ApprovalPolicy::HumanConfirmation => "human_confirmation",
        }
        .to_string(),
    );

    position_to_summary(position, live_price, entry_mode, approval_mode)
}

fn position_to_summary(
    position: &Position,
    live_price: Option<Price>,
    entry_mode: Option<String>,
    approval_mode: Option<String>,
) -> PositionSummary {
    let (state_str, entry_price, trailing_stop, current_price, pnl, variation_pct) = match &position
        .state
    {
        PositionState::Armed => ("Armed".to_string(), None, None, None, None, None),
        PositionState::Entering { expected_entry, .. } => (
            "Entering".to_string(),
            Some(expected_entry.as_decimal()),
            None,
            None,
            None,
            None,
        ),
        PositionState::Active { current_price, trailing_stop, .. } => {
            let observed_price = live_price.unwrap_or(*current_price);
            let valuation_price = stop_trigger_price(position.side, observed_price, *trailing_stop)
                .unwrap_or(observed_price);
            (
                "Active".to_string(),
                position.entry_price.map(|p| p.as_decimal()),
                Some(trailing_stop.as_decimal()),
                Some(observed_price.as_decimal()),
                pnl_at_price(position, valuation_price),
                variation_pct_at_price(position, valuation_price),
            )
        },
        PositionState::Exiting { .. } => {
            let current_price = live_price;
            (
                "Exiting".to_string(),
                position.entry_price.map(|p| p.as_decimal()),
                None,
                current_price.map(|p| p.as_decimal()),
                current_price.and_then(|p| pnl_at_price(position, p)),
                current_price.and_then(|p| variation_pct_at_price(position, p)),
            )
        },
        PositionState::Closed { exit_price, exit_reason, .. } => {
            let realized_pnl = if let PositionState::Closed { realized_pnl, .. } = &position.state {
                *realized_pnl
            } else {
                rust_decimal::Decimal::ZERO
            };
            (
                format!("Closed ({:?})", exit_reason),
                position.entry_price.map(|p| p.as_decimal()),
                Some(exit_price.as_decimal()),
                Some(exit_price.as_decimal()),
                Some(realized_pnl),
                variation_pct_at_price(position, *exit_price),
            )
        },
        PositionState::Error { error, .. } => {
            (format!("Error: {}", error), None, None, None, None, None)
        },
        PositionState::Cancelled => ("Cancelled".to_string(), None, None, None, None, None),
    };

    PositionSummary {
        id: position.id,
        symbol: position.symbol.as_pair(),
        side: format!("{:?}", position.side),
        state: state_str,
        created_at: position.created_at,
        entry_mode,
        approval_mode,
        quantity: if position.quantity.as_decimal() > Decimal::ZERO {
            Some(position.quantity.as_decimal())
        } else {
            None
        },
        entry_price,
        trailing_stop,
        current_price,
        pnl,
        variation_pct,
    }
}

fn pnl_at_price(position: &Position, current_price: Price) -> Option<Decimal> {
    let entry_price = position.entry_price?.as_decimal();
    let quantity = position.quantity.as_decimal();
    Some(match position.side {
        Side::Long => (current_price.as_decimal() - entry_price) * quantity,
        Side::Short => (entry_price - current_price.as_decimal()) * quantity,
    })
}

fn variation_pct_at_price(position: &Position, current_price: Price) -> Option<Decimal> {
    let entry_price = position.entry_price?.as_decimal();
    if entry_price <= Decimal::ZERO {
        return None;
    }

    let diff = match position.side {
        Side::Long => current_price.as_decimal() - entry_price,
        Side::Short => entry_price - current_price.as_decimal(),
    };
    Some((diff / entry_price) * Decimal::new(100, 0))
}

fn stop_trigger_price(side: Side, current_price: Price, trailing_stop: Price) -> Option<Price> {
    let is_hit = match side {
        Side::Long => current_price.as_decimal() <= trailing_stop.as_decimal(),
        Side::Short => current_price.as_decimal() >= trailing_stop.as_decimal(),
    };

    is_hit.then_some(trailing_stop)
}

fn parse_month_query(
    month: Option<&str>,
) -> Result<chrono::DateTime<chrono::Utc>, (StatusCode, Json<ErrorResponse>)> {
    let now = chrono::Utc::now();
    let Some(month) = month else {
        return Ok(month_start(now.year(), now.month()));
    };

    let parsed =
        chrono::NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d").map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Invalid month format: {month}. Expected YYYY-MM"),
                }),
            )
        })?;

    let naive = parsed.and_hms_opt(0, 0, 0).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Invalid month value: {month}"),
            }),
        )
    })?;

    Ok(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(naive, chrono::Utc))
}

async fn load_positions_for_month<E, S>(
    manager: &PositionManager<E, S>,
    month_start: chrono::DateTime<chrono::Utc>,
    state: &ApiState<E, S>,
) -> Result<Vec<PositionSummary>, DaemonError>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let mut positions: Vec<Position> =
        load_positions_for_month_positions(manager, month_start, state).await?;

    positions.sort_by_key(|position| position.created_at);

    let mut summaries = Vec::with_capacity(positions.len());
    for position in &positions {
        summaries.push(position_to_summary_with_live_price(manager, position).await);
    }

    Ok(summaries)
}

async fn load_positions_for_month_positions<E, S>(
    manager: &PositionManager<E, S>,
    month_start: chrono::DateTime<chrono::Utc>,
    state: &ApiState<E, S>,
) -> Result<Vec<Position>, DaemonError>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    #[cfg(feature = "postgres")]
    if let (Some(pool), Some(tenant_id)) = (&state.pg_pool, state.tenant_id) {
        return find_positions_overlapping_month(pool.as_ref(), tenant_id, month_start)
            .await
            .map_err(|e| DaemonError::Config(format!("Failed to load month projection: {}", e)));
    }

    let active = manager.store().positions().find_active().await?;
    let closed = manager.store().positions().find_all_closed().await?;
    let error = manager.store().positions().find_by_state("error").await?;
    let cancelled = manager.store().positions().find_by_state("cancelled").await?;

    let (next_year, next_month) = next_month(month_start.year(), month_start.month());
    let month_end = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
        chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)
            .expect("valid next month")
            .and_hms_opt(0, 0, 0)
            .expect("valid month boundary"),
        chrono::Utc,
    );

    Ok(active
        .into_iter()
        .chain(closed.into_iter())
        .chain(error.into_iter())
        .chain(cancelled.into_iter())
        .filter(|position| position_overlaps_month(position, month_start, month_end))
        .collect())
}

fn position_overlaps_month(
    position: &Position,
    month_start: chrono::DateTime<chrono::Utc>,
    month_end: chrono::DateTime<chrono::Utc>,
) -> bool {
    position.created_at < month_end
        && position.closed_at.map(|closed_at| closed_at > month_start).unwrap_or(true)
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

fn month_start(year: i32, month: u32) -> chrono::DateTime<chrono::Utc> {
    let naive = chrono::NaiveDate::from_ymd_opt(year, month, 1)
        .expect("valid month start")
        .and_hms_opt(0, 0, 0)
        .expect("valid month start time");
    chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(naive, chrono::Utc)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{header::CONTENT_TYPE, Request},
    };
    use http_body_util::BodyExt;
    use robson_domain::{Quantity, RiskConfig, TechnicalStopDistance, TradingPolicy};
    use robson_engine::Engine;
    use robson_exec::{Executor, IntentJournal, StubExchange};
    use robson_store::MemoryStore;
    use rust_decimal_macros::dec;
    use tokio::time::timeout;
    use tower::ServiceExt;

    use super::*;
    use crate::query_engine::TracingQueryRecorder;

    #[test]
    fn position_summary_uses_live_price_for_active_variation_pct_before_stop() {
        let mut position =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        position.entry_price = Some(Price::new(dec!(100)).unwrap());
        position.quantity = robson_domain::Quantity::new(dec!(2)).unwrap();
        position.state = PositionState::Active {
            current_price: Price::new(dec!(100)).unwrap(),
            trailing_stop: Price::new(dec!(95)).unwrap(),
            favorable_extreme: Price::new(dec!(105)).unwrap(),
            extreme_at: chrono::Utc::now(),
            insurance_stop_id: None,
            last_emitted_stop: None,
        };

        let summary =
            position_to_summary(&position, Some(Price::new(dec!(98)).unwrap()), None, None);

        assert_eq!(summary.current_price, Some(dec!(98)));
        assert_eq!(summary.pnl, Some(dec!(-4)));
        assert_eq!(summary.variation_pct, Some(dec!(-2.00)));
    }

    #[test]
    fn position_summary_uses_trailing_stop_for_active_variation_pct_after_stop_hit() {
        let mut position =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        position.entry_price = Some(Price::new(dec!(100)).unwrap());
        position.quantity = robson_domain::Quantity::new(dec!(2)).unwrap();
        position.state = PositionState::Active {
            current_price: Price::new(dec!(100)).unwrap(),
            trailing_stop: Price::new(dec!(95)).unwrap(),
            favorable_extreme: Price::new(dec!(105)).unwrap(),
            extreme_at: chrono::Utc::now(),
            insurance_stop_id: None,
            last_emitted_stop: None,
        };

        let summary =
            position_to_summary(&position, Some(Price::new(dec!(90)).unwrap()), None, None);

        assert_eq!(summary.current_price, Some(dec!(90)));
        assert_eq!(summary.pnl, Some(dec!(-10)));
        assert_eq!(summary.variation_pct, Some(dec!(-5.00)));
    }

    #[test]
    fn position_summary_uses_exit_price_for_closed_variation_pct() {
        let mut position =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Short);
        position.entry_price = Some(Price::new(dec!(100)).unwrap());
        position.quantity = robson_domain::Quantity::new(dec!(2)).unwrap();
        position.state = PositionState::Closed {
            exit_price: Price::new(dec!(90)).unwrap(),
            realized_pnl: dec!(20),
            exit_reason: robson_domain::ExitReason::UserPanic,
        };

        let summary = position_to_summary(&position, None, None, None);

        assert_eq!(summary.current_price, Some(dec!(90)));
        assert_eq!(summary.pnl, Some(dec!(20)));
        assert_eq!(summary.variation_pct, Some(dec!(10.0)));
    }

    #[test]
    fn position_overlaps_month_includes_inherited_and_closed_positions() {
        let month = month_start(2026, 4);
        let next = month_start(2026, 5);

        let mut inherited =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        inherited.created_at = month - chrono::Duration::days(10);
        inherited.closed_at = None;

        let mut closed_later =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        closed_later.created_at = month - chrono::Duration::days(10);
        closed_later.closed_at = Some(next + chrono::Duration::days(2));

        let mut closed_before =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        closed_before.created_at = month - chrono::Duration::days(20);
        closed_before.closed_at = Some(month - chrono::Duration::days(1));

        let mut born_after =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        born_after.created_at = next + chrono::Duration::days(1);
        born_after.closed_at = None;

        assert!(position_overlaps_month(&inherited, month, next));
        assert!(position_overlaps_month(&closed_later, month, next));
        assert!(!position_overlaps_month(&closed_before, month, next));
        assert!(!position_overlaps_month(&born_after, month, next));
    }

    #[test]
    fn next_month_handles_year_boundary() {
        assert_eq!(next_month(2026, 12), (2027, 1));
        assert_eq!(next_month(2026, 4), (2026, 5));
    }

    #[tokio::test]
    async fn test_month_positions_endpoint_includes_inherited_positions() {
        let (app, _event_bus, position_manager) = create_test_app_with_event_bus(100).await;
        let april = month_start(2026, 4);
        let may = month_start(2026, 5);

        let mut ada_armed =
            Position::new(Uuid::now_v7(), Symbol::from_pair("ADAUSDT").unwrap(), Side::Long);
        ada_armed.created_at = april + chrono::Duration::days(1);
        ada_armed.updated_at = april + chrono::Duration::days(3);
        ada_armed.state = PositionState::Armed;
        ada_armed.entry_price = Some(Price::new(dec!(100)).unwrap());

        let mut btc_long =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        btc_long.created_at = april + chrono::Duration::days(2);
        btc_long.updated_at = may + chrono::Duration::days(1);
        btc_long.closed_at = Some(may + chrono::Duration::days(1));
        btc_long.quantity = robson_domain::Quantity::new(dec!(1)).unwrap();
        btc_long.entry_price = Some(Price::new(dec!(100)).unwrap());
        btc_long.state = PositionState::Closed {
            exit_price: Price::new(dec!(110)).unwrap(),
            realized_pnl: dec!(10),
            exit_reason: robson_domain::ExitReason::TrailingStop,
        };

        let mut btc_short =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Short);
        btc_short.created_at = may + chrono::Duration::days(4);
        btc_short.updated_at = may + chrono::Duration::days(4);
        btc_short.state = PositionState::Armed;
        btc_short.entry_price = Some(Price::new(dec!(200)).unwrap());

        {
            let manager = position_manager.write().await;
            manager.store().positions().save(&ada_armed).await.unwrap();
            manager.store().positions().save(&btc_long).await.unwrap();
            manager.store().positions().save(&btc_short).await.unwrap();
        }

        let april_response = app
            .clone()
            .oneshot(
                Request::builder().uri("/positions?month=2026-04").body(Body::empty()).unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(april_response.status(), StatusCode::OK);
        let april_body = april_response.into_body().collect().await.unwrap().to_bytes();
        let april_month: MonthlyPositionsResponse = serde_json::from_slice(&april_body).unwrap();

        let may_response = app
            .oneshot(
                Request::builder().uri("/positions?month=2026-05").body(Body::empty()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(may_response.status(), StatusCode::OK);
        let may_body = may_response.into_body().collect().await.unwrap().to_bytes();
        let may_month: MonthlyPositionsResponse = serde_json::from_slice(&may_body).unwrap();

        assert_eq!(april_month.month, "2026-04");
        assert_eq!(may_month.month, "2026-05");

        assert_eq!(april_month.occupied_slots, 2);
        assert_eq!(april_month.slot_cells_total, 4);
        assert_eq!(april_month.positions.len(), 2);
        assert_eq!(april_month.positions[0].id, ada_armed.id);
        assert_eq!(april_month.positions[1].id, btc_long.id);

        assert_eq!(may_month.occupied_slots, 3);
        assert_eq!(may_month.slot_cells_total, 6);
        assert_eq!(may_month.positions.len(), 3);
        assert_eq!(may_month.positions[0].id, ada_armed.id);
        assert_eq!(may_month.positions[1].id, btc_long.id);
        assert_eq!(may_month.positions[2].id, btc_short.id);
    }

    #[tokio::test]
    async fn test_month_positions_endpoint_keeps_april_may_overlap_and_month_slots_change() {
        let (app, _event_bus, position_manager) = create_test_app_with_event_bus(100).await;
        let april = month_start(2026, 4);
        let may = month_start(2026, 5);

        let mut overlap =
            Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
        overlap.created_at = april + chrono::Duration::days(10);
        overlap.updated_at = may + chrono::Duration::days(2);
        overlap.closed_at = Some(may + chrono::Duration::days(2));
        overlap.quantity = robson_domain::Quantity::new(dec!(1)).unwrap();
        overlap.entry_price = Some(Price::new(dec!(100)).unwrap());
        overlap.state = PositionState::Closed {
            exit_price: Price::new(dec!(110)).unwrap(),
            realized_pnl: dec!(10),
            exit_reason: robson_domain::ExitReason::TrailingStop,
        };

        let mut april_only =
            Position::new(Uuid::now_v7(), Symbol::from_pair("ETHUSDT").unwrap(), Side::Short);
        april_only.created_at = april + chrono::Duration::days(2);
        april_only.updated_at = april + chrono::Duration::days(18);
        april_only.closed_at = Some(april + chrono::Duration::days(18));
        april_only.quantity = robson_domain::Quantity::new(dec!(2)).unwrap();
        april_only.entry_price = Some(Price::new(dec!(50)).unwrap());
        april_only.state = PositionState::Closed {
            exit_price: Price::new(dec!(40)).unwrap(),
            realized_pnl: dec!(20),
            exit_reason: robson_domain::ExitReason::UserPanic,
        };

        {
            let manager = position_manager.write().await;
            manager.store().positions().save(&overlap).await.unwrap();
            manager.store().positions().save(&april_only).await.unwrap();
        }

        let april_response = app
            .clone()
            .oneshot(
                Request::builder().uri("/positions?month=2026-04").body(Body::empty()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(april_response.status(), StatusCode::OK);
        let april_body = april_response.into_body().collect().await.unwrap().to_bytes();
        let april_month: MonthlyPositionsResponse = serde_json::from_slice(&april_body).unwrap();

        let may_response = app
            .oneshot(
                Request::builder().uri("/positions?month=2026-05").body(Body::empty()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(may_response.status(), StatusCode::OK);
        let may_body = may_response.into_body().collect().await.unwrap().to_bytes();
        let may_month: MonthlyPositionsResponse = serde_json::from_slice(&may_body).unwrap();

        assert_eq!(april_month.month, "2026-04");
        assert_eq!(may_month.month, "2026-05");
        assert_eq!(april_month.occupied_slots, 2);
        assert_eq!(may_month.occupied_slots, 1);
        assert_eq!(april_month.slot_cells_total, 4);
        assert_eq!(may_month.slot_cells_total, 5);

        assert_eq!(april_month.positions.len(), 2);
        assert!(april_month.positions.iter().any(|p| p.id == overlap.id));
        assert!(april_month.positions.iter().any(|p| p.id == april_only.id));

        assert_eq!(may_month.positions.len(), 1);
        assert_eq!(may_month.positions[0].id, overlap.id);
        assert!(may_month.positions.iter().all(|p| p.id != april_only.id));
    }

    async fn create_test_app_with_event_bus(
        capacity: usize,
    ) -> (Router, Arc<EventBus>, Arc<RwLock<PositionManager<StubExchange, MemoryStore>>>) {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(Arc::clone(&exchange), journal, store.clone()));
        let event_bus = Arc::new(crate::event_bus::EventBus::new(capacity));
        let risk_config = RiskConfig::new(dec!(10000)).unwrap();
        let engine = Engine::new(risk_config);

        let manager = PositionManager::new(
            engine,
            executor,
            store,
            Arc::clone(&event_bus),
            Arc::new(TracingQueryRecorder),
            TradingPolicy::default(),
        );

        let position_manager = Arc::new(RwLock::new(manager));
        let circuit_breaker = position_manager.read().await.circuit_breaker();

        let state = Arc::new(ApiState {
            exchange,
            position_manager: Arc::clone(&position_manager),
            event_bus: Arc::clone(&event_bus),
            circuit_breaker,
            position_monitor: None,
            #[cfg(feature = "postgres")]
            pg_pool: None,
            #[cfg(feature = "postgres")]
            tenant_id: None,
            api_token: None,
            funding: FundingConfig::default(),
        });

        (create_router(state), event_bus, position_manager)
    }

    #[tokio::test]
    async fn funding_execute_returns_503_when_disabled() {
        let (app, _, _) = create_test_app_with_event_bus(100).await;
        let body = serde_json::json!({ "quote_id": Uuid::now_v7() });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/funding/execute")
                    .header(CONTENT_TYPE, "application/json")
                    .header("Idempotency-Key", "test-key")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!({ "error": "funding_disabled" }));
    }

    // #[tokio::test]
    // async fn test_health_endpoint() {
    //     let app = create_test_app().await;
    //
    //     let response = app
    //         .oneshot(Request::builder().uri("/health").body(Body::empty()).
    // unwrap())         .await
    //         .unwrap();
    //
    //     assert_eq!(response.status(), StatusCode::OK);
    //
    //     let body = response.into_body().collect().await.unwrap().to_bytes();
    //     let health: HealthResponse = serde_json::from_slice(&body).unwrap();
    //
    //     assert_eq!(health.status, "healthy");
    // }

    // TODO: Fix tower ServiceExt import for oneshot
    // #[tokio::test]
    // async fn test_status_endpoint_empty() {
    //     let app = create_test_app().await;
    //
    //     let response = app
    //         .oneshot(
    //             Request::builder()
    //                 .uri("/status")
    //                 .body(Body::empty())
    //                 .unwrap(),
    //         )
    //         .await
    //         .unwrap();
    //
    //     assert_eq!(response.status(), StatusCode::OK);
    //
    //     let body = response.into_body().collect().await.unwrap().to_bytes();
    //     let status: StatusResponse = serde_json::from_slice(&body).unwrap();
    //
    //     assert_eq!(status.active_positions, 0);
    //     assert!(status.positions.is_empty());
    // }

    // TODO: Fix tower ServiceExt import for oneshot
    // #[tokio::test]
    // async fn test_arm_position() {
    //     let app = create_test_app().await;
    //
    //     let arm_req = serde_json::json!({
    //         "symbol": "BTCUSDT",
    //         "side": "LONG",
    //         "capital": "10000",
    //         "risk_percent": "0.01",
    //         "max_drawdown": "0.05",
    //         "tech_stop_percent": "0.02"
    //     });
    //
    //     let response = app
    //         .oneshot(
    //             Request::builder()
    //                 .method("POST")
    //                 .uri("/positions")
    //                 .header("content-type", "application/json")
    //                 .body(Body::from(arm_req.to_string()))
    //                 .unwrap(),
    //         )
    //         .await
    //         .unwrap();
    //
    //     assert_eq!(response.status(), StatusCode::CREATED);
    //
    //     let body = response.into_body().collect().await.unwrap().to_bytes();
    //     let arm_resp: ArmResponse = serde_json::from_slice(&body).unwrap();
    //
    //     assert_eq!(arm_resp.symbol, "BTCUSDT");
    //     assert_eq!(arm_resp.side, "Long");
    //     assert_eq!(arm_resp.state, "Armed");
    // }

    // TODO: Fix tower ServiceExt import for oneshot
    // #[tokio::test]
    // async fn test_arm_invalid_symbol() {
    //     let app = create_test_app().await;
    //
    //     let arm_req = serde_json::json!({
    //         "symbol": "INVALID",
    //         "side": "LONG",
    //         "capital": "10000",
    //         "risk_percent": "0.01",
    //         "max_drawdown": "0.05",
    //         "tech_stop_percent": "0.02"
    //     });
    //
    //     let response = app
    //         .oneshot(
    //             Request::builder()
    //                 .method("POST")
    //                 .uri("/positions")
    //                 .header("content-type", "application/json")
    //                 .body(Body::from(arm_req.to_string()))
    //                 .unwrap(),
    //         )
    //         .await
    //         .unwrap();
    //
    //     assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // }

    // TODO: Fix tower ServiceExt import for oneshot
    // #[tokio::test]
    // async fn test_get_position_not_found() {
    //     let app = create_test_app().await;
    //     let fake_id = Uuid::now_v7();
    //
    //     let response = app
    //         .oneshot(
    //             Request::builder()
    //                 .uri(format!("/positions/{}", fake_id))
    //                 .body(Body::empty())
    //                 .unwrap(),
    //         )
    //         .await
    //         .unwrap();
    //
    //     assert_eq!(response.status(), StatusCode::NOT_FOUND);
    // }

    #[tokio::test]
    async fn test_events_endpoint_returns_sse_headers_and_frame() {
        let (app, event_bus, _) = create_test_app_with_event_bus(100).await;

        let response = app
            .oneshot(Request::builder().uri("/events").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "text/event-stream");
        assert_eq!(response.headers().get(header::CACHE_CONTROL).unwrap(), "no-cache");
        assert_eq!(response.headers().get("x-accel-buffering").unwrap(), "no");

        event_bus.send(crate::event_bus::DaemonEvent::PositionStateChanged {
            position_id: Uuid::now_v7(),
            previous_state: "armed".to_string(),
            new_state: "active".to_string(),
            timestamp: chrono::Utc::now(),
        });

        let mut body = response.into_body();
        let frame = timeout(Duration::from_secs(1), body.frame())
            .await
            .expect("Timed out waiting for SSE frame")
            .expect("Expected SSE body frame")
            .expect("SSE body frame should be valid");
        let bytes = frame.into_data().expect("Expected SSE data frame");
        let frame_text = String::from_utf8(bytes.to_vec()).unwrap();

        assert!(frame_text.contains("id: "));
        assert!(frame_text.contains("event: position.changed"));
        assert!(frame_text.contains("data: "));
        assert!(frame_text.contains("\"event_type\":\"position.changed\""));
    }

    #[tokio::test]
    async fn test_events_endpoint_emits_resync_required_on_broadcast_lag() {
        let (app, event_bus, _) = create_test_app_with_event_bus(1).await;

        let response = app
            .oneshot(Request::builder().uri("/events").body(Body::empty()).unwrap())
            .await
            .unwrap();

        event_bus.send(crate::event_bus::DaemonEvent::PositionStateChanged {
            position_id: Uuid::now_v7(),
            previous_state: "armed".to_string(),
            new_state: "entering".to_string(),
            timestamp: chrono::Utc::now(),
        });
        event_bus.send(crate::event_bus::DaemonEvent::PositionStateChanged {
            position_id: Uuid::now_v7(),
            previous_state: "entering".to_string(),
            new_state: "active".to_string(),
            timestamp: chrono::Utc::now(),
        });

        let mut body = response.into_body();
        let frame = timeout(Duration::from_secs(1), body.frame())
            .await
            .expect("Timed out waiting for lag SSE frame")
            .expect("Expected lag SSE body frame")
            .expect("Lag SSE body frame should be valid");
        let bytes = frame.into_data().expect("Expected lag SSE data frame");
        let frame_text = String::from_utf8(bytes.to_vec()).unwrap();

        assert!(frame_text.contains("event: system.resync_required"));
        assert!(frame_text.contains("\"event_type\":\"system.resync_required\""));
        assert!(frame_text.contains("\"reason\":\"lagged\""));
    }

    #[tokio::test]
    async fn test_status_endpoint_includes_pending_approvals_for_bootstrap() {
        let (app, _event_bus, position_manager) = create_test_app_with_event_bus(100).await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let hc_policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = {
            let manager = position_manager.write().await;
            manager
                .arm_position_with_policy(
                    symbol.clone(),
                    Side::Long,
                    RiskConfig::new(dec!(10000)).unwrap(),
                    Some(tech_stop),
                    Uuid::now_v7(),
                    hc_policy,
                )
                .await
                .unwrap()
        };

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        let query_id = {
            let manager = position_manager.write().await;
            manager.handle_signal(signal).await.unwrap();
            manager
                .get_pending_approvals()
                .await
                .into_iter()
                .next()
                .expect("pending approval must exist")
                .id
        };

        let response = app
            .oneshot(Request::builder().uri("/status").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let status: StatusResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(status.active_positions, 1);
        assert_eq!(status.occupied_slots, 1);
        // Armed-position risk is reflected in capital_base at month boundary,
        // not subtracted from the live slot count.
        assert_eq!(status.new_slots_available, 4);
        assert_eq!(status.slot_cells_total, 5);
        assert_eq!(status.pending_approvals.len(), 1);
        assert_eq!(status.pending_approvals[0].query_id, query_id);
        assert_eq!(status.pending_approvals[0].position_id, Some(position.id));
        assert_eq!(status.pending_approvals[0].state, "AwaitingApproval");
    }

    #[tokio::test]
    async fn test_approve_query_endpoint_resumes_pending_query() {
        let (app, event_bus, position_manager) = create_test_app_with_event_bus(100).await;
        let mut receiver = event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let hc_policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = {
            let manager = position_manager.write().await;
            manager
                .arm_position_with_policy(
                    symbol.clone(),
                    Side::Long,
                    RiskConfig::new(dec!(10000)).unwrap(),
                    Some(tech_stop),
                    Uuid::now_v7(),
                    hc_policy,
                )
                .await
                .unwrap()
        };

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(), /* HumanConfirmation always requires
                                                          * approval */
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        {
            let manager = position_manager.write().await;
            manager.handle_signal(signal).await.unwrap();
        }

        let query_id = loop {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_secs(1), receiver.recv()).await
            {
                match event.unwrap() {
                    crate::event_bus::DaemonEvent::QueryAwaitingApproval { query_id, .. } => {
                        break query_id;
                    },
                    _ => continue,
                }
            } else {
                panic!("Expected QueryAwaitingApproval event");
            }
        };

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/queries/{}/approve", query_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let approval: ApproveQueryResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(approval.query_id, query_id);
        assert_eq!(approval.state, "Completed");
    }

    #[test]
    fn build_cors_layer_parses_origins_from_env() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("ROBSON_CORS_ALLOWED_ORIGINS");
        let _ = build_cors_layer();
        std::env::set_var("ROBSON_CORS_ALLOWED_ORIGINS", "  , ");
        let _ = build_cors_layer();
        std::env::set_var("ROBSON_CORS_ALLOWED_ORIGINS", "https://robson.rbx.ia.br");
        let _ = build_cors_layer();
        std::env::set_var(
            "ROBSON_CORS_ALLOWED_ORIGINS",
            "https://robson.rbx.ia.br,https://robson.rbxsystems.ch",
        );
        let _ = build_cors_layer();
        std::env::remove_var("ROBSON_CORS_ALLOWED_ORIGINS");
    }

    // =========================================================================
    // Reconcile Close API Tests (Slice 5B1)
    // =========================================================================

    async fn save_active_position_for_api(
        position_manager: &Arc<RwLock<PositionManager<StubExchange, MemoryStore>>>,
        symbol: &str,
        side: Side,
        entry_price: Decimal,
        quantity: Decimal,
    ) -> Position {
        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair(symbol).unwrap();
        let entry_price = Price::new(entry_price).unwrap();
        let trailing_stop = match side {
            Side::Long => Price::new(entry_price.as_decimal() - dec!(10)).unwrap(),
            Side::Short => Price::new(entry_price.as_decimal() + dec!(10)).unwrap(),
        };
        let quantity = Quantity::new(quantity).unwrap();
        let now = chrono::Utc::now();

        let mut position = Position::new(account_id, symbol, side);
        position.entry_price = Some(entry_price);
        position.entry_filled_at = Some(now);
        position.quantity = quantity;
        position.state = PositionState::Active {
            current_price: entry_price,
            trailing_stop,
            favorable_extreme: entry_price,
            extreme_at: now,
            insurance_stop_id: None,
            last_emitted_stop: None,
        };
        position.updated_at = now;

        position_manager.read().await.store().positions().save(&position).await.unwrap();
        position
    }

    fn make_order_fill_evidence_json(exit_price: &str, quantity: &str) -> serde_json::Value {
        serde_json::json!({
            "source": "order_fill_record",
            "data": {
                "exchange_order_id": "ORD-1",
                "fill_price": exit_price,
                "filled_quantity": quantity,
                "fee": "0.01",
                "fee_asset": "USDT",
                "filled_at": "2026-05-09T14:30:00Z"
            }
        })
    }

    fn make_user_trade_evidence_json(exit_price: &str, quantity: &str) -> serde_json::Value {
        serde_json::json!({
            "source": "user_trade_record",
            "data": {
                "exchange_order_id": "ORD-2",
                "exchange_trade_id": "TRADE-1",
                "fill_price": exit_price,
                "filled_quantity": quantity,
                "fee": "0.01",
                "fee_asset": "USDT",
                "filled_at": "2026-05-09T14:30:00Z"
            }
        })
    }

    #[tokio::test]
    async fn test_reconcile_close_api_success_order_fill() {
        let (app, _event_bus, pm) = create_test_app_with_event_bus(100).await;
        let pos =
            save_active_position_for_api(&pm, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;

        let body = serde_json::json!({
            "position_id": pos.id,
            "evidence": make_order_fill_evidence_json("90", "1")
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let resp_body = response.into_body().collect().await.unwrap().to_bytes();
        let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(resp["status"], "closed");
        assert_eq!(resp["position_id"], pos.id.to_string());
        assert_eq!(resp["closure_evidence"]["kind"], "reconciled");
        assert_eq!(resp["closure_evidence"]["evidence"]["source"], "order_fill_record");
        assert_eq!(resp["closure_evidence"]["evidence"]["data"]["exchange_order_id"], "ORD-1");
    }

    #[tokio::test]
    async fn test_reconcile_close_api_success_user_trade() {
        let (app, _event_bus, pm) = create_test_app_with_event_bus(100).await;
        let pos =
            save_active_position_for_api(&pm, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;

        let body = serde_json::json!({
            "position_id": pos.id,
            "evidence": make_user_trade_evidence_json("90", "1")
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let resp_body = response.into_body().collect().await.unwrap().to_bytes();
        let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(resp["status"], "closed");
        assert_eq!(resp["closure_evidence"]["kind"], "reconciled");
        assert_eq!(resp["closure_evidence"]["evidence"]["source"], "user_trade_record");
        assert_eq!(resp["closure_evidence"]["evidence"]["data"]["exchange_trade_id"], "TRADE-1");
    }

    #[tokio::test]
    async fn test_reconcile_close_api_position_not_found() {
        let (app, _event_bus, _pm) = create_test_app_with_event_bus(100).await;
        let fake_id = Uuid::now_v7();

        let body = serde_json::json!({
            "position_id": fake_id,
            "evidence": make_order_fill_evidence_json("90", "1")
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let resp_body = response.into_body().collect().await.unwrap().to_bytes();
        let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(resp["error"], "position_not_found");
    }

    #[tokio::test]
    async fn test_reconcile_close_api_position_not_active() {
        let (app, _event_bus, pm) = create_test_app_with_event_bus(100).await;
        // Save position in Armed state (not Active) — reconcile_close should
        // reject it with SkippedNonActive.
        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut pos = Position::new(account_id, symbol, Side::Long);
        pos.updated_at = chrono::Utc::now();
        pm.read().await.store().positions().save(&pos).await.unwrap();

        let body = serde_json::json!({
            "position_id": pos.id,
            "evidence": make_order_fill_evidence_json("90", "1")
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let resp_body = response.into_body().collect().await.unwrap().to_bytes();
        let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(resp["error"], "position_not_active");
    }

    #[tokio::test]
    async fn test_reconcile_close_api_rejects_invalid_real_evidence_fields() {
        let (app, _event_bus, pm) = create_test_app_with_event_bus(100).await;
        let pos =
            save_active_position_for_api(&pm, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;

        let body = serde_json::json!({
            "position_id": pos.id,
            "evidence": make_order_fill_evidence_json("-1", "1")
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = response.into_body().collect().await.unwrap().to_bytes();
        let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(resp["error"], "invalid_evidence");
        assert_eq!(resp["details"], "fill_price must be > 0");
    }

    #[tokio::test]
    async fn test_reconcile_close_api_second_call_conflicts_after_close() {
        let (app, _event_bus, pm) = create_test_app_with_event_bus(100).await;
        let pos =
            save_active_position_for_api(&pm, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;

        // First close succeeds
        let body = serde_json::json!({
            "position_id": pos.id,
            "evidence": make_order_fill_evidence_json("90", "1")
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Second close returns 409 (idempotent — not active anymore)
        let body2 = serde_json::json!({
            "position_id": pos.id,
            "evidence": make_order_fill_evidence_json("85", "1")
        });
        let response2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body2.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response2.status(), StatusCode::CONFLICT);
        let resp_body = response2.into_body().collect().await.unwrap().to_bytes();
        let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(resp["error"], "position_not_active");
    }

    #[tokio::test]
    async fn test_reconcile_close_api_unauthorized() {
        let (app_with_token, _event_bus, pm) = {
            let exchange = Arc::new(StubExchange::new(dec!(95000)));
            let journal = Arc::new(IntentJournal::new());
            let store = Arc::new(MemoryStore::new());
            let executor = Arc::new(Executor::new(Arc::clone(&exchange), journal, store.clone()));
            let event_bus = Arc::new(crate::event_bus::EventBus::new(100));
            let risk_config = RiskConfig::new(dec!(10000)).unwrap();
            let engine = Engine::new(risk_config);
            let manager = PositionManager::new(
                engine,
                executor,
                store,
                Arc::clone(&event_bus),
                Arc::new(TracingQueryRecorder),
                TradingPolicy::default(),
            );
            let position_manager = Arc::new(RwLock::new(manager));
            let circuit_breaker = position_manager.read().await.circuit_breaker();

            let state = Arc::new(ApiState {
                exchange,
                position_manager: Arc::clone(&position_manager),
                event_bus: Arc::clone(&event_bus),
                circuit_breaker,
                position_monitor: None,
                #[cfg(feature = "postgres")]
                pg_pool: None,
                #[cfg(feature = "postgres")]
                tenant_id: None,
                api_token: Some("secret-token-123".to_string()),
                funding: FundingConfig::default(),
            });

            (create_router(state), event_bus, position_manager)
        };

        let _pos =
            save_active_position_for_api(&pm, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;

        let body = serde_json::json!({
            "position_id": Uuid::now_v7(),
            "evidence": make_order_fill_evidence_json("90", "1")
        });

        let response = app_with_token
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_reconcile_close_api_idempotent() {
        let (app, _event_bus, pm) = create_test_app_with_event_bus(100).await;
        let pos =
            save_active_position_for_api(&pm, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;

        let body = serde_json::json!({
            "position_id": pos.id,
            "evidence": make_order_fill_evidence_json("90", "1")
        });

        // First call closes
        let r1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);

        // Second call returns 409
        let r2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_reconcile_close_api_rejects_account_snapshot() {
        let (app, _event_bus, _pm) = create_test_app_with_event_bus(100).await;

        let body = serde_json::json!({
            "position_id": Uuid::now_v7(),
            "evidence": {
                "source": "account_snapshot",
                "data": {
                    "first_observed_missing_at": "2026-05-09T14:00:00Z",
                    "confirmed_missing_at": "2026-05-09T14:01:00Z"
                }
            }
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = response.into_body().collect().await.unwrap().to_bytes();
        let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(resp["error"], "unsupported_evidence");
    }

    #[tokio::test]
    async fn test_reconcile_close_api_rejects_estimated() {
        let (app, _event_bus, _pm) = create_test_app_with_event_bus(100).await;

        let body = serde_json::json!({
            "position_id": Uuid::now_v7(),
            "evidence": {
                "source": "estimated",
                "data": {
                    "estimation_basis": "trailing_stop_at_detection",
                    "exit_price": "95000.00",
                    "evaluator": "op:ldamasio",
                    "detected_at": "2026-05-09T14:30:00Z"
                }
            }
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/reconcile-close")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = response.into_body().collect().await.unwrap().to_bytes();
        let resp: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
        assert_eq!(resp["error"], "unsupported_evidence");
    }
}

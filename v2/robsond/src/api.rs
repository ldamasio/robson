//! HTTP API for the Robson daemon.
//!
//! Provides REST endpoints for:
//! - Health check
//! - Status (active positions)
//! - Arm position
//! - Disarm position
//! - Panic (emergency close all)
//! - Safety net (rogue position monitoring)
//! - SSE events for operator-facing runtime updates

use std::{convert::Infallible, sync::Arc, time::Duration};

use async_stream::stream;
use axum::{
    extract::{Path, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{KeepAlive, Sse},
        IntoResponse,
    },
    routing::{delete, get, post},
    Json, Router,
};
use robson_domain::{
    DetectorSignal, Position, PositionState, Price, RiskConfig, Side, Symbol, TechnicalStopDistance,
};
use robson_exec::ExchangePort;
use robson_store::Store;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;
use uuid::Uuid;

use crate::{
    circuit_breaker::{CircuitBreaker, HaltState, MonthlyHaltSnapshot},
    error::DaemonError,
    event_bus::{DaemonEvent, EventBus},
    position_manager::PositionManager,
    position_monitor::PositionMonitor,
    sse::{map_daemon_event, resync_required_event},
};

// =============================================================================
// API State
// =============================================================================

/// Shared state for API handlers.
pub struct ApiState<E: ExchangePort + 'static, S: Store + 'static> {
    pub position_manager: Arc<RwLock<PositionManager<E, S>>>,
    pub event_bus: Arc<EventBus>,
    pub circuit_breaker: Arc<CircuitBreaker>,
    pub position_monitor: Option<Arc<PositionMonitor>>,
    /// PostgreSQL pool for liveness check. Present only when DATABASE_URL is
    /// configured.
    #[cfg(feature = "postgres")]
    pub pg_pool: Option<std::sync::Arc<sqlx::PgPool>>,
    /// Bearer token for authenticating mutating routes. `None` means auth is
    /// disabled (non-production environments only).
    pub api_token: Option<String>,
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
}

/// Summary of a position.
#[derive(Debug, Serialize, Deserialize)]
pub struct PositionSummary {
    pub id: Uuid,
    pub symbol: String,
    pub side: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_price: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailing_stop: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pnl: Option<Decimal>,
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

/// Request to arm a new position.
///
/// Note: risk per trade is fixed at 1% by v3 policy. Not configurable via API.
#[derive(Debug, Deserialize)]
pub struct ArmRequest {
    pub symbol: String,
    pub side: String,
    pub capital: Decimal,
    #[serde(default = "default_account_id")]
    pub account_id: Uuid,
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
            let auth_header = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok());

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
        .route("/positions/:id", delete(disarm_handler))
        .route("/positions/:id/signal", post(signal_handler))
        .route("/queries/:id/approve", post(approve_query_handler))
        .route("/panic", post(panic_handler))
        // MonthlyHalt trigger (mutating)
        .route("/monthly-halt", post(monthly_halt_trigger_handler))
        .layer(auth_layer)
        .with_state(state);

    read_only.merge(mutating)
}

// =============================================================================
// Handlers
// =============================================================================

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
    let mut database_ok;
    let mut binance_ok = false;

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
    if state.position_monitor.is_some() {
        binance_ok = true; // Assume OK if monitor is configured
    } else {
        binance_ok = true; // OK even if not configured (Safety Net is optional)
    }

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

    let summaries: Vec<PositionSummary> = positions.iter().map(position_to_summary).collect();

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

    Ok(Json(position_to_summary(&position)))
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

    // Create risk config (v3 policy: risk is fixed at 1%, not from request)
    let risk_config = RiskConfig::new(req.capital).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Invalid risk config: {}", e),
            }),
        )
    })?;

    // Create a dummy tech stop distance (will be replaced by detector signal)
    // Note: In production, the detector signal provides the actual tech stop
    // distance Use Price::zero() to bypass validation (allowed for
    // initialization only)
    let entry_price = Price::new(rust_decimal::Decimal::ONE).unwrap();
    let stop_loss = Price::zero();
    let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry_price, stop_loss);

    // Arm position
    let manager = state.position_manager.write().await;
    let position = manager
        .arm_position(symbol.clone(), side, risk_config, tech_stop, req.account_id)
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

/// Disarm (cancel) a position.
async fn disarm_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let manager = state.position_manager.write().await;
    manager.disarm_position(id).await.map_err(|e| to_error_response(e))?;

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

fn position_to_summary(position: &Position) -> PositionSummary {
    let (state_str, entry_price, trailing_stop, pnl) = match &position.state {
        PositionState::Armed => ("Armed".to_string(), None, None, None),
        PositionState::Entering { expected_entry, .. } => {
            ("Entering".to_string(), Some(expected_entry.as_decimal()), None, None)
        },
        PositionState::Active { trailing_stop, .. } => (
            "Active".to_string(),
            position.entry_price.map(|p| p.as_decimal()),
            Some(trailing_stop.as_decimal()),
            Some(position.calculate_pnl()),
        ),
        PositionState::Exiting { .. } => (
            "Exiting".to_string(),
            position.entry_price.map(|p| p.as_decimal()),
            None,
            Some(position.calculate_pnl()),
        ),
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
                Some(realized_pnl),
            )
        },
        PositionState::Error { error, .. } => (format!("Error: {}", error), None, None, None),
    };

    PositionSummary {
        id: position.id,
        symbol: position.symbol.as_pair(),
        side: format!("{:?}", position.side),
        state: state_str,
        entry_price,
        trailing_stop,
        pnl,
    }
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
    use robson_domain::RiskConfig;
    use robson_engine::Engine;
    use robson_exec::{Executor, IntentJournal, StubExchange};
    use robson_store::MemoryStore;
    use rust_decimal_macros::dec;
    use tokio::time::timeout;
    use tower::ServiceExt;

    use super::*;
    use crate::query_engine::TracingQueryRecorder;

    async fn create_test_app_with_event_bus(
        capacity: usize,
    ) -> (Router, Arc<EventBus>, Arc<RwLock<PositionManager<StubExchange, MemoryStore>>>) {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
        let event_bus = Arc::new(crate::event_bus::EventBus::new(capacity));
        let risk_config = RiskConfig::new(dec!(10000)).unwrap();
        let engine = Engine::new(risk_config);

        let manager = PositionManager::new(
            engine,
            executor,
            store,
            Arc::clone(&event_bus),
            Arc::new(TracingQueryRecorder),
        );

        let position_manager = Arc::new(RwLock::new(manager));
        let circuit_breaker = position_manager.read().await.circuit_breaker();

        let state = Arc::new(ApiState {
            position_manager: Arc::clone(&position_manager),
            event_bus: Arc::clone(&event_bus),
            circuit_breaker,
            position_monitor: None,
            #[cfg(feature = "postgres")]
            pg_pool: None,
            api_token: None,
        });

        (create_router(state), event_bus, position_manager)
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

        let position = {
            let manager = position_manager.write().await;
            manager
                .arm_position(
                    symbol.clone(),
                    Side::Long,
                    RiskConfig::new(dec!(10000)).unwrap(),
                    tech_stop,
                    Uuid::now_v7(),
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

        let position = {
            let manager = position_manager.write().await;
            manager
                .arm_position(
                    symbol.clone(),
                    Side::Long,
                    RiskConfig::new(dec!(10000)).unwrap(),
                    tech_stop,
                    Uuid::now_v7(),
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
            stop_loss: Price::new(dec!(85500)).unwrap(), /* 10% stop -> approval required, risk
                                                          * approved */
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
}

//! HTTP API for the Robson daemon.
//!
//! Provides REST endpoints for:
//! - Health check
//! - Status (active positions)
//! - Arm position
//! - Disarm position
//! - Panic (emergency close all)

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use robson_domain::{
    DetectorSignal, Position, PositionState, Price, RiskConfig, Side, Symbol,
    TechnicalStopDistance,
};
use robson_exec::ExchangePort;
use robson_store::Store;

use crate::error::DaemonError;
use crate::position_manager::PositionManager;

// =============================================================================
// API State
// =============================================================================

/// Shared state for API handlers.
pub struct ApiState<E: ExchangePort + 'static, S: Store + 'static> {
    pub position_manager: Arc<RwLock<PositionManager<E, S>>>,
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Health check response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

/// Status response.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub active_positions: usize,
    pub positions: Vec<PositionSummary>,
}

/// Summary of a position.
#[derive(Debug, Serialize)]
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

/// Request to arm a new position.
#[derive(Debug, Deserialize)]
pub struct ArmRequest {
    pub symbol: String,
    pub side: String,
    pub capital: Decimal,
    pub risk_percent: Decimal,
    #[serde(default = "default_account_id")]
    pub account_id: Uuid,
}

fn default_account_id() -> Uuid {
    Uuid::nil()
}

/// Response after arming a position.
#[derive(Debug, Serialize)]
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
#[derive(Debug, Serialize)]
pub struct PanicResponse {
    pub closed_positions: Vec<Uuid>,
    pub count: usize,
}

/// Error response.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

// =============================================================================
// Router
// =============================================================================

/// Create the API router.
pub fn create_router<E, S>(state: Arc<ApiState<E, S>>) -> Router
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    Router::new()
        .route("/health", get(health_handler))
        .route("/status", get(status_handler))
        .route("/positions", post(arm_handler))
        .route("/positions/:id", get(get_position_handler))
        .route("/positions/:id", delete(disarm_handler))
        .route("/positions/:id/signal", post(signal_handler))
        .route("/panic", post(panic_handler))
        .with_state(state)
}

// =============================================================================
// Handlers
// =============================================================================

/// Health check endpoint.
async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// Get status (all positions).
async fn status_handler<E, S>(
    State(state): State<Arc<ApiState<E, S>>>,
) -> Result<Json<StatusResponse>, (StatusCode, Json<ErrorResponse>)>
where
    E: ExchangePort + 'static,
    S: Store + 'static,
{
    let manager = state.position_manager.read().await;
    let positions = manager
        .get_active_positions()
        .await
        .map_err(|e| to_error_response(e))?;

    let summaries: Vec<PositionSummary> = positions.iter().map(position_to_summary).collect();

    Ok(Json(StatusResponse {
        active_positions: summaries.len(),
        positions: summaries,
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
    let position = manager
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
            Json(ErrorResponse {
                error: format!("Invalid symbol: {}", e),
            }),
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
            ))
        }
    };

    // Create risk config
    let risk_config = RiskConfig::new(req.capital, req.risk_percent).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Invalid risk config: {}", e),
            }),
        )
    })?;

    // Create a dummy tech stop distance (will be replaced by detector signal)
    // Note: In production, the detector signal provides the actual tech stop distance
    // Use Price::zero() to bypass validation (allowed for initialization only)
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
    manager
        .disarm_position(id)
        .await
        .map_err(|e| to_error_response(e))?;

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
    let position = manager
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

    manager
        .handle_signal(signal)
        .await
        .map_err(|e| to_error_response(e))?;

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
    let closed = manager
        .panic_close_all()
        .await
        .map_err(|e| to_error_response(e))?;

    Ok(Json(PanicResponse {
        count: closed.len(),
        closed_positions: closed,
    }))
}

// =============================================================================
// Helpers
// =============================================================================

fn to_error_response(error: DaemonError) -> (StatusCode, Json<ErrorResponse>) {
    let status = match &error {
        DaemonError::PositionNotFound(_) => StatusCode::NOT_FOUND,
        DaemonError::InvalidPositionState { .. } => StatusCode::CONFLICT,
        DaemonError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };

    (
        status,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}

fn position_to_summary(position: &Position) -> PositionSummary {
    let (state_str, entry_price, trailing_stop, pnl) = match &position.state {
        PositionState::Armed => ("Armed".to_string(), None, None, None),
        PositionState::Entering {
            expected_entry, ..
        } => (
            "Entering".to_string(),
            Some(expected_entry.as_decimal()),
            None,
            None,
        ),
        PositionState::Active {
            trailing_stop, ..
        } => (
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
        PositionState::Closed {
            exit_price,
            exit_reason,
            ..
        } => {
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
        }
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
    use super::*;
    use robson_domain::RiskConfig;
    use robson_engine::Engine;
    use robson_exec::{Executor, IntentJournal, StubExchange};
    use robson_store::MemoryStore;
    use rust_decimal_macros::dec;

    async fn create_test_app() -> Router {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
        let event_bus = Arc::new(crate::event_bus::EventBus::new(100));
        let risk_config = RiskConfig::new(dec!(10000), dec!(1)).unwrap();
        let engine = Engine::new(risk_config);

        let manager = PositionManager::new(engine, executor, store, event_bus);

        let state = Arc::new(ApiState {
            position_manager: Arc::new(RwLock::new(manager)),
        });

        create_router(state)
    }

    // ============================================================================
    // TODO: Re-enable API tests - requires tower/util feature
    // ============================================================================
    // The API integration tests below use `tower::ServiceExt::oneshot()` which
    // requires the "util" feature of the tower crate. To re-enable these tests:
    //
    // 1. Add tower = { version = "0.4", features = ["util"] } to workspace dependencies
    // 2. Uncomment the test functions below
    // 3. Uncomment the import: use tower::util::ServiceExt;
    //
    // Reference: https://github.com/tower-rs/tower/blob/master/tower/src/util/mod.rs
    // ============================================================================
    //
    // #[tokio::test]
    // async fn test_health_endpoint() {
    //     let app = create_test_app().await;
    //
    //     let response = app
    //         .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
    //         .await
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
}

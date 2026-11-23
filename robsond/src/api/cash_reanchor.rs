//! `POST /venue/cash-reanchor {reason}` (ADR-0056 step 8l-5; v16 §C8;
//! PLAN-r4 §6.12): the operator's re-anchor command, on the authenticated
//! router. The service is the daemon's, added to the router as an extension
//! at the 5a composition.
use std::sync::Arc;

use axum::{
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::Deserialize;

use super::ErrorResponse;
use crate::{auth::OperatorIdentity, venue_ledger_store::cash::reanchor::CashReanchor};

/// The longest reason accepted, as the CLI's, and the body's byte bound.
const MAX_REASON: usize = 2000;
const MAX_BODY: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
pub(super) struct ReanchorBody {
    reason: String,
}

/// The handler requires a verified identity itself, so it answers 401
/// without one even when the router's authentication is unconfigured (v16
/// §C8), before it reads any of the body (review r1). Without the composed
/// service it answers 503. The run has its own task, so a client that
/// disconnects never cancels a transaction midway; the single flight is the
/// service's.
pub(super) async fn handler(
    service: Option<Extension<Arc<CashReanchor>>>,
    request: Request,
) -> Response {
    let fail = |status: StatusCode, error: &str| {
        (status, Json(ErrorResponse { error: error.to_string() })).into_response()
    };
    let Some(identity) = request.extensions().get::<OperatorIdentity>().cloned() else {
        return fail(StatusCode::UNAUTHORIZED, "a verified identity is required");
    };
    let body = axum::body::to_bytes(request.into_body(), MAX_BODY).await.ok();
    let body = body.and_then(|body| serde_json::from_slice::<ReanchorBody>(&body).ok());
    let reason = body.map(|body| body.reason.trim().to_string()).unwrap_or_default();
    if reason.is_empty() || reason.chars().count() > MAX_REASON {
        return fail(StatusCode::BAD_REQUEST, "a reason of 1 to 2000 characters is required");
    }
    let Some(Extension(service)) = service else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "cash re-anchor is not available");
    };
    match tokio::spawn(async move { service.run(&identity.subject, &reason).await }).await {
        Ok(Ok(done)) => Json(done).into_response(),
        Ok(Err(refusal)) => (StatusCode::CONFLICT, Json(refusal)).into_response(),
        Err(_) => fail(StatusCode::INTERNAL_SERVER_ERROR, "operation failed"),
    }
}

#[cfg(test)]
#[path = "cash_reanchor_tests.rs"]
mod tests;

//! The re-anchor endpoint on the daemon's own router (PLAN-r4 §8 row 8l-5):
//! identity first, even with the router's authentication unconfigured; the
//! reason; the composition seam; the single flight and read-only mode,
//! refused before any I/O.
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Extension, Router,
};
use http_body_util::BodyExt;
use robson_exec::{
    order_journal::{OrderJournal, StubJournalBackend},
    venue::{AccountKind, VenueId},
};
use tower::ServiceExt;
use uuid::Uuid;

use super::super::{create_router, ApiState, FundingRouteMode, VenueFeedView};
use crate::{
    balance_accounting::{BalanceAccounting, GenerationCell},
    balance_reconciliation::AccountReceipt,
    capability::worker_venue::{disconnected_pool, manager, NoAccessStore, WorkerVenue},
    config::FundingConfig,
    event_bus::EventBus,
    venue_ledger_store::{
        cash::reanchor::{CashReanchor, ReanchorVenue},
        settlement::CommissionAssets,
    },
};

const TOKEN: &str = "reanchor-test-token";

/// A venue that answers nothing and counts the requests.
#[derive(Default)]
struct Silent(AtomicUsize);

#[async_trait::async_trait]
impl ReanchorVenue for Silent {
    async fn account(&self) -> Option<AccountReceipt> {
        self.0.fetch_add(1, Ordering::Relaxed);
        None
    }

    fn revalidate(&self, _: &AccountReceipt) -> bool {
        true
    }
}

/// The daemon's router on a balance account, its authentication
/// unconfigured or bridged by `token`, its pool unreachable.
async fn router(token: Option<&str>) -> Router {
    let exchange = Arc::new(WorkerVenue::new(AccountKind::BalanceAccount, false));
    let position_manager = manager(&exchange, Arc::new(NoAccessStore));
    let held = position_manager.read().await;
    let (circuit_breaker, order_journal) = (held.circuit_breaker(), held.order_journal());
    drop(held);
    create_router(Arc::new(ApiState {
        exchange,
        position_manager,
        event_bus: Arc::new(EventBus::new(8)),
        circuit_breaker,
        wallet_balance_cache: tokio::sync::Mutex::new(None),
        pg_pool: Some(Arc::new(disconnected_pool())),
        tenant_id: Some(Uuid::now_v7()),
        google_auth: None,
        legacy_api_token: token.map(str::to_string),
        funding: FundingConfig::default(),
        funding_route_mode: FundingRouteMode::Selected,
        venue_feed: VenueFeedView::default(),
        order_journal,
    }))
}

fn service(venue: Arc<Silent>, read_only: bool) -> Arc<CashReanchor> {
    let spot = VenueId::new(VenueId::BINANCE_SPOT);
    let (tenant, cell) = (Uuid::now_v7(), GenerationCell::default());
    Arc::new(CashReanchor {
        pool: disconnected_pool(),
        accounting: BalanceAccounting::for_daemon(
            AccountKind::BalanceAccount,
            tenant,
            &spot,
            &cell,
        )
        .expect("a balance account"),
        gate: OrderJournal::new(Arc::new(StubJournalBackend::default()), &spot).gate().clone(),
        epoch: Uuid::now_v7(),
        uid: "4242".into(),
        pair: CommissionAssets {
            symbol: "BTCUSDT",
            base: "BTC",
            quote: "USDT",
        },
        venue,
        read_only,
        flight: Default::default(),
    })
}

async fn post(app: &Router, token: Option<&str>, body: &str) -> (StatusCode, serde_json::Value) {
    let mut request =
        Request::post("/venue/cash-reanchor").header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

const BODY: &str = r#"{"reason":"withdrew 100 USDT"}"#;

/// v16 §C8: with the router's authentication unconfigured every request
/// reaches the handler, which still answers 401 without an identity, before
/// the body or the service, and never runs the re-anchor.
#[tokio::test]
async fn cash_reanchor_requires_identity_without_router_auth() {
    let venue = Arc::new(Silent::default());
    let app = router(None).await.layer(Extension(service(Arc::clone(&venue), false)));
    for body in [BODY, "not json", r#"{"reason":"  "}"#] {
        let (status, reply) = post(&app, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}: {reply}");
        assert_eq!(reply["error"], "a verified identity is required");
    }
    assert_eq!(venue.0.load(Ordering::Relaxed), 0);
}

/// Review r1: the identity comes before any of the body is read, so an
/// anonymous request whose body never ends is still answered 401 at once.
#[tokio::test]
async fn cash_reanchor_answers_401_before_reading_the_body() {
    let venue = Arc::new(Silent::default());
    let app = router(None).await.layer(Extension(service(Arc::clone(&venue), false)));
    let stalled = Body::from_stream(async_stream::stream! {
        yield Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{\"reason\":"));
        std::future::pending::<()>().await;
    });
    let request = Request::post("/venue/cash-reanchor")
        .header("content-type", "application/json")
        .body(stalled)
        .unwrap();
    let answered = tokio::time::timeout(Duration::from_secs(5), app.oneshot(request)).await;
    let response = answered.expect("answered without the body").unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(venue.0.load(Ordering::Relaxed), 0);
}

/// An identity with a blank, oversized or unreadable reason is a 400; a
/// valid one on a daemon without the composed service is a 503.
#[tokio::test]
async fn cash_reanchor_needs_a_reason_and_the_composed_service() {
    let app = router(Some(TOKEN)).await;
    let long = serde_json::json!({ "reason": "x".repeat(2001) }).to_string();
    for body in [
        r#"{"reason":"   "}"#,
        long.as_str(),
        "not json",
        r#"{"why":"x"}"#,
    ] {
        assert_eq!(post(&app, Some(TOKEN), body).await.0, StatusCode::BAD_REQUEST, "{body}");
    }
    let edge = serde_json::json!({ "reason": "x".repeat(2000) }).to_string();
    for body in [BODY, edge.as_str()] {
        let (status, reply) = post(&app, Some(TOKEN), body).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{reply}");
        assert_eq!(reply["error"], "cash re-anchor is not available");
    }
}

/// One flight at a time (409 `busy`) and read-only mode (409 `read_only`),
/// both before any venue request or statement.
#[tokio::test]
async fn cash_reanchor_is_single_flight_and_refuses_read_only() {
    let venue = Arc::new(Silent::default());
    let busy = service(Arc::clone(&venue), false);
    let app = router(Some(TOKEN)).await.layer(Extension(Arc::clone(&busy)));
    let held = busy.flight.lock().await;
    assert_eq!(
        post(&app, Some(TOKEN), BODY).await,
        (StatusCode::CONFLICT, serde_json::json!({ "error": "busy" }))
    );
    drop(held);
    let read_only = router(Some(TOKEN)).await.layer(Extension(service(Arc::clone(&venue), true)));
    assert_eq!(
        post(&read_only, Some(TOKEN), BODY).await,
        (StatusCode::CONFLICT, serde_json::json!({ "error": "read_only" }))
    );
    assert_eq!(venue.0.load(Ordering::Relaxed), 0);
}

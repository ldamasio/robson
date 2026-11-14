use http_body_util::BodyExt;
use robson_exec::venue::AccountKind;

use super::*;
use crate::capability::worker_venue::{immediately, manager, NoAccessStore, WorkerVenue};

async fn state(
    kind: AccountKind,
    route: FundingRouteMode,
) -> Arc<ApiState<WorkerVenue, NoAccessStore>> {
    let exchange = Arc::new(WorkerVenue::new(kind, true));
    let position_manager = manager(&exchange, Arc::new(NoAccessStore));
    let held = position_manager.read().await;
    let circuit_breaker = held.circuit_breaker();
    let order_journal = held.order_journal();
    drop(held);
    #[cfg(feature = "postgres")]
    let pool = crate::capability::worker_venue::disconnected_pool();
    #[cfg(feature = "postgres")]
    pool.close().await;
    Arc::new(ApiState {
        exchange,
        position_manager,
        event_bus: Arc::new(EventBus::new(8)),
        circuit_breaker,
        wallet_balance_cache: Mutex::new(None),
        #[cfg(feature = "postgres")]
        pg_pool: Some(Arc::new(pool)),
        #[cfg(feature = "postgres")]
        tenant_id: Some(Uuid::now_v7()),
        google_auth: None,
        legacy_api_token: None,
        funding: FundingConfig {
            enabled: true,
            ..FundingConfig::default()
        },
        funding_route_mode: route,
        venue_feed: VenueFeedView::default(),
        order_journal,
    })
}

fn recovery(dry_run: bool) -> FundingRecoverSpotUsdtRequest {
    FundingRecoverSpotUsdtRequest {
        asset: "USDT".into(),
        amount: Some(Decimal::ONE),
        dry_run,
        execute: !dry_run,
        confirm: Some("TRANSFER_SPOT_USDT_TO_FUTURES".into()),
        correlation_id: Some("account-gate-fixture".into()),
    }
}

async fn refused(response: impl IntoResponse, operation: &str) {
    let response = response.into_response();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["error"],
        format!("Configuration error: {operation} requires a position account")
    );
}

#[tokio::test]
async fn balance_funding_api_and_direct_manual_refuse_before_permit_or_io() {
    for route in [
        FundingRouteMode::Selected,
        FundingRouteMode::Recovery,
        FundingRouteMode::Blocked,
    ] {
        for closed_gate in [false, true] {
            let state = state(AccountKind::BalanceAccount, route).await;
            if closed_gate {
                state.order_journal.gate().begin_closing();
            }
            let held = state.position_manager.write().await;
            let before = held.configured_capital();
            let mut headers = HeaderMap::new();
            headers.insert("Idempotency-Key", "account-gate-fixture".parse().unwrap());
            refused(immediately(funding_quote_handler(State(state.clone()))).await, "funding")
                .await;
            refused(
                immediately(funding_execute_handler(
                    State(state.clone()),
                    headers.clone(),
                    Json(ExecuteFundingRequest { quote_id: Uuid::now_v7() }),
                ))
                .await,
                "funding",
            )
            .await;
            #[cfg(feature = "postgres")]
            refused(
                immediately(funding_execute(
                    State(state.clone()),
                    headers,
                    Json(ExecuteFundingRequest { quote_id: Uuid::now_v7() }),
                ))
                .await,
                "funding",
            )
            .await;
            for dry in [true, false] {
                refused(
                    immediately(funding_recover_spot_usdt_to_futures_handler(
                        State(state.clone()),
                        Json(recovery(dry)),
                    ))
                    .await,
                    "funding",
                )
                .await;
                refused(
                    immediately(recover_spot_usdt(State(state.clone()), Json(recovery(dry)))).await,
                    "funding",
                )
                .await;
            }
            assert_eq!(state.exchange.requests(), 0);
            assert_eq!(state.exchange.funding_accesses(), 0);
            assert_eq!(state.order_journal.gate().snapshot().permits, 0);
            assert_eq!(held.configured_capital(), before);
        }
    }
}

#[tokio::test]
async fn balance_capital_refresh_api_and_helper_refuse_before_lock_and_read() {
    let state = state(AccountKind::BalanceAccount, FundingRouteMode::Selected).await;
    let held = state.position_manager.write().await;
    let before = held.configured_capital();
    refused(
        immediately(capital_refresh_handler(State(state.clone()))).await,
        "capital refresh",
    )
    .await;
    assert!(
        matches!(immediately(refresh_capital_from_exchange(&state.exchange, &state.position_manager)).await,
        Err(DaemonError::Config(message)) if message == "capital refresh requires a position account")
    );
    assert_eq!(state.exchange.requests(), 0);
    assert_eq!(held.configured_capital(), before);
}

#[tokio::test]
async fn position_capital_refresh_keeps_success_and_server_error_mapping() {
    let state = state(AccountKind::PositionAccount, FundingRouteMode::Selected).await;
    let response = capital_refresh_handler(State(state.clone())).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.exchange.requests(), 1);
    assert_eq!(state.position_manager.read().await.configured_capital(), Decimal::from(12000));
    state.exchange.delegate.set_futures_balance(Decimal::ZERO);
    let response = capital_refresh_handler(State(state.clone())).await.into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(state.exchange.requests(), 2);
    assert_eq!(state.position_manager.read().await.configured_capital(), Decimal::from(12000));
}

#[cfg(feature = "postgres")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn balance_funding_http_routes_send_zero_physical_sql(pool: sqlx::PgPool) {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let (proxy, proxied) = crate::capability::worker_venue::counted_pool(&pool).await;
    let mut state = state(AccountKind::BalanceAccount, FundingRouteMode::Recovery).await;
    Arc::get_mut(&mut state).unwrap().pg_pool = Some(Arc::new(proxied.clone()));
    state.order_journal.gate().begin_closing();
    let app = create_router(state.clone());
    let held = state.position_manager.write().await;
    let before_capital = held.configured_capital();
    proxy.enable_query_trace(16);
    let before = proxy.stats().statements;
    for (path, body, operation) in [
        ("/funding/quote", serde_json::json!({}), "funding"),
        ("/funding/execute", serde_json::json!({"quote_id":Uuid::now_v7()}), "funding"),
        (
            "/funding/recover-spot-usdt-to-futures",
            serde_json::json!({"asset":"USDT", "dry_run":true}),
            "funding",
        ),
        (
            "/funding/recover-spot-usdt-to-futures",
            serde_json::json!({"asset":"USDT", "dry_run":false, "execute":true, "confirm":"TRANSFER_SPOT_USDT_TO_FUTURES"}),
            "funding",
        ),
        ("/capital/refresh", serde_json::json!({}), "capital refresh"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "fixture-http")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        refused(response, operation).await;
    }
    let ready = proxied.acquire().await.unwrap();
    assert_eq!(proxy.stats().statements - before, 0);
    assert!(proxy.query_trace().is_empty());
    assert_eq!(state.exchange.requests(), 0);
    assert_eq!(state.exchange.funding_accesses(), 0);
    assert_eq!(held.configured_capital(), before_capital);
    assert_eq!(state.order_journal.gate().snapshot().permits, 0);
    drop(ready);
    drop(held);
    drop(app);
    drop(state);
    proxied.close().await;
}

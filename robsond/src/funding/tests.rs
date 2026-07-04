use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{OrderSide, Price, Quantity, RiskConfig, Side, Symbol, TradingPolicy};
use robson_engine::Engine;
use robson_exec::{
    ExchangePort, ExchangePosition, ExecError, Executor, FuturesBalance, FuturesSettings,
    IntentJournal, OrderResult, SpotBalance, SpotOrder, SpotOrderRequest, StubExchange, Transfer,
    TransferId, UniversalTransferType, UserTradeRecord,
};
use robson_store::{MemoryStore, Store};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sqlx::PgPool;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{FundingService, FundingState};
use crate::{
    config::FundingConfig, event_bus::EventBus, position_manager::PositionManager,
    query_engine::TracingQueryRecorder,
};

struct TestFundingService<E: ExchangePort + 'static, S: Store + 'static> {
    exchange: Arc<E>,
    position_manager: Arc<RwLock<PositionManager<E, S>>>,
    service: FundingService<E, S>,
}

struct PostConvertedCrashExchange {
    inner: Arc<StubExchange>,
    spot_balance_calls: std::sync::RwLock<u64>,
    fail_spot_balance_call: std::sync::RwLock<Option<u64>>,
    fail_spot_order: std::sync::RwLock<Option<String>>,
    fail_transfers_remaining: std::sync::RwLock<u64>,
}

impl PostConvertedCrashExchange {
    fn new(inner: Arc<StubExchange>) -> Self {
        Self {
            inner,
            spot_balance_calls: std::sync::RwLock::new(0),
            fail_spot_balance_call: std::sync::RwLock::new(None),
            fail_spot_order: std::sync::RwLock::new(None),
            fail_transfers_remaining: std::sync::RwLock::new(0),
        }
    }

    fn fail_next_transfers(&self, count: u64) {
        *self.fail_transfers_remaining.write().unwrap() = count;
    }

    fn fail_spot_balance_call(&self, call: u64) {
        *self.spot_balance_calls.write().unwrap() = 0;
        *self.fail_spot_balance_call.write().unwrap() = Some(call);
    }

    fn fail_spot_order(&self, message: &str) {
        *self.fail_spot_order.write().unwrap() = Some(message.to_string());
    }

    fn spot_order_call_count(&self) -> u64 {
        self.inner.spot_order_call_count()
    }

    fn transfer_call_count(&self) -> u64 {
        self.inner.transfer_call_count()
    }
}

fn test_service<E: ExchangePort + 'static>(
    pool: PgPool,
    exchange: Arc<E>,
    config: FundingConfig,
) -> TestFundingService<E, MemoryStore> {
    let store = Arc::new(MemoryStore::new());
    let executor = Arc::new(Executor::new(
        Arc::clone(&exchange),
        Arc::new(IntentJournal::new()),
        store.clone(),
    ));
    let manager = PositionManager::new(
        Engine::new(RiskConfig::new(dec!(10000)).unwrap()),
        executor,
        store,
        Arc::new(EventBus::new(1000)),
        Arc::new(TracingQueryRecorder),
        TradingPolicy::default(),
    );
    let position_manager = Arc::new(RwLock::new(manager));
    let service = FundingService::new(
        Arc::new(pool),
        Uuid::new_v4(),
        Arc::clone(&exchange),
        Arc::clone(&position_manager),
        config,
    );

    TestFundingService { exchange, position_manager, service }
}

fn configured_exchange() -> Arc<StubExchange> {
    let exchange = Arc::new(StubExchange::new(dec!(50000)));
    exchange.set_futures_balance(dec!(10000));
    exchange.set_spot_balance("BTC", dec!(0.01), Decimal::ZERO);
    exchange.set_price("BTCUSDT", dec!(50000));
    exchange
}

#[async_trait]
impl ExchangePort for PostConvertedCrashExchange {
    async fn validate_futures_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<FuturesSettings, ExecError> {
        self.inner.validate_futures_settings(symbol, expected_leverage).await
    }

    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        reduce_only: bool,
    ) -> Result<OrderResult, ExecError> {
        self.inner
            .place_market_order(symbol, side, quantity, client_order_id, reduce_only)
            .await
    }

    async fn place_stop_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        stop_price: Price,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        self.inner
            .place_stop_market_order(symbol, side, quantity, stop_price, client_order_id)
            .await
    }

    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        self.inner.cancel_order(symbol, order_id).await
    }

    async fn cancel_stop_market_order(
        &self,
        symbol: &Symbol,
        algo_id: &str,
    ) -> Result<(), ExecError> {
        self.inner.cancel_stop_market_order(symbol, algo_id).await
    }

    async fn get_open_orders(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<robson_exec::OpenOrderRecord>, ExecError> {
        self.inner.get_open_orders(symbol).await
    }

    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        self.inner.get_price(symbol).await
    }

    async fn health_check(&self) -> Result<(), ExecError> {
        self.inner.health_check().await
    }

    async fn get_futures_balance(&self) -> Result<FuturesBalance, ExecError> {
        self.inner.get_futures_balance().await
    }

    async fn get_spot_account_balances(&self) -> Result<Vec<SpotBalance>, ExecError> {
        let call = {
            let mut calls = self.spot_balance_calls.write().unwrap();
            *calls += 1;
            *calls
        };
        if self.fail_spot_balance_call.read().unwrap().is_some_and(|fail| fail == call) {
            *self.fail_spot_balance_call.write().unwrap() = None;
            return Err(ExecError::Exchange("Simulated spot balance failure".to_string()));
        }
        self.inner.get_spot_account_balances().await
    }

    async fn get_spot_price(&self, symbol: &str) -> Result<Price, ExecError> {
        self.inner.get_spot_price(symbol).await
    }

    async fn spot_symbol_is_trading(&self, symbol: &str) -> Result<bool, ExecError> {
        self.inner.spot_symbol_is_trading(symbol).await
    }

    async fn place_spot_market_order(
        &self,
        request: SpotOrderRequest,
    ) -> Result<SpotOrder, ExecError> {
        let failure = self.fail_spot_order.write().unwrap().take();
        if let Some(message) = failure {
            return Err(ExecError::OrderRejected(message));
        }
        self.inner.place_spot_market_order(request).await
    }

    async fn get_spot_order(
        &self,
        symbol: &str,
        client_order_id: &str,
    ) -> Result<Option<SpotOrder>, ExecError> {
        self.inner.get_spot_order(symbol, client_order_id).await
    }

    async fn universal_transfer(
        &self,
        asset: &str,
        amount: Decimal,
        transfer_type: UniversalTransferType,
        client_tran_key: &str,
    ) -> Result<TransferId, ExecError> {
        {
            let mut remaining = self.fail_transfers_remaining.write().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                return Err(ExecError::Exchange("transfer temporarily unavailable".to_string()));
            }
        }
        self.inner
            .universal_transfer(asset, amount, transfer_type, client_tran_key)
            .await
    }

    async fn get_transfer_history(
        &self,
        transfer_type: UniversalTransferType,
        start_time: DateTime<Utc>,
    ) -> Result<Vec<Transfer>, ExecError> {
        self.inner.get_transfer_history(transfer_type, start_time).await
    }

    async fn get_all_open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError> {
        self.inner.get_all_open_positions().await
    }

    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: Side,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        self.inner.close_position_market(symbol, side, quantity, client_order_id).await
    }

    async fn get_order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.inner.get_order_by_exchange_id(symbol, order_id).await
    }

    async fn get_stop_order_fill(
        &self,
        symbol: &Symbol,
        algo_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.inner.get_stop_order_fill(symbol, algo_id).await
    }

    async fn get_user_trades_since(
        &self,
        symbol: &Symbol,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        self.inner.get_user_trades_since(symbol, since, limit).await
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL"]
async fn happy_path(pool: PgPool) -> anyhow::Result<()> {
    let harness = test_service(pool, configured_exchange(), FundingConfig::default());

    let quote = harness.service.quote().await?;
    let response = harness.service.execute(quote.quote_id, "happy-path").await?;

    assert_eq!(response.state, FundingState::Refreshed.as_str());
    assert_eq!(harness.exchange.spot_order_call_count(), 1);
    assert_eq!(harness.exchange.transfer_call_count(), 1);

    let capital = harness.service.refresh_capital().await?;
    assert_eq!(capital, dec!(10499.50000));
    let engine_capital = harness.position_manager.read().await.engine().risk_config().capital();
    assert_eq!(engine_capital, capital);

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL"]
async fn inverse_pair_brl_to_usdt(pool: PgPool) -> anyhow::Result<()> {
    let exchange = Arc::new(StubExchange::new(dec!(50000)));
    exchange.set_futures_balance(dec!(10000));
    exchange.set_spot_balance("BRL", dec!(1000), Decimal::ZERO);
    exchange.set_price("USDTBRL", dec!(5.0));
    exchange.set_trading_symbols(&["USDTBRL"]);
    let harness = test_service(pool, exchange, FundingConfig::default());

    let quote = harness.service.quote().await?;

    assert_eq!(quote.items.len(), 1);
    assert_eq!(quote.items[0].asset, "BRL");
    assert_eq!(quote.items[0].symbol, "USDTBRL");
    assert_eq!(quote.items[0].qty, dec!(1000));
    assert_eq!(quote.items[0].est_usdt, dec!(199.4000));

    let response = harness.service.execute(quote.quote_id, "inverse-brl").await?;

    assert_eq!(response.state, FundingState::Refreshed.as_str());
    assert_eq!(harness.exchange.spot_order_call_count(), 1);
    assert_eq!(harness.exchange.transfer_call_count(), 1);

    let capital = harness.service.refresh_capital().await?;
    assert_eq!(capital, dec!(10199.800));

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL"]
async fn idempotent_convert_no_double_execute(pool: PgPool) -> anyhow::Result<()> {
    let harness = test_service(pool, configured_exchange(), FundingConfig::default());

    let quote = harness.service.quote().await?;
    let first = harness.service.execute(quote.quote_id, "idempotent").await?;
    let spot_calls = harness.exchange.spot_order_call_count();
    let transfer_call_count = harness.exchange.transfer_call_count();

    let second = harness.service.execute(quote.quote_id, "idempotent").await?;

    assert_eq!(first.state, FundingState::Refreshed.as_str());
    assert_eq!(second.state, FundingState::Refreshed.as_str());
    assert_eq!(harness.exchange.spot_order_call_count(), spot_calls);
    assert_eq!(harness.exchange.transfer_call_count(), transfer_call_count);

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL"]
async fn resume_after_crash_post_converted(pool: PgPool) -> anyhow::Result<()> {
    let exchange = Arc::new(PostConvertedCrashExchange::new(configured_exchange()));
    let harness = test_service(pool, exchange, FundingConfig::default());

    let quote = harness.service.quote().await?;
    harness.exchange.fail_spot_balance_call(2);

    let first = harness.service.execute(quote.quote_id, "resume").await;
    assert!(first.is_err());
    assert_eq!(
        harness.service.get(quote.quote_id).await?.state,
        FundingState::Converted.as_str()
    );

    let spot_calls = harness.exchange.spot_order_call_count();
    let response = harness.service.execute(quote.quote_id, "funding-worker-resume").await?;

    assert_eq!(response.state, FundingState::Refreshed.as_str());
    assert_eq!(harness.exchange.spot_order_call_count(), spot_calls);
    assert_eq!(harness.exchange.transfer_call_count(), 1);

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL"]
async fn transfer_retries_append_transfer_prepared_once(pool: PgPool) -> anyhow::Result<()> {
    // Regression for the 2026-06-03 saga: a transfer stalled for 5.5 h
    // accumulated ~4 000 TransferPrepared duplicates because every 5 s
    // worker resume re-appended the event. Resumed passes through the
    // Transferring state must NOT append another TransferPrepared.
    let exchange = Arc::new(PostConvertedCrashExchange::new(configured_exchange()));
    let harness = test_service(pool, exchange, FundingConfig::default());

    let quote = harness.service.quote().await?;
    harness.exchange.fail_next_transfers(3);

    for attempt in 0..3 {
        let result = harness.service.execute(quote.quote_id, "stalled-transfer").await;
        assert!(result.is_err(), "attempt {attempt} must fail while the transfer is down");
        assert_eq!(
            harness.service.get(quote.quote_id).await?.state,
            FundingState::Transferring.as_str()
        );
    }

    let response = harness.service.execute(quote.quote_id, "transfer-recovered").await?;
    assert_eq!(response.state, FundingState::Refreshed.as_str());

    let view = harness.service.get(quote.quote_id).await?;
    let prepared_count = view
        .events
        .iter()
        .filter(|event| event.event_type == "TransferPrepared")
        .count();
    assert_eq!(
        prepared_count, 1,
        "TransferPrepared must be appended exactly once across retries"
    );
    assert_eq!(harness.exchange.transfer_call_count(), 1);

    Ok(())
}

#[test]
fn terminal_spot_order_error_detects_insufficient_balance() {
    let error = ExecError::OrderRejected(
        "Account has insufficient balance for requested action.".to_string(),
    );

    assert_eq!(
        super::saga::terminal_spot_order_error_reason(&error),
        Some("spot_order_insufficient_balance")
    );

    let transient = ExecError::Exchange("temporary gateway timeout".to_string());
    assert_eq!(super::saga::terminal_spot_order_error_reason(&transient), None);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL"]
async fn insufficient_balance_marks_saga_failed(pool: PgPool) -> anyhow::Result<()> {
    let exchange = Arc::new(PostConvertedCrashExchange::new(configured_exchange()));
    exchange.fail_spot_order("Account has insufficient balance for requested action.");
    let harness = test_service(pool, exchange, FundingConfig::default());

    let quote = harness.service.quote().await?;
    let error = harness
        .service
        .execute(quote.quote_id, "insufficient-balance")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("insufficient balance"));
    let view = harness.service.get(quote.quote_id).await?;
    assert_eq!(view.state, FundingState::Failed.as_str());
    assert!(view.events.iter().any(|event| {
        event.event_type == "FundingFailed"
            && event.payload.get("reason").and_then(|reason| reason.as_str())
                == Some("spot_order_insufficient_balance:BTC")
    }));
    assert_eq!(harness.service.resume_non_terminal().await?, 0);

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL"]
async fn expired_quote_rejected(pool: PgPool) -> anyhow::Result<()> {
    let config = FundingConfig {
        quote_ttl_secs: 0,
        ..FundingConfig::default()
    };
    let harness = test_service(pool, configured_exchange(), config);

    let quote = harness.service.quote().await?;
    let error = harness.service.execute(quote.quote_id, "expired").await.unwrap_err();
    let view = harness.service.get(quote.quote_id).await?;

    assert!(error.to_string().contains("quote_expired"));
    assert_eq!(view.state, FundingState::Failed.as_str());
    assert!(view.events.iter().any(|event| {
        event.event_type == "FundingFailed"
            && event.payload.get("reason").and_then(|reason| reason.as_str())
                == Some("quote_expired")
    }));
    assert_eq!(harness.exchange.spot_order_call_count(), 0);

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "Requires DATABASE_URL"]
async fn dust_skipped(pool: PgPool) -> anyhow::Result<()> {
    let exchange = configured_exchange();
    exchange.set_spot_balance("ETH", dec!(0.0001), Decimal::ZERO);
    exchange.set_price("ETHUSDT", dec!(1000));
    let harness = test_service(pool, exchange, FundingConfig::default());

    let quote = harness.service.quote().await?;

    assert_eq!(quote.items.len(), 1);
    assert_eq!(quote.items[0].asset, "BTC");
    assert!(!quote.items.iter().any(|item| item.asset == "ETH"));

    let response = harness.service.execute(quote.quote_id, "dust").await?;
    assert_eq!(response.state, FundingState::Refreshed.as_str());
    assert_eq!(harness.exchange.spot_order_call_count(), 1);

    Ok(())
}

//! Legacy port compatibility layer (ADR-0056 §2, step 2).
//!
//! [`ExchangePort`] and [`IncomePort`] are kept for one release as a
//! **forwarding** layer over the venue contracts in [`crate::venue`]: every
//! [`Venue`] implements both through the blanket impls below. Each legacy
//! method makes exactly one call on the new surface and maps its result
//! without changing it, so callers (executor, daemon, workers) see the same
//! behaviour and the same venue request count.
//!
//! Legacy methods that belong to an optional or account-kind surface
//! (position account, funding) fail with `ExecError::Config` on a venue
//! without that accessor; no venue in this step lacks them.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_domain::{OrderSide, Price, Quantity, Side, Symbol, SymbolTradingRules};
use rust_decimal::Decimal;

use crate::{
    error::ExecError,
    ports::{
        self, ExchangePort, ExchangePosition, FuturesBalance, FuturesSettings, IncomePort,
        IncomeRecord, OpenOrderRecord, OrderResult, SpotBalance, SpotOrder, SpotOrderRequest,
        Transfer, TransferId, UniversalTransferType, UserTradeRecord,
    },
    venue::{
        EntryOrReduce, FundingPort, PositionAccountPort, StopCancelOutcome, StopRef, TradingPort,
        Venue,
    },
};

fn position_account<V: Venue + ?Sized>(venue: &V) -> Result<&dyn PositionAccountPort, ExecError> {
    venue
        .position_account()
        .ok_or_else(|| ExecError::Config(format!("venue {} has no position account", venue.id())))
}

fn funding<V: Venue + ?Sized>(venue: &V) -> Result<&dyn FundingPort, ExecError> {
    venue
        .funding()
        .ok_or_else(|| ExecError::Config(format!("venue {} has no funding surface", venue.id())))
}

/// Map the venue cancel outcome onto the legacy two-variant outcome.
///
/// `Unknown` has no legacy variant; it becomes `ExecError::Timeout`, which
/// the executor's unknown-order classification does not match, so an
/// indeterminate cancel is never recorded as a cancellation. No adapter
/// returns `Unknown` in this step (request failures are still `Err`).
pub(crate) fn legacy_cancel_outcome(
    outcome: StopCancelOutcome,
) -> Result<ports::StopCancelOutcome, ExecError> {
    match outcome {
        StopCancelOutcome::Cancelled { .. } => Ok(ports::StopCancelOutcome::Cancelled),
        StopCancelOutcome::AlreadyGone { .. } => Ok(ports::StopCancelOutcome::AlreadyGone),
        StopCancelOutcome::Unknown => Err(ExecError::Timeout(
            "protective stop cancel outcome is indeterminate".to_string(),
        )),
    }
}

#[async_trait]
impl<V: Venue> ExchangePort for V {
    async fn validate_futures_settings(
        &self,
        symbol: &Symbol,
        expected_leverage: u8,
    ) -> Result<FuturesSettings, ExecError> {
        position_account(self)?
            .validate_position_settings(symbol, expected_leverage)
            .await
    }

    async fn trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        TradingPort::trading_rules(self, symbol).await
    }

    async fn trading_rules_preview(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        TradingPort::trading_rules_preview(self, symbol).await
    }

    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        reduce_only: bool,
    ) -> Result<OrderResult, ExecError> {
        TradingPort::place_market_order(
            self,
            symbol,
            side,
            quantity,
            client_order_id,
            EntryOrReduce::from_reduce_only(reduce_only),
        )
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
        self.place_protection(symbol, side, quantity, stop_price, client_order_id)
            .await
            .map(|placed| placed.acceptance)
    }

    async fn cancel_stop_market_order(
        &self,
        symbol: &Symbol,
        algo_id: &str,
    ) -> Result<ports::StopCancelOutcome, ExecError> {
        legacy_cancel_outcome(self.cancel_protection(symbol, &StopRef::new(algo_id)).await?)
    }

    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        TradingPort::cancel_order(self, symbol, order_id).await
    }

    async fn get_open_orders(&self, symbol: &Symbol) -> Result<Vec<OpenOrderRecord>, ExecError> {
        self.open_protective_orders(symbol).await
    }

    async fn get_price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        self.price(symbol).await
    }

    async fn health_check(&self) -> Result<(), ExecError> {
        self.health().await
    }

    async fn get_futures_balance(&self) -> Result<FuturesBalance, ExecError> {
        position_account(self)?.balance().await
    }

    async fn get_spot_account_balances(&self) -> Result<Vec<SpotBalance>, ExecError> {
        funding(self)?.spot_balances().await
    }

    async fn get_spot_price(&self, symbol: &str) -> Result<Price, ExecError> {
        funding(self)?.spot_price(symbol).await
    }

    async fn spot_symbol_is_trading(&self, symbol: &str) -> Result<bool, ExecError> {
        funding(self)?.spot_symbol_is_trading(symbol).await
    }

    async fn place_spot_market_order(
        &self,
        request: SpotOrderRequest,
    ) -> Result<SpotOrder, ExecError> {
        funding(self)?.place_spot_market_order(request).await
    }

    async fn get_spot_order(
        &self,
        symbol: &str,
        client_order_id: &str,
    ) -> Result<Option<SpotOrder>, ExecError> {
        funding(self)?.spot_order(symbol, client_order_id).await
    }

    async fn universal_transfer(
        &self,
        asset: &str,
        amount: Decimal,
        transfer_type: UniversalTransferType,
        client_tran_key: &str,
    ) -> Result<TransferId, ExecError> {
        funding(self)?.transfer(asset, amount, transfer_type, client_tran_key).await
    }

    async fn get_transfer_history(
        &self,
        transfer_type: UniversalTransferType,
        start_time: DateTime<Utc>,
    ) -> Result<Vec<Transfer>, ExecError> {
        funding(self)?.transfer_history(transfer_type, start_time).await
    }

    async fn get_all_open_positions(&self) -> Result<Vec<ExchangePosition>, ExecError> {
        position_account(self)?.open_positions().await
    }

    async fn close_position_market(
        &self,
        symbol: &Symbol,
        side: Side,
        quantity: Quantity,
        client_order_id: &str,
    ) -> Result<OrderResult, ExecError> {
        position_account(self)?
            .close_position_market(symbol, side, quantity, client_order_id)
            .await
    }

    async fn get_order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.order_by_exchange_id(symbol, order_id).await
    }

    async fn get_stop_order_fill(
        &self,
        symbol: &Symbol,
        algo_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        self.protection_fill(symbol, &StopRef::new(algo_id)).await
    }

    async fn get_user_trades_since(
        &self,
        symbol: &Symbol,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        self.user_trades_since(symbol, since, limit).await
    }
}

#[async_trait]
impl<V: Venue> IncomePort for V {
    async fn get_income_since(
        &self,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<IncomeRecord>, ExecError> {
        self.settlement().settlement_since(since, limit).await
    }
}

#[cfg(test)]
mod tests {
    //! Forwarding equals the direct venue call on `StubExchange`.

    use robson_domain::Side;
    use rust_decimal_macros::dec;

    use super::*;
    use crate::{
        stub::{StubExchange, STUB_FINALITY},
        venue::{
            self, AccountKind, AccountPort, FundingPort, MarketKind, OrderLookup,
            PositionAccountPort, ProtectionKind, ReplaceOutcome, ReplacementProtocol,
            SettlementPort, StopParams,
        },
    };

    fn btc() -> Symbol {
        Symbol::from_pair("BTCUSDT").unwrap()
    }

    fn qty() -> Quantity {
        Quantity::new(dec!(0.01)).unwrap()
    }

    fn px(value: Decimal) -> Price {
        Price::new(value).unwrap()
    }

    fn stub() -> StubExchange {
        StubExchange::new(dec!(95000))
    }

    /// Everything but the local-clock timestamp.
    fn key(o: &OrderResult) -> (String, String, Price, Quantity, Decimal, String) {
        let o = o.clone();
        (
            o.exchange_order_id,
            o.client_order_id,
            o.fill_price,
            o.filled_quantity,
            o.fee,
            o.fee_asset,
        )
    }

    #[tokio::test]
    async fn orders_and_stops_forward_unchanged() {
        let sym = btc();
        let (legacy, direct) = (stub(), stub());
        let a = ExchangePort::place_market_order(&legacy, &sym, OrderSide::Buy, qty(), "c1", false);
        let b = TradingPort::place_market_order(
            &direct,
            &sym,
            OrderSide::Buy,
            qty(),
            "c1",
            EntryOrReduce::Entry,
        );
        assert_eq!(key(&a.await.unwrap()), key(&b.await.unwrap()));

        // reduce_only = true reaches the venue as Reduce.
        legacy.set_reject_reduce_only_next(true);
        direct.set_reject_reduce_only_next(true);
        let a = ExchangePort::place_market_order(&legacy, &sym, OrderSide::Sell, qty(), "c2", true);
        let b = TradingPort::place_market_order(
            &direct,
            &sym,
            OrderSide::Sell,
            qty(),
            "c2",
            EntryOrReduce::Reduce,
        );
        assert_eq!(a.await.unwrap_err().to_string(), b.await.unwrap_err().to_string());

        let trigger = px(dec!(90000));
        let a = legacy.place_stop_market_order(&sym, OrderSide::Sell, qty(), trigger, "ins-1");
        let b = direct.place_protection(&sym, OrderSide::Sell, qty(), trigger, "ins-1");
        let (a, b) = (a.await.unwrap(), b.await.unwrap());
        assert_eq!(key(&a), key(&b.acceptance));
        assert_eq!(b.stop.exchange_order_id, a.exchange_order_id);
        assert_eq!((a.fill_price, a.filled_quantity), (trigger, Quantity::zero()));

        let id = a.exchange_order_id.as_str();
        assert_eq!(
            legacy.cancel_stop_market_order(&sym, id).await.unwrap(),
            ports::StopCancelOutcome::Cancelled
        );
        assert_eq!(
            legacy.cancel_stop_market_order(&sym, id).await.unwrap(),
            ports::StopCancelOutcome::AlreadyGone
        );
        assert!(matches!(
            direct.cancel_protection(&sym, &StopRef::new(id)).await.unwrap(),
            StopCancelOutcome::Cancelled { filled_before_cancel, .. } if filled_before_cancel.is_zero()
        ));
        assert!(matches!(
            direct.cancel_protection(&sym, &StopRef::new(id)).await.unwrap(),
            StopCancelOutcome::AlreadyGone { final_state: OrderLookup::Unknown }
        ));
    }

    #[test]
    fn indeterminate_cancel_is_never_a_legacy_success() {
        assert!(matches!(
            legacy_cancel_outcome(StopCancelOutcome::Unknown),
            Err(ExecError::Timeout(_))
        ));
    }

    #[tokio::test]
    async fn account_and_evidence_queries_forward_unchanged() {
        let sym = btc();
        let s = StubExchange::with_balance(dec!(95000), dec!(1234));
        s.set_unrealized_pnl(dec!(-7.5));
        s.set_open_position(btc(), Side::Long, qty(), px(dec!(90000)));
        s.set_spot_balance("USDT", dec!(50), dec!(1));
        let fill = OrderResult {
            exchange_order_id: "42".to_string(),
            client_order_id: "c42".to_string(),
            fill_price: px(dec!(95000)),
            filled_quantity: qty(),
            fee: dec!(0.95),
            fee_asset: "USDT".to_string(),
            filled_at: Utc::now(),
        };
        s.set_order_result("42", fill);

        let (legacy, direct) = (s.get_futures_balance().await.unwrap(), s.balance().await.unwrap());
        assert_eq!((legacy.wallet_balance, legacy.unrealized_pnl), (dec!(1234), dec!(-7.5)));
        assert_eq!(
            (legacy.wallet_balance, legacy.available_balance, legacy.unrealized_pnl),
            (direct.wallet_balance, direct.available_balance, direct.unrealized_pnl)
        );
        assert_eq!(s.get_all_open_positions().await.unwrap(), s.open_positions().await.unwrap());
        let legacy = s.validate_futures_settings(&sym, 1).await.unwrap();
        let direct = s.validate_position_settings(&sym, 1).await.unwrap();
        assert_eq!(
            (legacy.position_mode, legacy.leverage),
            (direct.position_mode, direct.leverage)
        );

        let legacy = s.get_order_by_exchange_id(&sym, "42").await.unwrap().unwrap();
        let direct = s.order_by_exchange_id(&sym, "42").await.unwrap().unwrap();
        assert_eq!(key(&legacy), key(&direct));
        let legacy = s.get_stop_order_fill(&sym, "42").await.unwrap().unwrap();
        let direct = s.protection_fill(&sym, &StopRef::new("42")).await.unwrap().unwrap();
        assert_eq!(key(&legacy), key(&direct));

        let since = Utc::now() - chrono::Duration::hours(1);
        assert_eq!(
            s.get_income_since(since, 10).await.unwrap(),
            s.settlement_since(since, 10).await.unwrap()
        );
        assert_eq!(
            s.get_open_orders(&sym).await.unwrap(),
            s.open_protective_orders(&sym).await.unwrap()
        );
        assert_eq!(s.get_price(&sym).await.unwrap(), TradingPort::price(&s, &sym).await.unwrap());
        assert_eq!(s.get_spot_account_balances().await.unwrap(), s.spot_balances().await.unwrap());
        assert_eq!(
            ExchangePort::spot_symbol_is_trading(&s, "BTCUSDT").await.unwrap(),
            FundingPort::spot_symbol_is_trading(&s, "BTCUSDT").await.unwrap()
        );

        s.set_futures_settings("Hedge", 1);
        let legacy = s.validate_futures_settings(&sym, 1).await.unwrap_err();
        let direct = s.validate_position_settings(&sym, 1).await.unwrap_err();
        assert_eq!(legacy.to_string(), direct.to_string());
    }

    #[tokio::test]
    async fn each_legacy_call_makes_exactly_one_venue_call() {
        let sym = btc();
        // The stub's fail flag is consumed by the first venue call.
        let s = stub();
        s.set_fail_next(true);
        assert!(s.get_futures_balance().await.is_err());
        assert!(s.get_futures_balance().await.is_ok());
        s.set_fail_next(true);
        assert!(s.cancel_stop_market_order(&sym, "missing").await.is_err());
        assert!(s.cancel_stop_market_order(&sym, "missing").await.is_ok());
    }

    #[tokio::test]
    async fn stub_capabilities_agree_with_accessors() {
        let s = stub();
        let caps = s.capabilities();
        assert_eq!(caps.market_kind, MarketKind::PerpUsdM);
        assert_eq!(caps.account_kind, AccountKind::PositionAccount);
        assert!(caps.supports_short && caps.reduce_only_wire_flag && caps.has_funding);
        assert_eq!(caps.protection, ProtectionKind::MarketOnTrigger);
        assert_eq!(caps.replacement, ReplacementProtocol::PlaceThenCancel);
        assert_eq!(caps.order_finality, STUB_FINALITY);
        assert_eq!(Venue::id(&s).as_str(), "stub");
        assert!(venue::capability_mismatches(&s).is_empty());
        assert!(matches!(
            venue::observe_account(&s).await.unwrap(),
            venue::AccountObservation::Positions(snap) if snap.wallet_balance == dec!(10000)
        ));
    }

    async fn lookup(s: &StubExchange, id: &str, at: Option<DateTime<Utc>>) -> OrderLookup {
        s.order_by_client_id(&btc(), id, at).await.unwrap()
    }

    #[tokio::test]
    async fn stub_order_by_client_id_applies_the_finality_rule() {
        let sym = btc();
        let s = stub();
        let entry = TradingPort::place_market_order(
            &s,
            &sym,
            OrderSide::Buy,
            qty(),
            "entry-1",
            EntryOrReduce::Entry,
        );
        entry.await.unwrap();
        s.place_protection(&sym, OrderSide::Sell, qty(), px(dec!(90000)), "ins-1")
            .await
            .unwrap();

        assert!(matches!(
            lookup(&s, "entry-1", None).await,
            OrderLookup::Terminal { filled_qty, .. } if filled_qty == dec!(0.01)
        ));
        assert!(matches!(lookup(&s, "ins-1", None).await, OrderLookup::Open { .. }));
        assert!(matches!(lookup(&s, "never", None).await, OrderLookup::Unknown));
        assert!(matches!(
            lookup(&s, "never", Some(Utc::now())).await,
            OrderLookup::Absent { .. }
        ));
    }

    #[tokio::test]
    async fn stub_replace_is_place_then_cancel() {
        let sym = btc();
        let s = stub();
        let old = s.place_protection(&sym, OrderSide::Sell, qty(), px(dec!(90000)), "ins-1");
        let old = old.await.unwrap().stop;
        let params = |id: &str| StopParams {
            close_side: OrderSide::Sell,
            quantity: qty(),
            trigger: px(dec!(91000)),
            client_order_id: id.to_string(),
        };

        // A failed placement leaves the old stop untouched.
        s.set_order_fail_next(true);
        let outcome = s.replace_protection(&sym, &old, params("ins-2")).await.unwrap();
        assert!(matches!(outcome, ReplaceOutcome::Unknown { new: None }));
        assert!(s.has_stop_order(&old.exchange_order_id));

        match s.replace_protection(&sym, &old, params("ins-3")).await.unwrap() {
            ReplaceOutcome::Replaced { new, old_filled } => {
                assert!(old_filled.is_zero());
                assert!(s.has_stop_order(&new.stop.exchange_order_id));
            },
            other => panic!("expected Replaced, got {other:?}"),
        }
        assert!(!s.has_stop_order(&old.exchange_order_id));
        assert_eq!(s.stop_order_count(), 1);
    }
}

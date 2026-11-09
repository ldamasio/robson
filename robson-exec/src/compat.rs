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

/// Map the venue cancel outcome onto the legacy port's, one to one and with
/// its payload. Nothing folds into an error: reading an outcome is
/// [`crate::cancel::classify_cancel`]'s job (ADR-0056 step 7).
pub(crate) fn legacy_cancel_outcome(outcome: StopCancelOutcome) -> ports::StopCancelOutcome {
    match outcome {
        StopCancelOutcome::Cancelled { filled_before_cancel, fills } => {
            ports::StopCancelOutcome::Cancelled { filled_before_cancel, fills }
        },
        StopCancelOutcome::AlreadyGone { final_state } => {
            ports::StopCancelOutcome::AlreadyGone { final_state }
        },
        StopCancelOutcome::Unknown => ports::StopCancelOutcome::Indeterminate,
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

    async fn trading_rules_fresh(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        TradingPort::trading_rules_fresh(self, symbol).await
    }

    fn last_validated_rules(&self, symbol: &Symbol) -> Option<SymbolTradingRules> {
        TradingPort::last_validated_rules(self, symbol)
    }

    fn rules_refresh_due(&self, symbol: &Symbol) -> bool {
        TradingPort::rules_refresh_due(self, symbol)
    }

    async fn refresh_trading_rules(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        TradingPort::refresh_trading_rules(self, symbol).await
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
        self.cancel_protection(symbol, &StopRef::new(algo_id))
            .await
            .map(legacy_cancel_outcome)
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
            SettlementPort, StopParams, VenueFinalityRule,
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
        assert!(matches!(
            legacy.cancel_stop_market_order(&sym, id).await.unwrap(),
            ports::StopCancelOutcome::Cancelled { filled_before_cancel, fills: None }
                if filled_before_cancel.is_zero()
        ));
        assert!(matches!(
            legacy.cancel_stop_market_order(&sym, id).await.unwrap(),
            ports::StopCancelOutcome::AlreadyGone { final_state: OrderLookup::Unknown }
        ));
        assert!(matches!(
            direct.cancel_protection(&sym, &StopRef::new(id)).await.unwrap(),
            StopCancelOutcome::Cancelled { filled_before_cancel, .. } if filled_before_cancel.is_zero()
        ));
        assert!(matches!(
            direct.cancel_protection(&sym, &StopRef::new(id)).await.unwrap(),
            StopCancelOutcome::AlreadyGone { final_state: OrderLookup::Unknown }
        ));
    }

    /// The way back, written here only to compare both sides: a variant or a
    /// payload lost on the way out shows up as a difference.
    fn venue_outcome_of(legacy: ports::StopCancelOutcome) -> StopCancelOutcome {
        match legacy {
            ports::StopCancelOutcome::Cancelled { filled_before_cancel, fills } => {
                StopCancelOutcome::Cancelled { filled_before_cancel, fills }
            },
            ports::StopCancelOutcome::AlreadyGone { final_state } => {
                StopCancelOutcome::AlreadyGone { final_state }
            },
            ports::StopCancelOutcome::Indeterminate => StopCancelOutcome::Unknown,
        }
    }

    #[test]
    fn the_legacy_outcome_round_trips_without_losing_anything() {
        let fill = OrderResult {
            exchange_order_id: "77".to_string(),
            client_order_id: "ins-1".to_string(),
            fill_price: Price::new(dec!(90)).unwrap(),
            filled_quantity: Quantity::new(dec!(0.1)).unwrap(),
            fee: dec!(0.01),
            fee_asset: "USDT".to_string(),
            filled_at: Utc::now(),
        };
        let gone = |final_state| StopCancelOutcome::AlreadyGone { final_state };
        for outcome in [
            StopCancelOutcome::Cancelled {
                filled_before_cancel: dec!(0.02),
                fills: Some(fill.clone()),
            },
            StopCancelOutcome::Cancelled {
                filled_before_cancel: dec!(0),
                fills: None,
            },
            gone(OrderLookup::Terminal {
                filled_qty: dec!(0.1),
                cancelled_qty: dec!(0.03),
                fills: Some(fill),
            }),
            gone(OrderLookup::Terminal {
                filled_qty: dec!(0),
                cancelled_qty: dec!(0.1),
                fills: None,
            }),
            gone(OrderLookup::Absent {
                finality: VenueFinalityRule::EveryAttemptRejected,
            }),
            gone(OrderLookup::Open { filled_qty: dec!(0.01) }),
            gone(OrderLookup::PartiallyFilled { filled_qty: dec!(0.02) }),
            gone(OrderLookup::Unknown),
            StopCancelOutcome::Unknown,
        ] {
            let before = format!("{outcome:?}");
            let after = venue_outcome_of(legacy_cancel_outcome(outcome));
            assert_eq!(format!("{after:?}"), before);
        }
    }

    /// `Unknown` used to fold into `ExecError::Timeout` here, so no reader
    /// could tell it from a request that failed.
    #[test]
    fn indeterminate_cancel_stays_a_typed_outcome() {
        assert!(matches!(
            legacy_cancel_outcome(StopCancelOutcome::Unknown),
            ports::StopCancelOutcome::Indeterminate
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

    async fn lookup(s: &StubExchange, id: &str) -> OrderLookup {
        s.order_by_client_id(&btc(), id).await.unwrap()
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
            lookup(&s, "entry-1").await,
            OrderLookup::Terminal { filled_qty, .. } if filled_qty == dec!(0.01)
        ));
        assert!(matches!(lookup(&s, "ins-1").await, OrderLookup::Open { .. }));
        // Never attempted: no evidence, so a not-found is not final.
        assert!(matches!(lookup(&s, "never").await, OrderLookup::Unknown));
        // A failure with an unknown outcome is not final.
        s.set_order_fail_next(true);
        let failed = s.place_protection(&sym, OrderSide::Sell, qty(), px(dec!(90000)), "ins-f");
        assert!(failed.await.is_err());
        assert!(matches!(lookup(&s, "ins-f").await, OrderLookup::Unknown));
        // Attempted and refused by the venue: final.
        s.set_reject_order_next(true);
        let refused = s.place_protection(&sym, OrderSide::Sell, qty(), px(dec!(90000)), "ins-x");
        assert!(refused.await.is_err());
        assert!(matches!(lookup(&s, "ins-x").await, OrderLookup::Absent { .. }));
    }

    #[tokio::test]
    async fn stub_lookup_remembers_cancelled_stops_and_closes() {
        let sym = btc();
        let s = stub();
        let placed = s.place_protection(&sym, OrderSide::Sell, qty(), px(dec!(90000)), "ins-1");
        let placed = placed.await.unwrap();
        s.cancel_protection(&sym, &placed.stop).await.unwrap();
        assert!(matches!(
            lookup(&s, "ins-1").await,
            OrderLookup::Terminal { filled_qty, cancelled_qty, fills: None }
                if filled_qty.is_zero() && cancelled_qty == dec!(0.01)
        ));

        s.set_open_position(sym.clone(), Side::Long, qty(), px(dec!(90000)));
        let closed =
            PositionAccountPort::close_position_market(&s, &sym, Side::Long, qty(), "close-1");
        let closed = closed.await.unwrap();
        assert!(matches!(
            lookup(&s, "close-1").await,
            OrderLookup::Terminal { filled_qty, fills: Some(fill), .. }
                if filled_qty == dec!(0.01) && fill.exchange_order_id == closed.exchange_order_id
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

        // A rejected retry does not resolve that earlier failure.
        s.set_reject_order_next(true);
        let outcome = s.replace_protection(&sym, &old, params("ins-2")).await.unwrap();
        assert!(matches!(outcome, ReplaceOutcome::Unknown { new: None }));
        assert!(s.has_stop_order(&old.exchange_order_id));

        // A rejection with no other attempt is definitive.
        s.set_reject_order_next(true);
        let outcome = s.replace_protection(&sym, &old, params("ins-r")).await.unwrap();
        assert!(matches!(outcome, ReplaceOutcome::OldKeptNewRejected { .. }));
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

#[cfg(test)]
mod recorded_forwarding {
    //! Every legacy method makes exactly one venue call, with its arguments
    //! unchanged, and returns that call's result.

    use std::sync::Mutex;

    use robson_domain::Side;
    use rust_decimal_macros::dec;

    use super::*;
    use crate::venue::{
        AccountPort, Cash, ExecutionCostEnvelope, Mark, OrderLookup, PlacedStop, ReplaceOutcome,
        SettlementPort, StopParams, VenueCapabilities, VenueId,
    };

    type R<T> = Result<T, ExecError>;

    struct Recorder {
        id: VenueId,
        calls: Mutex<Vec<String>>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                id: VenueId::new("rec"),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn log(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }

        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }
    }

    fn fill(id: &str) -> OrderResult {
        OrderResult {
            exchange_order_id: id.to_string(),
            client_order_id: format!("c-{id}"),
            fill_price: Price::new(dec!(1)).unwrap(),
            filled_quantity: Quantity::zero(),
            fee: Decimal::ZERO,
            fee_asset: "USDT".to_string(),
            filled_at: DateTime::from_timestamp(0, 0).unwrap(),
        }
    }

    fn rules(symbol: &Symbol) -> SymbolTradingRules {
        SymbolTradingRules::new(
            symbol.clone(),
            dec!(0.1),
            dec!(0.1),
            dec!(0.001),
            dec!(0.001),
            dec!(1000),
            dec!(5),
            1,
            3,
        )
        .unwrap()
    }

    #[async_trait]
    impl TradingPort for Recorder {
        async fn trading_rules(&self, s: &Symbol) -> R<SymbolTradingRules> {
            self.log(format!("trading_rules {}", s.as_pair()));
            Ok(rules(s))
        }
        async fn trading_rules_preview(&self, s: &Symbol) -> R<SymbolTradingRules> {
            self.log(format!("trading_rules_preview {}", s.as_pair()));
            Ok(rules(s))
        }
        async fn place_market_order(
            &self,
            s: &Symbol,
            side: OrderSide,
            q: Quantity,
            c: &str,
            i: EntryOrReduce,
        ) -> R<OrderResult> {
            self.log(format!("place_market_order {} {side:?} {q} {c} {i:?}", s.as_pair()));
            Ok(fill("m"))
        }
        async fn place_protection(
            &self,
            s: &Symbol,
            side: OrderSide,
            q: Quantity,
            t: Price,
            c: &str,
        ) -> R<PlacedStop> {
            self.log(format!("place_protection {} {side:?} {q} {t} {c}", s.as_pair()));
            Ok(PlacedStop {
                stop: StopRef::new("p"),
                acceptance: fill("p"),
            })
        }
        async fn cancel_protection(&self, s: &Symbol, stop: &StopRef) -> R<StopCancelOutcome> {
            self.log(format!("cancel_protection {} {}", s.as_pair(), stop.exchange_order_id));
            Ok(StopCancelOutcome::AlreadyGone { final_state: OrderLookup::Unknown })
        }
        async fn replace_protection(
            &self,
            _: &Symbol,
            _: &StopRef,
            _: StopParams,
        ) -> R<ReplaceOutcome> {
            unreachable!("no legacy method forwards here")
        }
        async fn cancel_order(&self, s: &Symbol, id: &str) -> R<()> {
            self.log(format!("cancel_order {} {id}", s.as_pair()));
            Ok(())
        }
        async fn order_by_client_id(&self, _: &Symbol, _: &str) -> R<OrderLookup> {
            unreachable!("no legacy method forwards here")
        }
        async fn order_by_exchange_id(&self, s: &Symbol, id: &str) -> R<Option<OrderResult>> {
            self.log(format!("order_by_exchange_id {} {id}", s.as_pair()));
            Ok(Some(fill("e")))
        }
        async fn protection_fill(&self, s: &Symbol, stop: &StopRef) -> R<Option<OrderResult>> {
            self.log(format!("protection_fill {} {}", s.as_pair(), stop.exchange_order_id));
            Ok(Some(fill("f")))
        }
        async fn open_protective_orders(&self, s: &Symbol) -> R<Vec<OpenOrderRecord>> {
            self.log(format!("open_protective_orders {}", s.as_pair()));
            Ok(Vec::new())
        }
        async fn user_trades_since(
            &self,
            s: &Symbol,
            since: DateTime<Utc>,
            limit: u16,
        ) -> R<Vec<UserTradeRecord>> {
            self.log(format!("user_trades_since {} {} {limit}", s.as_pair(), since.timestamp()));
            Ok(Vec::new())
        }
        async fn price(&self, s: &Symbol) -> R<Price> {
            self.log(format!("price {}", s.as_pair()));
            Ok(Price::new(dec!(7)).unwrap())
        }
        async fn health(&self) -> R<()> {
            self.log("health".to_string());
            Ok(())
        }
    }

    #[async_trait]
    impl AccountPort for Recorder {
        async fn cash(&self, _: &str) -> R<Cash> {
            unreachable!("no legacy method forwards here")
        }
        async fn valuation(&self, _: &Symbol) -> R<Mark> {
            unreachable!("no legacy method forwards here")
        }
        async fn execution_costs(&self, _: &Symbol) -> R<ExecutionCostEnvelope> {
            unreachable!("no legacy method forwards here")
        }
        fn capabilities(&self) -> VenueCapabilities {
            crate::stub::StubExchange::new(dec!(1)).capabilities()
        }
    }

    #[async_trait]
    impl PositionAccountPort for Recorder {
        async fn validate_position_settings(&self, s: &Symbol, lev: u8) -> R<FuturesSettings> {
            self.log(format!("validate_position_settings {} {lev}", s.as_pair()));
            Ok(FuturesSettings {
                position_mode: "One-way".into(),
                leverage: lev,
                symbol: s.as_pair(),
            })
        }
        async fn balance(&self) -> R<FuturesBalance> {
            self.log("balance".to_string());
            Ok(FuturesBalance {
                wallet_balance: dec!(3),
                available_balance: dec!(2),
                unrealized_pnl: dec!(-1),
            })
        }
        async fn open_positions(&self) -> R<Vec<ExchangePosition>> {
            self.log("open_positions".to_string());
            Ok(Vec::new())
        }
        async fn close_position_market(
            &self,
            s: &Symbol,
            side: Side,
            q: Quantity,
            c: &str,
        ) -> R<OrderResult> {
            self.log(format!("close_position_market {} {side:?} {q} {c}", s.as_pair()));
            Ok(fill("x"))
        }
    }

    #[async_trait]
    impl SettlementPort for Recorder {
        async fn settlement_since(&self, since: DateTime<Utc>, limit: u16) -> R<Vec<IncomeRecord>> {
            self.log(format!("settlement_since {} {limit}", since.timestamp()));
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl FundingPort for Recorder {
        async fn spot_balances(&self) -> R<Vec<SpotBalance>> {
            self.log("spot_balances".to_string());
            Ok(Vec::new())
        }
        async fn spot_price(&self, s: &str) -> R<Price> {
            self.log(format!("spot_price {s}"));
            Ok(Price::new(dec!(9)).unwrap())
        }
        async fn spot_symbol_is_trading(&self, s: &str) -> R<bool> {
            self.log(format!("spot_symbol_is_trading {s}"));
            Ok(true)
        }
        async fn place_spot_market_order(&self, r: SpotOrderRequest) -> R<SpotOrder> {
            self.log(format!("place_spot_market_order {r:?}"));
            Err(ExecError::Exchange("recorded".to_string()))
        }
        async fn spot_order(&self, s: &str, c: &str) -> R<Option<SpotOrder>> {
            self.log(format!("spot_order {s} {c}"));
            Ok(None)
        }
        async fn transfer(
            &self,
            a: &str,
            amount: Decimal,
            t: UniversalTransferType,
            k: &str,
        ) -> R<TransferId> {
            self.log(format!("transfer {a} {amount} {t:?} {k}"));
            Ok(TransferId("t".to_string()))
        }
        async fn transfer_history(
            &self,
            t: UniversalTransferType,
            start: DateTime<Utc>,
        ) -> R<Vec<Transfer>> {
            self.log(format!("transfer_history {t:?} {}", start.timestamp()));
            Ok(Vec::new())
        }
    }

    impl Venue for Recorder {
        fn id(&self) -> &VenueId {
            &self.id
        }
        fn position_account(&self) -> Option<&dyn PositionAccountPort> {
            Some(self)
        }
        fn inventory(&self) -> Option<&dyn crate::venue::InventoryPort> {
            None
        }
        fn settlement(&self) -> &dyn SettlementPort {
            self
        }
        fn funding(&self) -> Option<&dyn FundingPort> {
            Some(self)
        }
    }

    #[tokio::test]
    async fn every_legacy_method_forwards_one_call_with_its_arguments() {
        let v = Recorder::new();
        let s = Symbol::from_pair("BTCUSDT").unwrap();
        let q = Quantity::new(dec!(0.01)).unwrap();
        let t = Price::new(dec!(90000)).unwrap();
        let at = DateTime::from_timestamp(1_000, 0).unwrap();
        let one = |expected: &str| assert_eq!(v.take(), vec![expected.to_string()]);

        ExchangePort::trading_rules(&v, &s).await.unwrap();
        one("trading_rules BTCUSDT");
        ExchangePort::trading_rules_preview(&v, &s).await.unwrap();
        one("trading_rules_preview BTCUSDT");
        v.validate_futures_settings(&s, 1).await.unwrap();
        one("validate_position_settings BTCUSDT 1");
        ExchangePort::place_market_order(&v, &s, OrderSide::Buy, q, "c1", false)
            .await
            .unwrap();
        one("place_market_order BTCUSDT Buy 0.01 c1 Entry");
        ExchangePort::place_market_order(&v, &s, OrderSide::Sell, q, "c2", true)
            .await
            .unwrap();
        one("place_market_order BTCUSDT Sell 0.01 c2 Reduce");
        let accepted = v.place_stop_market_order(&s, OrderSide::Sell, q, t, "ins-1").await.unwrap();
        one("place_protection BTCUSDT Sell 0.01 90000 ins-1");
        assert_eq!(accepted.exchange_order_id, "p");
        let outcome = v.cancel_stop_market_order(&s, "77").await.unwrap();
        one("cancel_protection BTCUSDT 77");
        assert!(matches!(outcome, ports::StopCancelOutcome::AlreadyGone {
            final_state: OrderLookup::Unknown
        }));
        ExchangePort::cancel_order(&v, &s, "5").await.unwrap();
        one("cancel_order BTCUSDT 5");
        v.get_open_orders(&s).await.unwrap();
        one("open_protective_orders BTCUSDT");
        assert_eq!(v.get_price(&s).await.unwrap(), Price::new(dec!(7)).unwrap());
        one("price BTCUSDT");
        v.health_check().await.unwrap();
        one("health");
        let balance = v.get_futures_balance().await.unwrap();
        one("balance");
        assert_eq!(
            (balance.wallet_balance, balance.available_balance, balance.unrealized_pnl),
            (dec!(3), dec!(2), dec!(-1))
        );
        v.get_spot_account_balances().await.unwrap();
        one("spot_balances");
        assert_eq!(v.get_spot_price("BTCUSDT").await.unwrap(), Price::new(dec!(9)).unwrap());
        one("spot_price BTCUSDT");
        assert!(ExchangePort::spot_symbol_is_trading(&v, "BTCUSDT").await.unwrap());
        one("spot_symbol_is_trading BTCUSDT");
        let request = SpotOrderRequest {
            symbol: "BTCUSDT".to_string(),
            side: ports::SpotOrderSide::Buy,
            quantity_kind: ports::SpotOrderQuantity::Quote,
            quantity: dec!(10),
            client_order_id: "s1".to_string(),
        };
        let err = ExchangePort::place_spot_market_order(&v, request).await.unwrap_err();
        one("place_spot_market_order SpotOrderRequest { symbol: \"BTCUSDT\", side: Buy, \
             quantity_kind: Quote, quantity: 10, client_order_id: \"s1\" }");
        assert_eq!(err.to_string(), "Exchange error: recorded");
        v.get_spot_order("BTCUSDT", "s1").await.unwrap();
        one("spot_order BTCUSDT s1");
        let kind = UniversalTransferType::MainUmfuture;
        v.universal_transfer("USDT", dec!(5), kind, "k1").await.unwrap();
        one("transfer USDT 5 MainUmfuture k1");
        v.get_transfer_history(kind, at).await.unwrap();
        one("transfer_history MainUmfuture 1000");
        v.get_all_open_positions().await.unwrap();
        one("open_positions");
        ExchangePort::close_position_market(&v, &s, Side::Long, q, "x1").await.unwrap();
        one("close_position_market BTCUSDT Long 0.01 x1");
        assert_eq!(
            v.get_order_by_exchange_id(&s, "9").await.unwrap().unwrap().exchange_order_id,
            "e"
        );
        one("order_by_exchange_id BTCUSDT 9");
        assert_eq!(v.get_stop_order_fill(&s, "8").await.unwrap().unwrap().exchange_order_id, "f");
        one("protection_fill BTCUSDT 8");
        v.get_user_trades_since(&s, at, 50).await.unwrap();
        one("user_trades_since BTCUSDT 1000 50");
        v.get_income_since(at, 25).await.unwrap();
        one("settlement_since 1000 25");
    }
}

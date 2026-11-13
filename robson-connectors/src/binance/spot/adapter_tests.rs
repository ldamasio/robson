//! ADR-0056 step 8c-2: the spot venue's reads and refusals, on the wire
//! against a local stand-in for Binance. Nothing reaches the network.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use robson_domain::{OrderSide, Price, Quantity, Symbol};
use robson_exec::{
    venue::{
        capability_mismatches, AccountKind, AccountPort, EntryOrReduce, InventoryPort, MarketKind,
        OrderLookup, OrderObservation, OrderPhase, OrderRef, ProtectionKind, ReplacementProtocol,
        SettlementPort, StopParams, StopRef, TradingPort, Venue, VenueId,
    },
    ExecError,
};
use rust_decimal_macros::dec;
use serde_json::{json, Value};

use super::{
    stand_in::{self, Seen, Seens},
    BinanceSpotAdapter, SpotRead, SPOT_FINALITY,
};
use crate::binance_rest::BinanceRestClient;

const EXCHANGE_INFO: &str = include_str!("testdata/exchange_info_btcusdt.json");
const ACCOUNT: &str = include_str!("testdata/account.json");
const ORDER: &str = include_str!("testdata/order.json");
const OPEN_ORDER_LIST: &str = include_str!("testdata/open_order_list.json");
const MY_TRADES: &str = include_str!("testdata/my_trades.json");
const TICKER_PRICE: &str = include_str!("testdata/ticker_price.json");
const ERRORS: &str = include_str!("testdata/errors.json");

async fn venue(
    reply: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static,
) -> (BinanceSpotAdapter, Seens) {
    let (client, seen) = stand_in::serve(reply).await;
    (BinanceSpotAdapter::new(client).unwrap(), seen)
}

async fn answering(status: u16, body: impl Into<String>) -> (BinanceSpotAdapter, Seens) {
    let body = body.into();
    venue(move |_| (status, body.clone())).await
}

fn symbol(pair: &str) -> Symbol {
    Symbol::from_pair(pair).unwrap()
}

fn error_body(name: &str) -> String {
    serde_json::from_str::<Value>(ERRORS).unwrap()[name]
        .as_str()
        .unwrap()
        .to_string()
}

/// The documented order record with `edit` applied.
fn order(edit: impl FnOnce(&mut Value)) -> String {
    let mut order: Value = serde_json::from_str(ORDER).unwrap();
    edit(&mut order);
    order.to_string()
}

fn requests(seen: &Seens) -> Vec<String> {
    seen.lock().unwrap().iter().map(Seen::identity).collect()
}

fn ltc() -> Symbol {
    symbol("LTCBTC")
}

#[tokio::test]
async fn the_venue_is_a_spot_balance_account_sending_each_client_id_once() {
    let (venue, _) = answering(200, "{}").await;
    let caps = venue.capabilities();
    assert_eq!(caps.market_kind, MarketKind::Spot);
    assert_eq!(caps.account_kind, AccountKind::BalanceAccount);
    assert!(!caps.supports_short && !caps.reduce_only_wire_flag && !caps.has_funding);
    assert_eq!(caps.protection, ProtectionKind::MarketOnTrigger);
    assert_eq!(caps.replacement, ReplacementProtocol::CancelThenPlace);
    assert_eq!(caps.order_finality, SPOT_FINALITY);
    assert!(caps.one_attempt_per_client_id);
    assert_eq!(venue.id(), &VenueId::new(VenueId::BINANCE_SPOT));
    assert!(capability_mismatches(&venue).is_empty(), "{:?}", capability_mismatches(&venue));
    assert!(venue.position_account().is_none() && venue.funding().is_none());
    assert!(venue.inventory().is_some());
}

#[test]
fn a_testnet_client_is_refused() {
    let client = Arc::new(BinanceRestClient::testnet("key".into(), "secret".into()));
    assert!(matches!(BinanceSpotAdapter::new(client), Err(ExecError::Config(_))));
}

/// Every order-sending method, and every read a later step owns, refuses
/// with `Config` and sends nothing.
#[tokio::test]
async fn every_mutation_and_every_later_read_is_refused_with_no_request() {
    let (venue, seen) = answering(200, "{}").await;
    let btc = symbol("BTCUSDT");
    let qty = Quantity::new(dec!(0.001)).unwrap();
    let price = Price::new(dec!(90000)).unwrap();
    let stop = StopRef::new("1");
    let params = StopParams {
        close_side: OrderSide::Sell,
        quantity: qty,
        trigger: price,
        client_order_id: "ins-1".to_string(),
    };
    let mut refusals: Vec<(&str, ExecError)> = vec![
        (
            "place_market_order",
            venue
                .place_market_order(&btc, OrderSide::Buy, qty, "e-1", EntryOrReduce::Entry)
                .await
                .unwrap_err(),
        ),
        (
            "place_protection",
            venue
                .place_protection(&btc, OrderSide::Sell, qty, price, "ins-1")
                .await
                .unwrap_err(),
        ),
        ("cancel_protection", venue.cancel_protection(&btc, &stop).await.unwrap_err()),
        (
            "replace_protection",
            venue.replace_protection(&btc, &stop, params).await.unwrap_err(),
        ),
        ("cancel_order", venue.cancel_order(&btc, "1").await.unwrap_err()),
        ("order_by_exchange_id", venue.order_by_exchange_id(&btc, "1").await.unwrap_err()),
        ("protection_fill", venue.protection_fill(&btc, &stop).await.unwrap_err()),
        ("cancel_order_once", venue.cancel_order_once(&btc, "1").await.unwrap_err()),
    ];
    refusals.extend([
        ("governed_inventory", venue.governed_inventory(&btc).await.unwrap_err()),
        ("execution_costs", venue.execution_costs(&btc).await.unwrap_err()),
        ("settlement_since", venue.settlement_since(Utc::now(), 10).await.unwrap_err()),
        (
            "user_trades_since",
            venue.user_trades_since(&btc, Utc::now(), 10).await.unwrap_err(),
        ),
        (
            "observe_order",
            venue.observe_order(&btc, &OrderRef::ExchangeId("x1".into())).await.unwrap_err(),
        ),
        ("order_trades_page", venue.order_trades_page(&btc, "x1", 0).await.unwrap_err()),
    ]);
    for (name, error) in &refusals {
        assert!(matches!(error, ExecError::Config(_)), "{name}: {error:?}");
    }
    assert_eq!(requests(&seen), Vec::<String>::new(), "nothing reached the venue");
}

/// Each read: its request, its weight, and nothing else on the wire.
#[tokio::test]
async fn every_read_sends_one_documented_request() {
    let (venue, seen) = venue(|request| {
        let body = match request.path.as_str() {
            "/api/v3/exchangeInfo" => EXCHANGE_INFO.to_string(),
            "/api/v3/account" => ACCOUNT.to_string(),
            "/api/v3/openOrders" => format!("[{ORDER}]"),
            "/api/v3/openOrderList" => OPEN_ORDER_LIST.to_string(),
            "/api/v3/order" => ORDER.to_string(),
            "/api/v3/myTrades" => MY_TRADES
                .replace("\"orderId\": 100234", "\"orderId\": 1")
                .replace("BNBBTC", "LTCBTC"),
            "/api/v3/ticker/price" => TICKER_PRICE.to_string(),
            "/api/v3/ping" => "{}".to_string(),
            other => panic!("unexpected request {other}"),
        };
        (200, body)
    })
    .await;
    let btc = symbol("BTCUSDT");
    venue.trading_rules(&btc).await.unwrap();
    venue.order_by_client_id(&ltc(), "myOrder1").await.unwrap();
    venue.observe_order(&ltc(), &OrderRef::ExchangeId("1".into())).await.unwrap();
    venue
        .observe_order(&ltc(), &OrderRef::ClientId("myOrder1".into()))
        .await
        .unwrap();
    venue.order_trades_page(&ltc(), "1", 28457).await.unwrap();
    venue.open_protective_orders(&ltc()).await.unwrap();
    venue.price(&ltc()).await.unwrap();
    venue.health().await.unwrap();
    venue.cash("BTC").await.unwrap();
    venue.account_observation().await.unwrap();
    assert_eq!(requests(&seen), [
        "GET /api/v3/exchangeInfo?symbol=BTCUSDT",
        "GET /api/v3/order?origClientOrderId=myOrder1&symbol=LTCBTC",
        "GET /api/v3/order?orderId=1&symbol=LTCBTC",
        "GET /api/v3/order?origClientOrderId=myOrder1&symbol=LTCBTC",
        "GET /api/v3/myTrades?fromId=28457&limit=1000&orderId=1&symbol=LTCBTC",
        "GET /api/v3/openOrders?symbol=LTCBTC",
        "GET /api/v3/ticker/price?symbol=LTCBTC",
        "GET /api/v3/ping",
        "GET /api/v3/account",
        "GET /api/v3/account",
        "GET /api/v3/openOrders",
        "GET /api/v3/openOrderList",
    ]);
    let weights: Vec<u32> = venue.reader().sent().iter().map(|read| read.weight).collect();
    assert_eq!(weights, [20, 4, 4, 4, 5, 6, 2, 1, 20, 20, 80, 6]);
    assert_eq!(weights[9..].iter().sum::<u32>(), 106, "one account observation");
    assert!(venue.reader().sent().iter().all(|read| *read != SpotRead::MY_TRADES));
}

#[tokio::test]
async fn a_lookup_maps_the_venue_s_record_whole() {
    let body = order(|o| {
        o["type"] = json!("STOP_LOSS");
        o["side"] = json!("SELL");
        o["stopPrice"] = json!("0.09");
        o["cummulativeQuoteQty"] = json!("-1.00000000");
    });
    let (venue, _) = answering(200, body).await;
    let OrderObservation::Found(record) =
        venue.observe_order(&ltc(), &OrderRef::ExchangeId("1".into())).await.unwrap()
    else {
        panic!("not found");
    };
    assert_eq!(
        (
            record.exchange_order_id.as_str(),
            record.client_order_id.as_str(),
            record.symbol.as_str()
        ),
        ("1", "myOrder1", "LTCBTC")
    );
    assert_eq!((record.side, record.order_type.as_str()), (OrderSide::Sell, "STOP_LOSS"));
    assert_eq!(record.protection_kind, Some(ProtectionKind::MarketOnTrigger));
    assert_eq!((record.status.as_str(), record.phase), ("NEW", OrderPhase::Live));
    assert_eq!((record.orig_qty, record.executed_qty), (dec!(1.0), dec!(0.0)));
    assert_eq!(record.cumulative_quote_qty, None, "a negative quote is unavailable, never 0");
    assert_eq!(record.stop_price, Some(dec!(0.09)));
    assert_eq!(record.expiry_reason.as_deref(), Some("INSUFFICIENT_LIQUIDITY"));
    assert_eq!(record.orig_client_order_id, None);
    assert_eq!(
        (
            record.transact_time_ms,
            record.time_ms,
            record.update_time_ms,
            record.working_time_ms
        ),
        (None, Some(1499827319559), Some(1499827319559), Some(1499827319559))
    );

    // Only the exact protection primitive is protection.
    for kind in ["STOP_LOSS_LIMIT", "LIMIT", "MARKET"] {
        let (venue, _) = answering(200, order(|o| o["type"] = json!(kind))).await;
        let OrderObservation::Found(record) = venue
            .observe_order(&ltc(), &OrderRef::ClientId("myOrder1".into()))
            .await
            .unwrap()
        else {
            panic!("not found");
        };
        assert_eq!(record.protection_kind, None, "{kind}");
    }
}

#[tokio::test]
async fn the_phase_is_read_from_the_status_alone() {
    for (status, phase) in [
        ("NEW", OrderPhase::Live),
        ("PARTIALLY_FILLED", OrderPhase::Live),
        ("PENDING_NEW", OrderPhase::Live),
        ("FILLED", OrderPhase::Terminal),
        ("CANCELED", OrderPhase::Terminal),
        ("EXPIRED", OrderPhase::Terminal),
        ("EXPIRED_IN_MATCH", OrderPhase::Terminal),
        ("REJECTED", OrderPhase::Terminal),
        ("PENDING_CANCEL", OrderPhase::Unknown),
        ("PAUSED", OrderPhase::Unknown),
    ] {
        let (venue, _) = answering(200, order(|o| o["status"] = json!(status))).await;
        let OrderObservation::Found(record) =
            venue.observe_order(&ltc(), &OrderRef::ExchangeId("1".into())).await.unwrap()
        else {
            panic!("not found");
        };
        assert_eq!(record.phase, phase, "{status}");
    }
}

async fn lookup(status: &str, orig: &str, executed: &str) -> Result<OrderLookup, ExecError> {
    let body = order(|o| {
        o["status"] = json!(status);
        o["origQty"] = json!(orig);
        o["executedQty"] = json!(executed);
    });
    let (venue, _) = answering(200, body).await;
    venue.order_by_client_id(&ltc(), "myOrder1").await
}

#[tokio::test]
async fn a_client_id_lookup_reads_every_status_and_refuses_impossible_quantities() {
    let terminal = |filled, cancelled| OrderLookup::Terminal {
        filled_qty: filled,
        cancelled_qty: cancelled,
        fills: None,
    };
    for (status, executed, expected) in [
        ("NEW", "0", OrderLookup::Open { filled_qty: dec!(0) }),
        ("PENDING_NEW", "0", OrderLookup::Open { filled_qty: dec!(0) }),
        ("PARTIALLY_FILLED", "0.4", OrderLookup::PartiallyFilled {
            filled_qty: dec!(0.4),
        }),
        ("NEW", "0.4", OrderLookup::PartiallyFilled { filled_qty: dec!(0.4) }),
        ("FILLED", "1", terminal(dec!(1), dec!(0))),
        ("CANCELED", "0.4", terminal(dec!(0.4), dec!(0.6))),
        ("EXPIRED", "0", terminal(dec!(0), dec!(1))),
        ("EXPIRED_IN_MATCH", "0.25", terminal(dec!(0.25), dec!(0.75))),
        ("REJECTED", "0", terminal(dec!(0), dec!(1))),
        ("PENDING_CANCEL", "0", OrderLookup::Unknown),
        ("SOMETHING_NEW", "0", OrderLookup::Unknown),
    ] {
        assert_eq!(
            format!("{:?}", lookup(status, "1", executed).await.unwrap()),
            format!("{expected:?}"),
            "{status} {executed}"
        );
    }
    for (orig, executed) in [("1", "1.5"), ("1", "-0.1")] {
        assert!(
            matches!(lookup("FILLED", orig, executed).await, Err(ExecError::Exchange(_))),
            "{orig} {executed}"
        );
    }
    let (venue, _) = answering(200, order(|o| o["side"] = json!("SIDEWAYS"))).await;
    assert!(venue.order_by_client_id(&ltc(), "myOrder1").await.is_err());
}

#[tokio::test]
async fn a_not_found_is_absent_only_under_the_finality_rule_and_never_at_another_status() {
    // -2013 at 400: the finality rule over the attempts recorded for the id.
    // None were (no journal, no send), so it stays Unknown.
    for (status, name) in [
        (400, "no_such_order"),
        (500, "no_such_order"),
        (503, "no_such_order"),
        (400, "order_archived"),
        (503, "order_archived"),
    ] {
        let (venue, _) = answering(status, error_body(name)).await;
        assert!(
            matches!(venue.order_by_client_id(&ltc(), "e-1").await, Ok(OrderLookup::Unknown)),
            "{status} {name}"
        );
    }
    for (status, name) in [
        (429, "too_many_requests"),
        (400, "bad_symbol"),
        (500, "unknown"),
    ] {
        let (venue, _) = answering(status, error_body(name)).await;
        assert!(venue.order_by_client_id(&ltc(), "e-1").await.is_err(), "{status} {name}");
    }
}

#[tokio::test]
async fn an_observation_is_not_found_or_archived_only_at_http_400() {
    let observe = |status, name: &'static str| async move {
        let (venue, _) = answering(status, error_body(name)).await;
        venue.observe_order(&ltc(), &OrderRef::ExchangeId("1".into())).await
    };
    assert_eq!(observe(400, "no_such_order").await.unwrap(), OrderObservation::NotFound);
    assert_eq!(observe(400, "order_archived").await.unwrap(), OrderObservation::Archived);
    for (status, name) in [
        (503, "no_such_order"),
        (500, "order_archived"),
        (418, "no_such_order"),
        (503, "backend_timeout"),
        (503, "html_page"),
    ] {
        assert!(observe(status, name).await.is_err(), "{status} {name}");
    }
}

#[tokio::test]
async fn an_observation_of_another_order_or_with_impossible_quantities_is_an_error() {
    for body in [
        order(|o| o["orderId"] = json!(2)),
        order(|o| o["symbol"] = json!("ETHBTC")),
        order(|o| o["executedQty"] = json!("2.0")),
    ] {
        let (venue, _) = answering(200, body.clone()).await;
        assert!(
            venue.observe_order(&ltc(), &OrderRef::ExchangeId("1".into())).await.is_err(),
            "{body}"
        );
    }
}

#[tokio::test]
async fn a_trades_page_carries_each_trade_with_its_time_and_the_cursor() {
    let trades: Vec<Value> = (100..1_100)
        .map(|id| {
            let mut trade: Value = serde_json::from_str::<Value>(MY_TRADES).unwrap()[0].clone();
            trade["id"] = json!(id);
            trade["orderId"] = json!(7);
            trade
        })
        .collect();
    let (venue, _) = answering(200, serde_json::to_string(&trades).unwrap()).await;
    let page = venue.order_trades_page(&symbol("BNBBTC"), "7", 100).await.unwrap();
    assert_eq!((page.trades.len(), page.next_from_id), (1_000, Some(1_100)));
    let first = &page.trades[0];
    assert_eq!(first.trade_id, "100");
    assert_eq!(
        (first.price, first.qty, first.quote_qty),
        (dec!(4.00000100), dec!(12), dec!(48.000012))
    );
    assert_eq!((first.commission, first.commission_asset.as_str()), (dec!(10.1), "BNB"));
    assert_eq!(first.time, Some(Utc.timestamp_millis_opt(1499865549590).unwrap()));
}

#[tokio::test]
async fn open_protective_orders_are_the_stop_loss_sells_only() {
    let orders: Vec<Value> = [
        ("STOP_LOSS", "SELL", 11),
        ("STOP_LOSS", "BUY", 12),
        ("STOP_LOSS_LIMIT", "SELL", 13),
        ("LIMIT", "SELL", 14),
    ]
    .into_iter()
    .map(|(kind, side, id)| {
        serde_json::from_str::<Value>(&order(|o| {
            o["type"] = json!(kind);
            o["side"] = json!(side);
            o["orderId"] = json!(id);
            o["clientOrderId"] = json!(format!("ins-{id}"));
            o["stopPrice"] = json!("0.09");
        }))
        .unwrap()
    })
    .collect();
    let (venue, _) = answering(200, serde_json::to_string(&orders).unwrap()).await;
    let protective = venue.open_protective_orders(&ltc()).await.unwrap();
    assert_eq!(protective.len(), 1);
    let stop = &protective[0];
    assert_eq!(
        (stop.exchange_order_id.as_str(), stop.client_order_id.as_str(), stop.side),
        ("11", "ins-11", OrderSide::Sell)
    );
    assert!(!stop.reduce_only, "spot has no reduce-only flag");
    assert_eq!(stop.stop_price, Some(Price::new(dec!(0.09)).unwrap()));
}

#[tokio::test]
async fn cash_and_the_account_observation_read_the_account_as_given() {
    let (venue, _) = venue(|request| match request.path.as_str() {
        "/api/v3/account" => (200, ACCOUNT.replace("\"4763368.68006011\"", "\"0\"")),
        "/api/v3/openOrders" => (200, format!("[{ORDER}]")),
        "/api/v3/openOrderList" => (200, OPEN_ORDER_LIST.to_string()),
        other => panic!("unexpected request {other}"),
    })
    .await;
    let btc = venue.cash("BTC").await.unwrap();
    assert_eq!((btc.free, btc.locked), (dec!(4723846.89208129), dec!(0)));
    let usdt = venue.cash("USDT").await.unwrap();
    assert_eq!((usdt.free, usdt.locked), (dec!(0), dec!(0)), "an unlisted asset holds nothing");

    let observed = venue.account_observation().await.unwrap();
    let assets: Vec<&str> = observed.holdings.iter().map(|(asset, _)| asset.as_str()).collect();
    assert_eq!(assets, ["BTC"], "nonzero holdings only");
    assert_eq!(observed.open_orders.len(), 1);
    let open = &observed.open_orders[0];
    assert_eq!(open.symbol, ltc());
    assert_eq!(open.record.as_ref().unwrap().exchange_order_id, "1");
    assert_eq!(open.order.client_order_id, "myOrder1");
    assert_eq!(observed.open_order_lists, 1);
    assert_eq!(
        (observed.uid.as_str(), observed.can_trade, observed.account_type.as_str()),
        ("354937868", true, "SPOT")
    );
    assert_eq!(observed.permissions, ["SPOT"]);
}

#[tokio::test]
async fn an_open_order_on_a_symbol_robson_cannot_name_fails_the_observation() {
    let (venue, _) = venue(|request| match request.path.as_str() {
        "/api/v3/account" => (200, ACCOUNT.to_string()),
        "/api/v3/openOrders" => (200, format!("[{}]", order(|o| o["symbol"] = json!("ABCXYZ")))),
        "/api/v3/openOrderList" => (200, "[]".to_string()),
        other => panic!("unexpected request {other}"),
    })
    .await;
    assert!(venue.account_observation().await.is_err());
}

#[tokio::test]
async fn price_and_health_are_one_public_read_each() {
    let (venue, _) = answering(200, TICKER_PRICE).await;
    assert_eq!(venue.price(&ltc()).await.unwrap(), Price::new(dec!(4.00000200)).unwrap());
    assert_eq!(
        venue.valuation(&ltc()).await.unwrap().price,
        Price::new(dec!(4.000002)).unwrap()
    );
    assert!(venue.price(&symbol("ETHBTC")).await.is_err(), "a ticker of another symbol");
    let (venue, _) = answering(503, error_body("html_page")).await;
    assert!(venue.health().await.is_err());
}

#[tokio::test]
async fn the_venue_serves_its_rules_from_the_cache() {
    let (venue, seen) = answering(200, EXCHANGE_INFO).await;
    let btc = symbol("BTCUSDT");
    assert_eq!(venue.last_validated_rules(&btc), None);
    assert!(venue.rules_refresh_due(&btc));
    let rules = venue.trading_rules_fresh(&btc).await.unwrap();
    assert_eq!(venue.trading_rules(&btc).await.unwrap(), rules);
    assert_eq!(venue.refresh_trading_rules(&btc).await.unwrap(), rules);
    assert_eq!(venue.trading_rules_preview(&btc).await.unwrap(), rules);
    assert_eq!(venue.last_validated_rules(&btc), Some(rules));
    assert!(!venue.rules_refresh_due(&btc));
    assert_eq!(requests(&seen).len(), 1, "one exchangeInfo serves them all");
    let spot = venue.validated_symbol_rules(&btc).unwrap();
    assert!(spot.allowed_stp_modes.iter().any(|mode| mode == "EXPIRE_MAKER"));
}

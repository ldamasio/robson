//! The report's pure evaluation (ADR-0056 step 8m-3): each venue check's pass
//! and fail, C7's process memory, C10's precision, the hash and the terminal
//! event types. The database checks run on PostgreSQL in
//! `cutover_report_pg_tests.rs`.
use chrono::TimeZone;
use robson_domain::OrderSide;
use robson_exec::{
    ports::OpenOrderRecord,
    venue::{AccountOpenOrder, Holding},
};
use rust_decimal_macros::dec;

use super::*;

/// C10-Z's quote: ten decimals, kept at the ledger's precision.
const QUOTE: Decimal = dec!(2100.1234567891);
pub(super) const CLEAN: RuntimeFacts = RuntimeFacts { incidents: 0, latch: false, suspended: 0 };

pub(super) fn symbol() -> Symbol {
    Symbol::from_pair("BTCUSDT").unwrap()
}

fn at(second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 12, 15, 12, 0, second).unwrap()
}

fn order(base: &str, quote: &str) -> AccountOpenOrder {
    AccountOpenOrder {
        symbol: Symbol::new(base.into(), quote.into()).unwrap(),
        order: OpenOrderRecord {
            exchange_order_id: "7".into(),
            client_order_id: "manual".into(),
            order_type: "LIMIT".into(),
            reduce_only: false,
            stop_price: None,
            side: OrderSide::Sell,
        },
        record: None,
    }
}

/// Every check's evidence green, at basis min(0, QUOTE) = 0.
pub(super) fn green() -> (DatabaseFacts, VenueFacts) {
    let db = DatabaseFacts {
        month: NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(),
        previous: Some(Decimal::ZERO),
        checks: DATABASE_CHECKS.iter().map(|(id, _)| (id.to_string(), true)).collect(),
        values: BTreeMap::from([("incidents".to_string(), json!(0))]),
        incident_reasons: None,
        run: None,
    };
    let account = InventoryObservation {
        holdings: vec![("USDT".into(), Holding { free: QUOTE, locked: Decimal::ZERO })],
        open_orders: vec![],
        as_of: at(0),
        open_order_lists: 0,
        uid: "1234567".into(),
        can_trade: true,
        account_type: "SPOT".into(),
        permissions: vec!["SPOT".into()],
    };
    let venue = VenueFacts {
        account: Ok(account),
        order_types: Ok(["LIMIT", "MARKET", "STOP_LOSS"].map(String::from).to_vec()),
        costs: Ok(ExecutionCostEnvelope {
            maker_bps: dec!(10),
            taker_bps: dec!(10),
            gap_bps: dec!(5),
            fee_asset_policy: FeeAssetPolicy::Base,
            funding: None,
            as_of: at(0),
            source: "test".into(),
        }),
        rules: Ok(()),
        clock: Ok(Duration::from_millis(200)),
        usdm: Ok([0; 3]),
    };
    (db, venue)
}

fn report(db: &DatabaseFacts, venue: &VenueFacts, runtime: RuntimeFacts) -> Report {
    evaluate(db, venue, runtime, &symbol(), at(30))
}

/// Only `id` fails, with `reason` in its detail.
fn only(report: &Report, id: &str, reason: &str) {
    let failed: Vec<&str> =
        report.checks.iter().filter(|(_, v)| !v.pass).map(|(id, _)| *id).collect();
    assert_eq!(failed, [id], "{report:#?}");
    assert!(report.checks[id].detail.contains(reason), "{:?}", report.checks[id]);
    assert!(!report.passed());
}

fn account(venue: &mut VenueFacts) -> &mut InventoryObservation {
    venue.account.as_mut().unwrap()
}

#[test]
fn a_green_report_passes_every_check() {
    let (db, venue) = green();
    let report = report(&db, &venue, CLEAN);
    let ids: Vec<&str> = report.checks.keys().copied().collect();
    assert_eq!(ids, [
        "c1", "c10", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9a", "c9b", "c9c", "c9d", "sd14"
    ]);
    assert!(report.passed(), "{report:#?}");
    assert_eq!(report.decision.uid.as_deref(), Some("1234567"));
    assert_eq!(
        (report.decision.previous, report.decision.basis),
        (Some(dec!(0)), Some(dec!(0)))
    );
    assert_eq!(report.decision.quote_total, Some(QUOTE));
    assert_eq!(report.hash.len(), 64);
}

#[test]
fn c1_requires_a_spot_account_allowed_to_trade_with_both_order_types() {
    let changes: [fn(&mut VenueFacts); 5] = [
        |v| account(v).account_type = "MARGIN".into(),
        |v| account(v).can_trade = false,
        |v| account(v).permissions = vec!["MARGIN".into()],
        |v| v.order_types = Ok(vec!["LIMIT".into(), "MARKET".into()]),
        |v| v.order_types = Ok(vec!["LIMIT".into(), "STOP_LOSS".into()]),
    ];
    for change in changes {
        let (db, mut venue) = green();
        change(&mut venue);
        only(&report(&db, &venue, CLEAN), "c1", "not a SPOT account");
    }
    let (db, mut venue) = green();
    venue.order_types = Err("exchangeInfo: HTTP 503".into());
    only(&report(&db, &venue, CLEAN), "c1", "HTTP 503");
}

#[test]
fn c2_requires_a_base_asset_envelope_and_c3_and_c5_their_reads() {
    let (db, mut venue) = green();
    venue.costs.as_mut().unwrap().fee_asset_policy = FeeAssetPolicy::Quote;
    only(&report(&db, &venue, CLEAN), "c2", "base asset");
    venue.costs = Err("the Spot cost envelope arrives with 8g".into());
    only(&report(&db, &venue, CLEAN), "c2", "arrives with 8g");
    let (db, mut venue) = green();
    venue.rules = Err("rules stale".into());
    only(&report(&db, &venue, CLEAN), "c3", "rules stale");
    let (db, mut venue) = green();
    venue.clock = Ok(Duration::from_millis(1001));
    only(&report(&db, &venue, CLEAN), "c5", "above 1 s");
    venue.clock = Err("the sample's evidence is invalid".to_string());
    only(&report(&db, &venue, CLEAN), "c5", "evidence is invalid");
    venue.clock = Ok(Duration::from_secs(1));
    assert!(report(&db, &venue, CLEAN).passed());
}

/// SD7 and operator question Q2: any BTC, held or locked (a foreign order
/// locking it), an open BTCUSDT order or an open order list fails C4 with its
/// reason and values.
#[test]
fn c4_fails_on_held_or_locked_btc_and_open_btcusdt_orders() {
    let changes: [fn(&mut VenueFacts); 4] = [
        |v| {
            account(v)
                .holdings
                .push(("BTC".into(), Holding { free: dec!(0.0001), locked: dec!(0) }))
        },
        // A foreign order on another BTC pair locks BTC.
        |v| {
            account(v)
                .holdings
                .push(("BTC".into(), Holding { free: dec!(0), locked: dec!(0.002) }));
            account(v).open_orders.push(order("BTC", "FDUSD"));
        },
        |v| account(v).open_orders.push(order("BTC", "USDT")),
        |v| account(v).open_order_lists = 1,
    ];
    for change in changes {
        let (db, mut venue) = green();
        change(&mut venue);
        only(&report(&db, &venue, CLEAN), "c4", "BTC held or locked");
    }
    // Other assets are ignored (SD7).
    let (db, mut venue) = green();
    account(&mut venue)
        .holdings
        .push(("ETH".into(), Holding { free: dec!(1), locked: dec!(1) }));
    account(&mut venue).open_orders.push(order("ETH", "USDT"));
    let passed = report(&db, &venue, CLEAN);
    assert!(passed.passed(), "{passed:#?}");
    assert_eq!(passed.decision.values["c4"], json!([["0", "0"], 0, 0]));
    // A failed account read fails every check it feeds, never green.
    let (db, mut venue) = green();
    venue.account = Err("account: HTTP 418".into());
    let failed = report(&db, &venue, CLEAN);
    for id in ["c1", "c4", "c10"] {
        assert_eq!(failed.checks[id], Verdict {
            pass: false,
            detail: "account: HTTP 418".into()
        });
    }
    assert_eq!(failed.decision.uid, None);
}

#[test]
fn c6_fails_on_an_open_usdm_algo_order() {
    for open in [
        // An open position, an open order, an open algo order.
        [1, 0, 0],
        [0, 1, 0],
        [0, 0, 1],
    ] {
        let (db, mut venue) = green();
        venue.usdm = Ok(open);
        only(&report(&db, &venue, CLEAN), "c6", "USD-M");
    }
    // Q1: a missing permission fails C6 closed and says so.
    let (db, mut venue) = green();
    venue.usdm = Err(usdm_refusal(BinanceRestError::ApiError {
        code: -2015,
        msg: "Invalid API-key, IP, or permissions for action.".into(),
        http_status: 400,
    }));
    only(&report(&db, &venue, CLEAN), "c6", "usdm_permission_missing");
}

/// C6's seam: 8m-6 fills `VenueFacts::usdm` with what a [`UsdmReadObserver`]
/// answers (in production the Spot daemon key's client, through
/// `open_book_counts`), as is: a clean book passes, an open one or a refused
/// read fails C6 alone, by its reason.
#[tokio::test]
async fn c6_takes_a_usdm_observers_answer_as_its_evidence() {
    struct Answer(Result<[usize; 3], String>);
    #[async_trait]
    impl UsdmReadObserver for Answer {
        async fn open(&self) -> Result<[usize; 3], String> {
            self.0.clone()
        }
    }
    let denied = BinanceRestError::RequestFailed("HTTP 401 Unauthorized: ".into());
    for (answer, failed) in [
        (Ok([0; 3]), None),
        (Ok([0, 0, 1]), Some("USD-M")),
        (Err(usdm_refusal(denied)), Some("usdm_permission_missing")),
    ] {
        let observer: &dyn UsdmReadObserver = &Answer(answer);
        let (db, mut venue) = green();
        venue.usdm = observer.open().await;
        let report = report(&db, &venue, CLEAN);
        match failed {
            None => assert!(report.passed(), "{report:#?}"),
            Some(reason) => only(&report, "c6", reason),
        }
    }
}

/// Q1: each sign of a key without the read permission, alone, is named so:
/// the codes, a JSON error at 401 or 403, and a 401 or 403 without Binance's
/// JSON body (`RestAnswer::into_body`'s text, pinned in robson-connectors).
#[test]
fn a_missing_usdm_permission_is_named_by_each_sign() {
    let api = |code: i64, http_status: u16| BinanceRestError::ApiError {
        code,
        msg: "refused".into(),
        http_status,
    };
    for denied in [
        api(-2015, 400),
        api(-2014, 400),
        api(-1100, 401),
        api(-1100, 403),
        BinanceRestError::RequestFailed("HTTP 401 Unauthorized: ".into()),
        BinanceRestError::RequestFailed("HTTP 403 Forbidden: <html></html>".into()),
    ] {
        let reason = usdm_refusal(denied);
        assert!(reason.starts_with("usdm_permission_missing: "), "{reason}");
    }
    for failed in [
        api(-1003, 429),
        api(-1100, 400),
        BinanceRestError::RequestFailed("HTTP 502 Bad Gateway: <html></html>".into()),
        BinanceRestError::Timeout,
    ] {
        let reason = usdm_refusal(failed);
        assert!(reason.starts_with("usdm_read_failed: "), "{reason}");
    }
}

#[test]
fn c7_reads_the_gate_journal_and_stream_memory() {
    for runtime in [
        RuntimeFacts { incidents: 1, ..CLEAN },
        RuntimeFacts { latch: true, ..CLEAN },
        RuntimeFacts { suspended: 1, ..CLEAN },
    ] {
        let (db, venue) = green();
        only(&report(&db, &venue, runtime), "c7", "an open incident");
    }
    // The open incidents' reasons are named in C7's detail (Q2's
    // venue_discrepancy among them), and never hashed.
    let (mut db, venue) = green();
    let base = report(&db, &venue, CLEAN).hash;
    db.checks.insert("c7".into(), false);
    let unnamed = report(&db, &venue, CLEAN).hash;
    assert_ne!(unnamed, base);
    for reasons in [vec!["venue_discrepancy"], vec![
        "stream_suspended",
        "unclean_start",
    ]] {
        db.incident_reasons = Some(reasons.iter().map(|r| r.to_string()).collect());
        let named = report(&db, &venue, CLEAN);
        only(&named, "c7", &format!("open incidents: {}", reasons.join(", ")));
        assert_eq!(named.hash, unnamed);
    }
    // Each database verdict is the statement's; a missing one fails.
    for (id, _) in DATABASE_CHECKS {
        let (mut db, venue) = green();
        db.checks.insert(id.to_string(), false);
        only(&report(&db, &venue, CLEAN), id, "");
        db.checks.remove(id);
        only(&report(&db, &venue, CLEAN), id, "");
    }
}

/// C10-Z: previous 0 certifies at basis 0 whatever the quote's scale; a
/// positive previous above a quote not exact at scale 8 is refused.
#[test]
fn a_precise_quote_with_zero_previous_certifies_at_zero() {
    let (db, venue) = green();
    let zero = report(&db, &venue, CLEAN);
    assert!(zero.passed(), "{zero:#?}");
    assert_eq!(zero.decision.basis, Some(Decimal::ZERO));
    let (mut db, venue) = green();
    db.previous = Some(dec!(2500));
    let refused = report(&db, &venue, CLEAN);
    only(&refused, "c10", "not exact");
    assert_eq!(refused.decision.basis, Some(QUOTE));
    // Exact at scale 8, the quote below a previous certifies at it.
    let (mut db, mut venue) = green();
    db.previous = Some(dec!(2500));
    account(&mut venue).holdings[0].1.free = dec!(2100.12345678);
    assert!(report(&db, &venue, CLEAN).passed());
    // No month row: no previous basis.
    let (mut db, venue) = green();
    db.previous = None;
    only(&report(&db, &venue, CLEAN), "c10", "no previous basis");
}

#[test]
fn the_hash_covers_verdicts_and_decisions_and_nothing_else() {
    let (db, venue) = green();
    let base = report(&db, &venue, CLEAN);
    // Timestamps, reasons' text and the venue's run are not hashed.
    assert_eq!(evaluate(&db, &venue, CLEAN, &symbol(), at(59)).hash, base.hash);
    let (mut db, mut venue) = green();
    db.run = Some(json!({"state": "pending"}));
    account(&mut venue).as_of = at(9);
    venue.costs.as_mut().unwrap().as_of = at(9);
    assert_eq!(report(&db, &venue, CLEAN).hash, base.hash);
    let (db, mut venue) = green();
    venue.rules = Err("one reason".into());
    let one = report(&db, &venue, CLEAN).hash;
    venue.rules = Err("another reason".into());
    assert_eq!(report(&db, &venue, CLEAN).hash, one);
    // Any verdict or decision value changes it.
    let changes: [fn(&mut DatabaseFacts, &mut VenueFacts); 8] = [
        |_, v| v.rules = Err("x".into()),
        |_, v| account(v).uid = "7654321".into(),
        |_, v| account(v).holdings[0].1.free = dec!(2100),
        |_, v| account(v).holdings[0].1.locked = dec!(1),
        |d, _| d.previous = Some(dec!(1)),
        |d, _| d.month = NaiveDate::from_ymd_opt(2027, 1, 1).unwrap(),
        |d, _| {
            d.values.insert("incidents".into(), json!(1));
        },
        |_, v| v.usdm = Ok([0, 0, 1]),
    ];
    for change in changes {
        let (mut db, mut venue) = green();
        change(&mut db, &mut venue);
        assert_ne!(report(&db, &venue, CLEAN).hash, base.hash);
    }
}

/// C9b's terminal types are exactly the event types the projector routes to
/// a handler that ends a row closed, cancelled or in error.
#[test]
fn the_terminal_event_types_are_the_projectors() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../robson-projector/src");
    let read = |file: &str| std::fs::read_to_string(src.join(file)).unwrap();
    let (apply, positions) = (read("apply.rs"), read("handlers/positions.rs"));
    // The handlers whose body sets a terminal state.
    let terminal: Vec<&str> = positions
        .split("async fn handle_")
        .skip(1)
        .filter(|body| {
            ["'closed'", "'cancelled'", "'error'"]
                .iter()
                .any(|s| body.contains(&format!("state = {s}")))
        })
        .map(|body| body.split('(').next().unwrap())
        .collect();
    assert_eq!(terminal, [
        "position_closed",
        "entry_execution_rejected",
        "position_disarmed",
        "position_closed_domain"
    ]);
    // The routes dispatched to them, then the event types routed there.
    let start = apply.find("fn projection_route(").unwrap();
    let (routing, dispatch) = apply[start..].split_once("\n}\n").unwrap();
    let (mut routes, mut pending) = (Vec::new(), Vec::new());
    for line in dispatch.lines() {
        if let Some((_, route)) = line.split_once("ProjectionRoute::") {
            pending.push(route.split(')').next().unwrap());
        } else if let Some((_, handler)) = line.split_once("::handle_") {
            if terminal.contains(&handler.split('(').next().unwrap()) {
                routes.append(&mut pending);
            }
            pending.clear();
        }
    }
    let (mut types, mut quoted) = (Vec::new(), Vec::new());
    for line in routing.lines().filter(|line| !line.trim_start().starts_with("//")) {
        quoted.extend(line.split('"').skip(1).step_by(2));
        if let Some((_, route)) = line.split_once("ProjectionRoute::") {
            if routes.contains(&route.split(')').next().unwrap()) {
                types.append(&mut quoted);
            }
            quoted.clear();
        }
    }
    types.sort_unstable();
    let mut expected = TERMINAL.to_vec();
    expected.sort_unstable();
    assert_eq!(types, expected);
}

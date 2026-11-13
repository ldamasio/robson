//! `SpotBook` against hand-computed values and its transition table
//! (8d plan §3.1, §7.1, §7.4). Only the public API is used.

use rust_decimal::Decimal;

use crate::spot_book::{
    BookError, BookEvent, BookOrder, BookRules, BookStatus, CrossRefused, OrderRef, Placement,
    Script, Side, SpotBook, MAX_EVENTS, MAX_RETAINED_ORDERS, MAX_SCRIPTS, MAX_TRADES_PER_ORDER,
};

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// BTCUSDT at 100 with its live limits (200, 5); ETHUSDT at 10 with 3 and 5.
fn book() -> SpotBook {
    let eth = BookRules::new("ETHUSDT", "ETH", "USDT").with_limits(3, 5);
    let book = SpotBook::new(vec![BookRules::btcusdt(), eth], d("0.001"));
    book.set_price("BTCUSDT", d("100"));
    book.set_price("ETHUSDT", d("10"));
    book
}

fn rejected(code: i64, msg: &str) -> BookError {
    BookError::Rejected { code, msg: msg.to_string() }
}

/// Per asset, account + market + fees sum to zero.
fn assert_conserved(book: &SpotBook) {
    for asset in ["BTC", "ETH", "USDT", "BNB"] {
        let l = book.ledger(asset);
        assert_eq!(l.free + l.locked + l.market + l.fees, Decimal::ZERO, "{asset}: {l:?}");
    }
}

/// Everything a refused command must leave as it was.
fn state(book: &SpotBook) -> (Vec<BookOrder>, Vec<(Decimal, Decimal)>, usize) {
    let balances = ["BTC", "ETH", "USDT", "BNB"]
        .iter()
        .map(|a| (book.balance(a).free, book.balance(a).locked))
        .collect();
    (book.all_orders(), balances, book.trades_retained())
}

fn stop(book: &SpotBook, qty: &str, trigger: &str, client_id: &str) -> u64 {
    book.place_stop("BTCUSDT", d(qty), d(trigger), client_id).unwrap().order.id
}

fn status(book: &SpotBook, id: u64) -> BookStatus {
    book.all_orders().into_iter().find(|o| o.id == id).unwrap().status
}

#[test]
fn a_buy_pays_its_commission_in_base_and_a_sell_in_quote() {
    let book = book();
    book.set_balance("USDT", d("1000"));
    let buy = book.place_market("BTCUSDT", Side::Buy, d("1"), "b1").unwrap();
    assert_eq!(buy.order.status, BookStatus::Filled);
    assert_eq!(book.balance("BTC").free, d("0.999"), "receives 1 − 0.001");
    assert_eq!(book.balance("USDT").free, d("900"), "spends 1 × 100");
    let fill = &buy.fills[0];
    assert_eq!((fill.commission, fill.commission_asset.as_str()), (d("0.001"), "BTC"));
    assert_eq!(
        book.trades("BTCUSDT", Some(buy.order.id), None, 10),
        buy.fills,
        "FULL fills = myTrades"
    );

    book.set_price("BTCUSDT", d("110"));
    let sell = book.place_market("BTCUSDT", Side::Sell, d("0.999"), "s1").unwrap();
    assert_eq!(book.balance("BTC").free, Decimal::ZERO);
    assert_eq!(book.balance("USDT").free, d("1009.78011"), "+ 0.999 × 110 × 0.999 = 109.78011");
    let fill = &sell.fills[0];
    assert_eq!((fill.commission, fill.commission_asset.as_str()), (d("0.10989"), "USDT"));
    assert_eq!((fill.qty, fill.price, fill.quote_qty), (d("0.999"), d("110"), d("109.89")));
    assert_eq!(book.ledger("BTC").fees, d("0.001"));
    assert_eq!(book.ledger("USDT").fees, d("0.10989"));
    assert_eq!((book.bought("BTCUSDT"), book.sold("BTCUSDT")), (d("1"), d("0.999")));
    assert_conserved(&book);
}

#[test]
fn a_stop_locks_its_base_and_executes_from_the_lock() {
    let book = book();
    book.set_balance("BTC", d("1"));
    let id = stop(&book, "0.4", "90", "st");
    assert_eq!((book.balance("BTC").free, book.balance("BTC").locked), (d("0.6"), d("0.4")));
    book.set_price("BTCUSDT", d("85"));
    let trade = book.cross(id, d("0.1")).unwrap();
    assert_eq!(status(&book, id), BookStatus::PartiallyFilled);
    assert_eq!(book.balance("BTC").locked, d("0.3"));
    assert_eq!(book.balance("USDT").free, d("8.4915"), "0.1 × 85 × 0.999");
    assert_eq!((trade.commission, trade.commission_asset.as_str()), (d("0.0085"), "USDT"));
    book.cross(id, d("0.3")).unwrap();
    assert_eq!(status(&book, id), BookStatus::Filled);
    assert_eq!(
        (book.balance("BTC").free, book.balance("BTC").locked),
        (d("0.6"), Decimal::ZERO)
    );
    let ended = book
        .events()
        .into_iter()
        .filter(|e| matches!(e, BookEvent::Ended { .. }))
        .count();
    assert_eq!(ended, 1, "one terminal transition");
    assert_conserved(&book);
}

/// §3.1, `cross`: positive quantities move a live stop to `PartiallyFilled`
/// then `Filled`; zero, negative, overfill, terminal and MARKET are refused
/// with nothing changed.
#[test]
fn cross_follows_the_transition_table() {
    let book = book();
    book.set_balance("BTC", d("2"));
    book.set_balance("USDT", d("1000"));
    let id = stop(&book, "1", "90", "st");
    for (qty, refusal) in [
        ("0", CrossRefused::NotPositive),
        ("-1", CrossRefused::NotPositive),
        ("1.1", CrossRefused::Overfill),
    ] {
        let before = state(&book);
        assert_eq!(book.cross(id, d(qty)), Err(refusal), "{qty}");
        assert_eq!(state(&book), before, "{qty} changed nothing");
    }
    assert_eq!(status(&book, id), BookStatus::New, "cross(0) never sets PartiallyFilled");
    book.cross(id, d("0.25")).unwrap();
    book.cross(id, d("0.25")).unwrap();
    assert_eq!(status(&book, id), BookStatus::PartiallyFilled);
    assert_eq!(book.cross(id, d("0.6")), Err(CrossRefused::Overfill));
    book.cross(id, d("0.5")).unwrap();
    assert_eq!(status(&book, id), BookStatus::Filled);
    assert_eq!(book.cross(id, d("0.1")), Err(CrossRefused::Terminal));
    let market = book.place_market("BTCUSDT", Side::Buy, d("1"), "m").unwrap().order.id;
    assert_eq!(book.cross(market, d("0.1")), Err(CrossRefused::NotAStop));
    assert_eq!(book.cross(999, d("0.1")), Err(CrossRefused::UnknownOrder));
    assert_conserved(&book);
}

/// §3.1, `cancel` and `expire`: the lock released is `orig − executed`,
/// once; a terminal order answers `-2011` and stays as it is.
#[test]
fn cancel_and_expire_release_the_rest_once() {
    let book = book();
    book.set_balance("BTC", d("3"));
    let new = stop(&book, "1", "90", "a");
    let partial = stop(&book, "1", "90", "b");
    let expiring = stop(&book, "1", "90", "c");
    book.cross(partial, d("0.4")).unwrap();
    book.cross(expiring, d("0.3")).unwrap();
    assert_eq!(book.balance("BTC").locked, d("2.3"));

    assert_eq!(book.cancel("BTCUSDT", new).unwrap().status, BookStatus::Canceled);
    assert_eq!(book.balance("BTC").locked, d("1.3"));
    let cancelled = book.cancel("BTCUSDT", partial).unwrap();
    assert_eq!((cancelled.status, cancelled.executed_qty), (BookStatus::Canceled, d("0.4")));
    assert_eq!(book.balance("BTC").locked, d("0.7"), "0.6 of the partial released");
    assert!(book.expire(expiring));
    assert_eq!(status(&book, expiring), BookStatus::Expired);
    assert_eq!(
        (book.balance("BTC").free, book.balance("BTC").locked),
        (d("2.3"), Decimal::ZERO)
    );

    let before = state(&book);
    for id in [new, partial, expiring] {
        assert_eq!(book.cancel("BTCUSDT", id), Err(BookError::NotOpen));
        assert!(!book.expire(id));
    }
    assert_eq!(state(&book), before);
    let unlocks: Vec<Decimal> = book
        .events()
        .into_iter()
        .filter_map(|e| match e {
            BookEvent::Ended { unlocked, .. } => Some(unlocked),
            _ => None,
        })
        .collect();
    assert_eq!(unlocks, [d("1"), d("0.6"), d("0.7")]);
    assert_conserved(&book);
}

#[test]
fn a_cancel_of_an_unknown_order_or_on_another_symbol_is_not_open() {
    let book = book();
    book.set_balance("BTC", d("1"));
    let id = stop(&book, "1", "90", "a");
    assert_eq!(book.cancel("BTCUSDT", 999), Err(BookError::NotOpen));
    assert_eq!(book.cancel("ETHUSDT", id), Err(BookError::NotOpen));
    assert_eq!(BookError::NotOpen.code(), Some(-2011));
    assert_eq!(status(&book, id), BookStatus::New);
}

/// §3.1, delayed cancels: queued and answered lost while the order stays
/// live; applied once; a fill that overtakes one wins (terminal precedence).
#[test]
fn delayed_cancels_apply_once_and_never_overwrite_a_fill() {
    let book = book();
    book.set_balance("BTC", d("2"));
    let overtaken = stop(&book, "1", "90", "a");
    let waiting = stop(&book, "1", "90", "b");
    book.delay_cancels(true);
    assert_eq!(book.cancel("BTCUSDT", overtaken), Err(BookError::Lost));
    assert_eq!(
        book.cancel("BTCUSDT", overtaken),
        Err(BookError::Lost),
        "asked twice, queued once"
    );
    assert_eq!(book.cancel("BTCUSDT", waiting), Err(BookError::Lost));
    assert_eq!(book.queued_cancels(), [overtaken, waiting]);
    assert_eq!(status(&book, overtaken), BookStatus::New, "still live");
    assert_eq!(book.open_orders(Some("BTCUSDT")).len(), 2);

    book.cross(overtaken, d("1")).unwrap();
    assert_eq!(
        book.cancel("BTCUSDT", overtaken),
        Err(BookError::NotOpen),
        "terminal, even when delayed"
    );
    assert_eq!(book.apply_delayed_cancels(), 1);
    assert_eq!(status(&book, overtaken), BookStatus::Filled, "the fill stands");
    assert_eq!(status(&book, waiting), BookStatus::Canceled);
    assert_eq!((book.balance("BTC").free, book.balance("BTC").locked), (d("1"), Decimal::ZERO));
    let before = (state(&book), book.events_total());
    assert_eq!(book.apply_delayed_cancels(), 0);
    assert!(book.queued_cancels().is_empty());
    assert_eq!(state(&book), before.0, "a second application changes nothing");
    assert_eq!(book.events_total(), before.1);
    assert_conserved(&book);
}

/// A queued cancel whose order was cancelled meanwhile releases nothing more.
#[test]
fn a_queued_cancel_of_an_order_cancelled_meanwhile_changes_nothing() {
    let book = book();
    book.set_balance("BTC", d("2"));
    let id = stop(&book, "1", "90", "a");
    stop(&book, "1", "90", "b");
    book.cross(id, d("0.25")).unwrap();
    book.delay_cancels(true);
    assert_eq!(book.cancel("BTCUSDT", id), Err(BookError::Lost));
    book.delay_cancels(false);
    assert_eq!(book.cancel("BTCUSDT", id).unwrap().status, BookStatus::Canceled);
    let before = state(&book);
    assert_eq!(book.apply_delayed_cancels(), 0);
    assert_eq!(state(&book), before);
    assert_eq!((book.balance("BTC").free, book.balance("BTC").locked), (d("0.75"), d("1")));
    let ended = book
        .events()
        .into_iter()
        .filter(|e| matches!(e, BookEvent::Ended { .. }))
        .count();
    assert_eq!(ended, 1);
    assert_conserved(&book);
}

/// A placement to try, and the refusal it must get.
type Refused<'a> = (Box<dyn Fn() -> Result<Placement, BookError> + 'a>, BookError);

/// The venue's refusals change nothing (errors.md L485, L487, L500, L546,
/// L552-L553, L177).
#[test]
fn refusals_change_nothing() {
    let book = book();
    book.set_balance("BTC", d("1"));
    book.set_balance("USDT", d("50"));
    stop(&book, "0.5", "90", "held");
    let cases: Vec<Refused<'_>> = vec![
        (
            Box::new(|| book.place_stop("BTCUSDT", d("0.1"), d("100"), "x")),
            rejected(-2010, "Order would trigger immediately."),
        ),
        (
            Box::new(|| book.place_stop("BTCUSDT", d("0.1"), d("120"), "x")),
            rejected(-2010, "Order would trigger immediately."),
        ),
        (
            Box::new(|| book.place_stop("BTCUSDT", d("0.6"), d("90"), "x")),
            rejected(-2010, INSUFFICIENT),
        ),
        (
            Box::new(|| book.place_market("BTCUSDT", Side::Sell, d("0.6"), "x")),
            rejected(-2010, INSUFFICIENT),
        ),
        (
            Box::new(|| book.place_market("BTCUSDT", Side::Buy, d("0.6"), "x")),
            rejected(-2010, INSUFFICIENT),
        ),
        (
            Box::new(|| book.place_stop("BTCUSDT", d("0.1"), d("90"), "held")),
            rejected(-2010, "Duplicate order sent."),
        ),
        (
            Box::new(|| book.place_market("XRPUSDT", Side::Buy, d("1"), "x")),
            rejected(-1121, "Invalid symbol."),
        ),
        (
            Box::new(|| book.place_market("BTCUSDT", Side::Buy, d("0"), "x")),
            rejected(-1013, "Filter failure: LOT_SIZE"),
        ),
        (
            Box::new(|| book.place_stop("BTCUSDT", d("0.1"), d("0"), "x")),
            rejected(-1013, "Filter failure: PRICE_FILTER"),
        ),
        (
            Box::new(|| book.place_market("BTCUSDT", Side::Buy, Decimal::MAX, "x")),
            rejected(-1013, "Filter failure: NOTIONAL"),
        ),
    ];
    for (i, (place, expected)) in cases.into_iter().enumerate() {
        let before = state(&book);
        assert_eq!(place().map(|p| p.order.id), Err(expected), "case {i}");
        assert_eq!(state(&book), before, "case {i} changed nothing");
    }
    assert_conserved(&book);
}

const INSUFFICIENT: &str = "Account has insufficient balance for requested action.";

#[test]
fn a_scripted_commission_is_charged_in_its_asset_or_falls_back() {
    let book = book();
    book.set_balance("USDT", d("1000"));
    book.set_balance("BNB", d("1"));
    book.script_next(Script::Commission { asset: "BNB".into(), amount: d("0.05") });
    let fill = book.place_market("BTCUSDT", Side::Buy, d("1"), "a").unwrap().fills.remove(0);
    assert_eq!((fill.commission, fill.commission_asset.as_str()), (d("0.05"), "BNB"));
    assert_eq!(book.balance("BTC").free, d("1"), "the base is received whole");
    assert_eq!(book.balance("BNB").free, d("0.95"));
    book.script_next(Script::Commission { asset: "BNB".into(), amount: d("5") });
    let fill = book.place_market("BTCUSDT", Side::Buy, d("1"), "b").unwrap().fills.remove(0);
    assert_eq!(
        (fill.commission, fill.commission_asset.as_str()),
        (d("0.001"), "BTC"),
        "not covered"
    );
    assert_eq!(book.balance("BNB").free, d("0.95"));
    assert_eq!(book.balance("BTC").free, d("1.999"));
    assert_conserved(&book);
}

#[test]
fn placement_scripts_reject_lose_or_expire() {
    let book = book();
    book.set_balance("USDT", d("1000"));
    book.script_next(Script::Reject { code: -2010, msg: "scripted".into() });
    assert_eq!(
        book.place_market("BTCUSDT", Side::Buy, d("1"), "r"),
        Err(rejected(-2010, "scripted"))
    );
    assert!(book.all_orders().is_empty());

    book.script_next(Script::Lost);
    assert_eq!(book.place_market("BTCUSDT", Side::Buy, d("1"), "lost"), Err(BookError::Lost));
    let found = book.order("BTCUSDT", OrderRef::Client("lost")).unwrap();
    assert_eq!(found.status, BookStatus::Filled, "the book acted before the answer was lost");

    book.script_next(Script::Lost);
    assert_eq!(
        book.place_market("BTCUSDT", Side::Buy, d("100"), "lost-refused"),
        Err(BookError::Lost)
    );
    assert_eq!(
        book.order("BTCUSDT", OrderRef::Client("lost-refused")),
        Err(BookError::NoSuchOrder)
    );

    let usdt = book.balance("USDT").free;
    book.script_next(Script::ExpireUnfilled);
    let unfilled = book.place_market("BTCUSDT", Side::Buy, d("1"), "u").unwrap();
    assert_eq!(
        (unfilled.order.status, unfilled.order.executed_qty),
        (BookStatus::Expired, Decimal::ZERO)
    );
    assert!(unfilled.fills.is_empty());
    assert_eq!(book.balance("USDT").free, usdt);

    book.script_next(Script::ExpirePartial(d("0.4")));
    let partial = book.place_market("BTCUSDT", Side::Buy, d("1"), "p").unwrap();
    assert_eq!(
        (partial.order.status, partial.order.executed_qty),
        (BookStatus::Expired, d("0.4"))
    );
    assert_eq!(book.balance("USDT").free, usdt - d("40"));
    assert_conserved(&book);
}

/// filters.md L253-L272: both limits count per symbol; a cancel frees a slot
/// on its own symbol only.
#[test]
fn order_limits_are_per_symbol() {
    let book = book();
    book.set_balance("BTC", d("10"));
    book.set_balance("ETH", d("10"));
    book.set_balance("USDT", d("1000"));
    let btc: Vec<u64> = (0..5).map(|i| stop(&book, "0.1", "90", &format!("b{i}"))).collect();
    let algo = rejected(-1013, "Filter failure: MAX_NUM_ALGO_ORDERS");
    assert_eq!(
        book.place_stop("BTCUSDT", d("0.1"), d("90"), "b5").map(|p| p.order.id),
        Err(algo.clone())
    );
    for i in 0..3 {
        book.place_stop("ETHUSDT", d("1"), d("9"), &format!("e{i}")).unwrap();
    }
    let orders = rejected(-1013, "Filter failure: MAX_NUM_ORDERS");
    assert_eq!(
        book.place_stop("ETHUSDT", d("1"), d("9"), "e3").map(|p| p.order.id),
        Err(orders.clone())
    );
    assert_eq!(
        book.place_market("ETHUSDT", Side::Buy, d("1"), "em").map(|p| p.order.id),
        Err(orders.clone())
    );
    book.place_market("BTCUSDT", Side::Buy, d("1"), "bm").unwrap();

    book.cancel("BTCUSDT", btc[0]).unwrap();
    book.place_stop("BTCUSDT", d("0.1"), d("90"), "b6").unwrap();
    assert_eq!(
        book.place_stop("BTCUSDT", d("0.1"), d("90"), "b7").map(|p| p.order.id),
        Err(algo)
    );
    assert_eq!(
        book.place_stop("ETHUSDT", d("1"), d("9"), "e4").map(|p| p.order.id),
        Err(orders)
    );
}

/// §7.4: a full book evicts its oldest terminal order with its trades, its
/// client-id entry and its queued cancel.
#[test]
fn retention_evicts_the_oldest_terminal_order() {
    let book = SpotBook::new(vec![BookRules::new("BTCUSDT", "BTC", "USDT")], d("0.001"));
    book.set_price("BTCUSDT", d("100"));
    book.set_balance("BTC", d("1"));
    book.set_balance("USDT", d("1000"));
    let oldest = stop(&book, "1", "90", "oldest");
    book.delay_cancels(true);
    assert_eq!(book.cancel("BTCUSDT", oldest), Err(BookError::Lost));
    book.cross(oldest, d("1")).unwrap();
    assert_eq!(book.queued_cancels(), [oldest], "overtaken, still queued");
    for i in 1..MAX_RETAINED_ORDERS {
        book.place_market("BTCUSDT", Side::Buy, d("0.001"), &format!("m{i}")).unwrap();
    }
    assert_eq!(book.all_orders().len(), MAX_RETAINED_ORDERS);
    let trades = book.trades_retained();
    let next = book.place_market("BTCUSDT", Side::Buy, d("0.001"), "next").unwrap().order.id;
    assert_eq!(book.all_orders().len(), MAX_RETAINED_ORDERS);
    assert_eq!(book.all_orders()[0].id, oldest + 1);
    assert_eq!(book.trades_retained(), trades, "one trade evicted, one added");
    assert!(book.trades("BTCUSDT", Some(oldest), Some(0), 1000).is_empty());
    assert_eq!(book.order("BTCUSDT", OrderRef::Client("oldest")), Err(BookError::NoSuchOrder));
    assert_eq!(book.order("BTCUSDT", OrderRef::Id(oldest)), Err(BookError::NoSuchOrder));
    assert!(book.queued_cancels().is_empty());
    assert!(book.events().contains(&BookEvent::Evicted { order_id: oldest }));
    assert_eq!(book.order("BTCUSDT", OrderRef::Id(next)).unwrap().status, BookStatus::Filled);
    assert_eq!(book.events().len(), MAX_EVENTS, "the ring keeps the last events");
    assert!(book.events_total() > MAX_EVENTS as u64);
}

#[test]
#[should_panic(expected = "retention bound, all 4096 retained orders are live")]
fn retention_with_every_order_live_is_a_test_resource_error() {
    let book = SpotBook::new(vec![BookRules::new("BTCUSDT", "BTC", "USDT")], d("0.001"));
    book.set_price("BTCUSDT", d("100"));
    book.set_balance("BTC", d("5000"));
    for i in 0..=MAX_RETAINED_ORDERS {
        stop(&book, "1", "90", &format!("s{i}"));
    }
}

/// §3.2: an order's visibility and its trades' visibility are independent.
#[test]
fn order_and_trade_visibility_are_independent() {
    let book = book();
    book.set_balance("USDT", d("1000"));
    book.set_balance("BTC", d("1"));
    let id = book.place_market("BTCUSDT", Side::Buy, d("1"), "a").unwrap().order.id;
    assert!(book.withhold_trades(id));
    assert_eq!(book.order("BTCUSDT", OrderRef::Id(id)).unwrap().status, BookStatus::Filled);
    assert!(book.trades("BTCUSDT", Some(id), None, 10).is_empty());
    assert!(book.hide_order(id) && book.release_trades(id));
    assert_eq!(book.order("BTCUSDT", OrderRef::Id(id)), Err(BookError::NoSuchOrder));
    assert_eq!(book.trades("BTCUSDT", Some(id), None, 10).len(), 1);
    let resting = stop(&book, "1", "90", "s");
    assert!(book.hide_order(resting));
    assert!(book.open_orders(None).is_empty());
    assert_eq!(
        book.cancel("BTCUSDT", resting).unwrap().status,
        BookStatus::Canceled,
        "the engine sees it"
    );
    assert!(book.show_order(id));
    assert_eq!(book.order("BTCUSDT", OrderRef::Client("a")).unwrap().id, id);
    assert!(!book.hide_order(999));
}

/// errors.md L451-L453: only a canceled or expired order with nothing
/// executed is archived; its lookups answer `-2026`.
#[test]
fn archive_takes_only_unexecuted_canceled_or_expired_orders() {
    let book = book();
    book.set_balance("BTC", d("3"));
    let canceled = stop(&book, "1", "90", "a");
    let partial = stop(&book, "1", "90", "b");
    let live = stop(&book, "1", "90", "c");
    book.cancel("BTCUSDT", canceled).unwrap();
    book.cross(partial, d("0.5")).unwrap();
    book.cancel("BTCUSDT", partial).unwrap();
    assert!(!book.archive(partial) && !book.archive(live) && !book.archive(999));
    assert!(book.archive(canceled));
    assert!(!book.archive(canceled), "once");
    assert_eq!(book.order("BTCUSDT", OrderRef::Id(canceled)), Err(BookError::Archived));
    assert_eq!(BookError::Archived.code(), Some(-2026));
    assert_eq!(book.cancel("BTCUSDT", canceled), Err(BookError::NotOpen));
}

#[test]
fn trade_pages_follow_my_trades() {
    let book = book();
    book.set_balance("USDT", d("1000"));
    let ids: Vec<u64> = (0..5)
        .map(|i| {
            book.place_market("BTCUSDT", Side::Buy, d("0.1"), &format!("m{i}"))
                .unwrap()
                .fills[0]
                .id
        })
        .collect();
    let page = |order, from, limit| -> Vec<u64> {
        book.trades("BTCUSDT", order, from, limit).into_iter().map(|t| t.id).collect()
    };
    assert_eq!(page(None, None, 2), ids[3..], "most recent, ascending");
    assert_eq!(page(None, Some(ids[1]), 2), ids[1..3], ">= fromId");
    assert_eq!(page(None, Some(ids[1]), 0), ids[1..2], "limit at least 1");
    assert_eq!(page(Some(2), None, 10), [ids[1]]);
    assert!(book.trades("ETHUSDT", None, None, 10).is_empty());
}

#[test]
fn an_order_takes_at_most_its_trade_bound() {
    let book = book();
    book.set_balance("BTC", d("1"));
    let id = stop(&book, "1", "90", "a");
    for _ in 0..MAX_TRADES_PER_ORDER {
        book.cross(id, d("0.001")).unwrap();
    }
    assert_eq!(book.cross(id, d("0.001")), Err(CrossRefused::TradeBound));
    assert_eq!(book.trades("BTCUSDT", Some(id), Some(0), 1000).len(), MAX_TRADES_PER_ORDER);
}

#[test]
#[should_panic(expected = "64 scripts queued")]
fn scripts_are_bounded() {
    let book = book();
    for _ in 0..=MAX_SCRIPTS {
        book.script_next(Script::Lost);
    }
}

#[test]
#[should_panic(expected = "no price set for BTCUSDT")]
fn a_market_order_without_a_price_is_a_test_error() {
    let book = SpotBook::new(vec![BookRules::btcusdt()], d("0.001"));
    let _ = book.place_market("BTCUSDT", Side::Buy, d("1"), "a");
}

#[test]
fn a_client_id_names_its_latest_order_and_is_reused_only_when_terminal() {
    let book = book();
    book.set_balance("USDT", d("1000"));
    let first = book.place_market("BTCUSDT", Side::Buy, d("1"), "same").unwrap().order.id;
    let second = book.place_market("BTCUSDT", Side::Buy, d("1"), "same").unwrap().order.id;
    assert_ne!(first, second);
    assert_eq!(book.order("BTCUSDT", OrderRef::Client("same")).unwrap().id, second);
    assert_eq!(book.order("ETHUSDT", OrderRef::Client("same")), Err(BookError::NoSuchOrder));
}

/// §3.4: no clock: the same commands give the same book.
#[test]
fn the_same_commands_give_the_same_book() {
    let run = || {
        let book = book();
        book.set_balance("BTC", d("1"));
        book.set_balance("USDT", d("500"));
        let id = stop(&book, "0.5", "90", "s");
        book.place_market("BTCUSDT", Side::Buy, d("0.2"), "m").unwrap();
        book.set_price("BTCUSDT", d("80"));
        book.cross(id, d("0.2")).unwrap();
        book.delay_cancels(true);
        let _ = book.cancel("BTCUSDT", id);
        book.apply_delayed_cancels();
        (
            book.all_orders(),
            book.events(),
            book.trades("BTCUSDT", None, Some(0), 1000),
            book.now_ms(),
        )
    };
    assert_eq!(run(), run());
}

//! Generated model tests for `SpotBook` (8d plan §7.2): seeded command
//! sequences over two symbols that share their quote asset. After every
//! command the invariants are checked from the public view and the answers,
//! never from the engine's own bookkeeping:
//! - every `free` and `locked` is ≥ 0; each base's `locked` is the sum of its
//!   live stops' `orig − executed`; the quote and fee assets lock nothing;
//! - per asset, the account, the market and the fee sink sum to zero;
//! - each balance moves exactly as the command's fills, stop placements and
//!   releases say, and each fill's commission follows the default rule (a BUY
//!   in base at `qty × rate`, a SELL in quote at `notional × rate`) or the
//!   commission script it consumed;
//! - executed quantities only grow and never pass the original; a terminal
//!   order never changes again, so nothing executes after a definite cancel;
//!   `New` has nothing executed, `PartiallyFilled` some, `Filled` all;
//! - each order ends once, with one event, and a stop's end releases exactly
//!   its `orig − executed`;
//! - a refused command changes no order and no balance; a cross of zero is
//!   refused; open orders and stops stay within each symbol's limits, and a
//!   limit refusal happens only at the limit;
//! - applying the delayed cancels empties the queue, and applying them again
//!   changes nothing.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rust_decimal::Decimal;

use crate::spot_book::{
    BookError, BookEvent, BookOrder, BookRules, BookStatus, BookTrade, OrderKind, OrderRef, Script,
    Side, SpotBook,
};

const SEEDS: u64 = 200;
const COMMANDS: usize = 200;
/// 0.001.
const FEE: Decimal = Decimal::from_parts(1, 0, 0, false, 3);
const QUOTE: &str = "USDT";
const ASSETS: [&str; 4] = ["BTC", "ETH", "USDT", "BNB"];

struct Market {
    symbol: &'static str,
    base: &'static str,
    max_orders: u32,
    max_stops: u32,
    /// The quantity step, as an integer and a scale.
    unit: (i64, u32),
    /// Prices drawn from `low..=high`, in cents.
    cents: (u64, u64),
}

const MARKETS: [Market; 2] = [
    Market {
        symbol: "BTCUSDT",
        base: "BTC",
        max_orders: 200,
        max_stops: 5,
        unit: (1, 2),
        cents: (5_000, 15_000),
    },
    Market {
        symbol: "ETHUSDT",
        base: "ETH",
        max_orders: 3,
        max_stops: 4,
        unit: (1, 1),
        cents: (500, 1_500),
    },
];

fn market(symbol: &str) -> &'static Market {
    MARKETS.iter().find(|m| m.symbol == symbol).expect("a modeled symbol")
}

/// xorshift64, as the other generated tests: no dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        let mut rng = Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        rng.next();
        rng
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn one_in(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }
}

#[derive(Clone, Debug)]
enum Command {
    Place {
        m: usize,
        side: Side,
        qty: Decimal,
        trigger: Option<Decimal>,
        script: Option<Script>,
    },
    Cross {
        id: u64,
        qty: Decimal,
    },
    Expire {
        id: u64,
    },
    Cancel {
        m: usize,
        id: u64,
        delayed: bool,
    },
    ApplyDelayed,
    Price {
        m: usize,
        price: Decimal,
    },
    Hide {
        id: u64,
        hidden: bool,
    },
    Withhold {
        id: u64,
        withheld: bool,
    },
    Archive {
        id: u64,
    },
    Commission {
        amount: Decimal,
    },
}

/// Free and locked, per asset.
type Balances = BTreeMap<&'static str, (Decimal, Decimal)>;

/// Everything compared across a command.
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    orders: BTreeMap<u64, BookOrder>,
    balances: Balances,
    prices: Vec<Decimal>,
    queued: Vec<u64>,
    events_total: u64,
}

impl Snapshot {
    fn take(book: &SpotBook) -> Self {
        Self {
            orders: book.all_orders().into_iter().map(|o| (o.id, o)).collect(),
            balances: ASSETS
                .iter()
                .map(|a| (*a, (book.balance(a).free, book.balance(a).locked)))
                .collect(),
            prices: MARKETS.iter().map(|m| book.price(m.symbol).expect("seeded")).collect(),
            queued: book.queued_cancels(),
            events_total: book.events_total(),
        }
    }

    /// Orders and balances: what a refused command must leave as it was.
    fn account(&self) -> (&BTreeMap<u64, BookOrder>, &Balances) {
        (&self.orders, &self.balances)
    }

    fn live(&self, symbol: &str) -> (u32, u32) {
        let live = self.orders.values().filter(|o| o.symbol == symbol && !o.status.is_terminal());
        let (mut orders, mut stops) = (0, 0);
        for order in live {
            orders += 1;
            stops += u32::from(matches!(order.kind, OrderKind::StopLoss { .. }));
        }
        (orders, stops)
    }
}

/// What the command's answers said.
#[derive(Default)]
struct Answers {
    /// The command was refused: nothing may change.
    refused: bool,
    fills: Vec<BookTrade>,
    /// The cases reached, each named only after its post-state was
    /// asserted; credited once every other check has passed.
    cases: Vec<&'static str>,
}

/// The order as the book holds it now, whatever its visibility.
fn order_now(book: &SpotBook, id: u64) -> BookOrder {
    book.all_orders()
        .into_iter()
        .find(|o| o.id == id)
        .unwrap_or_else(|| panic!("order {id} is not retained"))
}

fn steps_of(m: &Market, steps: u64) -> Decimal {
    Decimal::new(m.unit.0 * steps as i64, m.unit.1)
}

fn generate(rng: &mut Rng, before: &Snapshot) -> Command {
    let m = rng.below(2) as usize;
    let market = &MARKETS[m];
    let price = before.prices[m];
    let last = before.orders.keys().next_back().copied().unwrap_or(0);
    let id = rng.below(last + 2);
    match rng.below(100) {
        0..=19 => {
            let side = if rng.one_in(2) { Side::Buy } else { Side::Sell };
            let qty = steps_of(market, 1 + rng.below(30));
            let script = match rng.below(12) {
                0 => Some(Script::Reject { code: -2010, msg: "scripted".into() }),
                1 => Some(Script::Lost),
                2 => Some(Script::ExpireUnfilled),
                3 if qty > qty_unit(market) => Some(Script::ExpirePartial(qty_unit(market))),
                _ => None,
            };
            Command::Place { m, side, qty, trigger: None, script }
        },
        20..=39 => {
            let qty = steps_of(market, 1 + rng.below(30));
            // 70% to 105% of the price, so some triggers are at or above it.
            let trigger = (price * Decimal::new(70 + rng.below(36) as i64, 2)).round_dp(2);
            let script = match rng.below(12) {
                0 => Some(Script::Reject { code: -2010, msg: "scripted".into() }),
                1 => Some(Script::Lost),
                _ => None,
            };
            Command::Place {
                m,
                side: Side::Sell,
                qty,
                trigger: Some(trigger),
                script,
            }
        },
        40..=54 => {
            let qty = match before.orders.get(&id) {
                Some(order) => {
                    let rest = order.orig_qty - order.executed_qty;
                    let unit = qty_unit(market);
                    match rng.below(6) {
                        0 => Decimal::ZERO,
                        1 => rest,
                        2 => rest + unit,
                        3 => -unit,
                        _ => (rest / Decimal::TWO).round_dp(qty_scale(&order.symbol)),
                    }
                },
                None => qty_unit(market),
            };
            Command::Cross { id, qty }
        },
        55..=66 => {
            let m = match before.orders.get(&id) {
                Some(order) if !rng.one_in(8) => {
                    MARKETS.iter().position(|x| x.symbol == order.symbol).unwrap()
                },
                _ => m,
            };
            Command::Cancel { m, id, delayed: rng.one_in(3) }
        },
        67..=74 => {
            let cents = market.cents.0 + rng.below(market.cents.1 - market.cents.0 + 1);
            Command::Price { m, price: Decimal::new(cents as i64, 2) }
        },
        75..=79 => Command::ApplyDelayed,
        80..=84 => Command::Expire { id },
        85..=88 => Command::Hide { id, hidden: rng.one_in(2) },
        89..=92 => Command::Withhold { id, withheld: rng.one_in(2) },
        93..=96 => Command::Archive { id },
        _ => Command::Commission {
            amount: Decimal::new(1 + rng.below(30) as i64, 3),
        },
    }
}

fn qty_unit(m: &Market) -> Decimal {
    steps_of(m, 1)
}

fn qty_scale(symbol: &str) -> u32 {
    market(symbol).unit.1
}

/// The model's own state across commands.
#[derive(Default)]
struct Model {
    /// Commission scripts queued, consumed one per fill.
    commissions: VecDeque<(String, Decimal)>,
    /// Terminal transitions seen per order.
    ended: BTreeMap<u64, u32>,
    /// Orders a definite cancel ended.
    cancelled: BTreeSet<u64>,
    /// How often each case was reached, over every seed.
    coverage: BTreeMap<&'static str, u64>,
}

impl Model {
    fn hit(&mut self, case: &'static str) {
        *self.coverage.entry(case).or_default() += 1;
    }
}

/// Cases the generator must reach, so no invariant holds vacuously.
const CASES: &[&str] = &[
    "market filled",
    "market expired by script",
    "stop placed",
    "refused: insufficient balance",
    "refused: would trigger immediately",
    "refused: MAX_NUM_ORDERS",
    "refused: MAX_NUM_ALGO_ORDERS",
    "refused: scripted",
    "lost, applied",
    "lost, refused",
    "cross: partial",
    "cross: filling",
    "cross refused",
    "cancel: immediate",
    "cancel: queued",
    "cancel: not open",
    "delayed cancel applied",
    "delayed cancel overtaken",
    "expired",
    "archived",
    "commission: scripted",
    "commission: fallen back",
    "release of a partial stop",
];

fn run(
    book: &SpotBook,
    model: &mut Model,
    command: &Command,
    step: usize,
    before: &Snapshot,
) -> Answers {
    let mut answers = Answers::default();
    let client_id = format!("c{step}");
    match command.clone() {
        Command::Place { m, side, qty, trigger, script } => {
            let symbol = MARKETS[m].symbol;
            if let Some(script) = script.clone() {
                book.script_next(script);
            }
            let placed = match trigger {
                Some(trigger) => book.place_stop(symbol, qty, trigger, &client_id),
                None => book.place_market(symbol, side, qty, &client_id),
            };
            match placed {
                Ok(placement) => {
                    let order = &placement.order;
                    assert_eq!(order.orig_qty, qty);
                    assert_eq!(&order_now(book, order.id), order, "the answer is the stored order");
                    assert!(!before.orders.contains_key(&order.id), "a new order");
                    let (orders, stops) = before.live(symbol);
                    assert!(orders < MARKETS[m].max_orders, "accepted under the order limit");
                    assert!(
                        trigger.is_none() || stops < MARKETS[m].max_stops,
                        "accepted under the stop limit"
                    );
                    let filled: Decimal = placement.fills.iter().map(|f| f.qty).sum();
                    assert_eq!(filled, order.executed_qty, "the fills are the execution");
                    let state = (order.status, order.executed_qty);
                    if let Some(trigger) = trigger {
                        assert_eq!(state, (BookStatus::New, Decimal::ZERO));
                        assert!(trigger < before.prices[m], "a stop rests only below the price");
                        answers.cases.push("stop placed");
                    } else {
                        match script {
                            Some(Script::ExpireUnfilled) => {
                                assert_eq!(state, (BookStatus::Expired, Decimal::ZERO));
                                answers.cases.push("market expired by script");
                            },
                            Some(Script::ExpirePartial(part)) => {
                                assert_eq!(state, (BookStatus::Expired, part));
                                answers.cases.push("market expired by script");
                            },
                            _ => {
                                assert_eq!(state, (BookStatus::Filled, qty));
                                answers.cases.push("market filled");
                            },
                        }
                    }
                    answers.fills = placement.fills;
                },
                Err(BookError::Lost) => {
                    assert_eq!(script, Some(Script::Lost), "lost only when scripted");
                    // The book acted, or refused: whatever it did is read back.
                    match book.order(symbol, OrderRef::Client(&client_id)) {
                        Ok(order) => {
                            assert!(!before.orders.contains_key(&order.id), "a new order");
                            let state = (order.status, order.executed_qty);
                            if trigger.is_some() {
                                assert_eq!(state, (BookStatus::New, Decimal::ZERO));
                            } else {
                                assert_eq!(state, (BookStatus::Filled, qty));
                            }
                            answers.fills = book.trades(symbol, Some(order.id), Some(0), 1_000);
                            answers.cases.push("lost, applied");
                        },
                        Err(BookError::NoSuchOrder) => {
                            answers.refused = true;
                            answers.cases.push("lost, refused");
                        },
                        Err(other) => panic!("a fresh order looked up as {other:?}"),
                    }
                },
                Err(BookError::Rejected { msg, .. }) => {
                    answers.refused = true;
                    let (orders, stops) = before.live(symbol);
                    if msg.contains("MAX_NUM_ORDERS") {
                        assert_eq!(orders, MARKETS[m].max_orders, "refused only at the limit");
                    }
                    if msg.contains("MAX_NUM_ALGO_ORDERS") {
                        assert_eq!(stops, MARKETS[m].max_stops, "refused only at the limit");
                    }
                    answers.cases.push(match msg.as_str() {
                        "Account has insufficient balance for requested action." => {
                            "refused: insufficient balance"
                        },
                        "Order would trigger immediately." => "refused: would trigger immediately",
                        "Filter failure: MAX_NUM_ORDERS" => "refused: MAX_NUM_ORDERS",
                        "Filter failure: MAX_NUM_ALGO_ORDERS" => "refused: MAX_NUM_ALGO_ORDERS",
                        "scripted" => "refused: scripted",
                        other => panic!("an unexpected refusal: {other}"),
                    });
                },
                Err(other) => panic!("a placement answered {other:?}"),
            }
        },
        Command::Cross { id, qty } => match book.cross(id, qty) {
            Ok(trade) => {
                assert!(qty > Decimal::ZERO, "a cross of {qty} was accepted");
                assert_eq!(trade.qty, qty);
                let symbol = MARKETS.iter().position(|m| m.symbol == trade.symbol).unwrap();
                assert_eq!(trade.price, before.prices[symbol]);
                let (old, now) = (&before.orders[&id], order_now(book, id));
                assert_eq!(now.executed_qty, old.executed_qty + qty);
                assert_eq!(now.quote_qty, old.quote_qty + trade.quote_qty);
                if qty == old.orig_qty - old.executed_qty {
                    assert_eq!(now.status, BookStatus::Filled, "a cross of the rest fills");
                    answers.cases.push("cross: filling");
                } else {
                    assert_eq!(now.status, BookStatus::PartiallyFilled);
                    answers.cases.push("cross: partial");
                }
                answers.fills.push(trade);
            },
            Err(_) => {
                answers.refused = true;
                answers.cases.push("cross refused");
            },
        },
        Command::Expire { id } => {
            answers.refused = !book.expire(id);
            if !answers.refused {
                let (old, now) = (&before.orders[&id], order_now(book, id));
                assert!(!old.status.is_terminal());
                assert_eq!((now.status, now.executed_qty), (BookStatus::Expired, old.executed_qty));
                answers.cases.push("expired");
            }
        },
        Command::Cancel { m, id, delayed } => {
            book.delay_cancels(delayed);
            let symbol = MARKETS[m].symbol;
            let open = before
                .orders
                .get(&id)
                .is_some_and(|o| o.symbol == symbol && !o.status.is_terminal());
            match book.cancel(symbol, id) {
                Ok(order) => {
                    assert!(open && !delayed);
                    let now = order_now(book, id);
                    assert_eq!(order, now, "the answer is the stored order");
                    let executed = before.orders[&id].executed_qty;
                    assert_eq!((now.status, now.executed_qty), (BookStatus::Canceled, executed));
                    model.cancelled.insert(id);
                    answers.cases.push("cancel: immediate");
                },
                Err(BookError::Lost) => {
                    assert!(open && delayed);
                    assert!(book.queued_cancels().contains(&id), "queued");
                    answers.refused = true;
                    answers.cases.push("cancel: queued");
                },
                Err(BookError::NotOpen) => {
                    assert!(!open);
                    answers.refused = true;
                    answers.cases.push("cancel: not open");
                },
                Err(other) => panic!("a cancel answered {other:?}"),
            }
        },
        Command::ApplyDelayed => {
            let cancelled = book.apply_delayed_cancels();
            assert!(book.queued_cancels().is_empty());
            let mut live = 0;
            for (id, old) in &before.orders {
                let now = order_now(book, *id);
                let state = |o: &BookOrder| (o.status, o.executed_qty);
                if !before.queued.contains(id) || old.status.is_terminal() {
                    assert_eq!(state(&now), state(old), "order {id} untouched");
                    if before.queued.contains(id) {
                        answers.cases.push("delayed cancel overtaken");
                    }
                } else {
                    let cancelled = (BookStatus::Canceled, old.executed_qty);
                    assert_eq!(state(&now), cancelled, "queued order {id} cancelled");
                    live += 1;
                    answers.cases.push("delayed cancel applied");
                }
            }
            assert_eq!(cancelled, live, "the count is the live orders cancelled");
        },
        Command::Price { m, price } => book.set_price(MARKETS[m].symbol, price),
        Command::Hide { id, hidden } => {
            let known = if hidden {
                book.hide_order(id)
            } else {
                book.show_order(id)
            };
            answers.refused = !known;
        },
        Command::Withhold { id, withheld } => {
            let known = if withheld {
                book.withhold_trades(id)
            } else {
                book.release_trades(id)
            };
            answers.refused = !known;
        },
        Command::Archive { id } => {
            answers.refused = !book.archive(id);
            if !answers.refused {
                let (old, now) = (&before.orders[&id], order_now(book, id));
                assert!(matches!(old.status, BookStatus::Canceled | BookStatus::Expired));
                assert!(old.executed_qty == Decimal::ZERO && !old.archived && now.archived);
                let symbol = now.symbol.as_str();
                let lookup = book.order(symbol, OrderRef::Id(id));
                let hidden = !now.order_visible;
                assert_eq!(
                    lookup,
                    Err(if hidden {
                        BookError::NoSuchOrder
                    } else {
                        BookError::Archived
                    })
                );
                answers.cases.push("archived");
            }
        },
        Command::Commission { amount } => {
            if model.commissions.len() < 8 {
                book.script_next(Script::Commission { asset: "BNB".into(), amount });
                model.commissions.push_back(("BNB".into(), amount));
            }
        },
    }
    answers
}

/// Every invariant, from `before`, `after`, the answers and the events.
fn check(
    book: &SpotBook,
    model: &mut Model,
    before: &Snapshot,
    after: &Snapshot,
    answers: &Answers,
) {
    let mut credits = answers.cases.clone();
    // Balances: non-negative, locks exactly the live stops' rest, conserved.
    let mut locks: BTreeMap<&str, Decimal> = BTreeMap::new();
    for order in after.orders.values() {
        assert!(
            order.executed_qty >= Decimal::ZERO && order.executed_qty <= order.orig_qty,
            "{order:?}"
        );
        match order.status {
            BookStatus::New => assert_eq!(order.executed_qty, Decimal::ZERO),
            BookStatus::PartiallyFilled => assert!(
                order.executed_qty > Decimal::ZERO && order.executed_qty < order.orig_qty,
                "{order:?}"
            ),
            BookStatus::Filled => assert_eq!(order.executed_qty, order.orig_qty),
            BookStatus::Canceled | BookStatus::Expired => {},
        }
        if matches!(order.kind, OrderKind::StopLoss { .. }) && !order.status.is_terminal() {
            *locks.entry(market(&order.symbol).base).or_default() +=
                order.orig_qty - order.executed_qty;
        }
    }
    for asset in ASSETS {
        let (free, locked) = after.balances[asset];
        assert!(free >= Decimal::ZERO && locked >= Decimal::ZERO, "{asset}: {free} {locked}");
        assert_eq!(
            locked,
            locks.get(asset).copied().unwrap_or_default(),
            "{asset} locks its live stops' rest"
        );
        let ledger = book.ledger(asset);
        assert_eq!(
            ledger.free + ledger.locked + ledger.market + ledger.fees,
            Decimal::ZERO,
            "{asset} conserved"
        );
    }
    for m in &MARKETS {
        let (orders, stops) = after.live(m.symbol);
        assert!(orders <= m.max_orders && stops <= m.max_stops, "{} limits", m.symbol);
    }

    // Orders: none lost, execution monotonic, terminal orders frozen.
    for (id, old) in &before.orders {
        let new = after.orders.get(id).unwrap_or_else(|| panic!("order {id} vanished"));
        assert!(new.executed_qty >= old.executed_qty, "order {id} executed went back");
        if old.status.is_terminal() {
            let frozen = |o: &BookOrder| (o.status, o.executed_qty, o.quote_qty);
            assert_eq!(frozen(new), frozen(old), "terminal order {id} changed");
        }
        if model.cancelled.contains(id) {
            assert_eq!(new.executed_qty, old.executed_qty, "order {id} executed after its cancel");
        }
    }
    if answers.refused {
        assert_eq!(after.account(), before.account(), "a refused command changed the account");
    }

    // Events: one end per order, each releasing exactly the rest.
    let fresh = (after.events_total - before.events_total) as usize;
    let events = book.events();
    let events = &events[events.len() - fresh..];
    let mut expected: BTreeMap<&str, (Decimal, Decimal)> = BTreeMap::new();
    let mut entry = |asset: &str, free: Decimal, locked: Decimal| {
        let asset = *ASSETS.iter().find(|a| **a == asset).expect("modeled asset");
        let e = expected.entry(asset).or_default();
        e.0 += free;
        e.1 += locked;
    };
    let mut executed = BTreeSet::new();
    let mut ended_now = BTreeSet::new();
    for event in events {
        match event {
            BookEvent::Ended { order_id, status, unlocked } => {
                let order = &after.orders[order_id];
                assert_eq!(*status, order.status);
                let count = model.ended.entry(*order_id).or_default();
                *count += 1;
                assert_eq!(*count, 1, "order {order_id} ended twice");
                ended_now.insert(*order_id);
                let rest = match order.kind {
                    OrderKind::StopLoss { .. } => order.orig_qty - order.executed_qty,
                    OrderKind::Market => Decimal::ZERO,
                };
                assert_eq!(*unlocked, rest, "order {order_id} released {unlocked}, not its rest");
                if rest > Decimal::ZERO && order.executed_qty > Decimal::ZERO {
                    credits.push("release of a partial stop");
                }
                if *status == BookStatus::Canceled {
                    model.cancelled.insert(*order_id);
                }
                let base = market(&order.symbol).base;
                entry(base, *unlocked, -*unlocked);
            },
            BookEvent::Executed { trade_id, .. } => {
                executed.insert(*trade_id);
            },
            _ => {},
        }
    }
    for (id, order) in &after.orders {
        let was_terminal = before.orders.get(id).is_some_and(|o| o.status.is_terminal());
        if order.status.is_terminal() && !was_terminal {
            assert!(ended_now.contains(id), "order {id} ended without its event");
        }
        if !before.orders.contains_key(id) {
            if let OrderKind::StopLoss { .. } = order.kind {
                let base = market(&order.symbol).base;
                entry(base, -order.orig_qty, order.orig_qty);
            }
        }
    }

    // Fills: each one accounted for, its commission by the rule or its script.
    let filled: BTreeSet<u64> = answers.fills.iter().map(|f| f.id).collect();
    assert_eq!(filled, executed, "the answers show every execution");
    for fill in &answers.fills {
        let m = market(&fill.symbol);
        assert_eq!(fill.quote_qty, fill.qty * fill.price);
        let default = match fill.side {
            Side::Buy => (m.base.to_string(), fill.qty * FEE),
            Side::Sell => (QUOTE.to_string(), fill.quote_qty * FEE),
        };
        let charged = (fill.commission_asset.clone(), fill.commission);
        let from_lock = matches!(after.orders[&fill.order_id].kind, OrderKind::StopLoss { .. });
        match model.commissions.pop_front() {
            Some(scripted) => {
                // The script applies when its asset's free, after this
                // fill's own transfers, covers it.
                let asset = scripted.0.as_str();
                let own = match fill.side {
                    _ if asset != m.base && asset != QUOTE => Decimal::ZERO,
                    Side::Buy if asset == QUOTE => -fill.quote_qty,
                    Side::Buy => fill.qty,
                    Side::Sell if asset == QUOTE => fill.quote_qty,
                    Side::Sell if from_lock => Decimal::ZERO,
                    Side::Sell => -fill.qty,
                };
                let covered = before.balances[asset].0 + own >= scripted.1;
                if covered {
                    assert_eq!(charged, scripted, "the scripted commission, covered");
                    credits.push("commission: scripted");
                } else {
                    assert_eq!(charged, default, "the default commission, not covered");
                    credits.push("commission: fallen back");
                }
            },
            None => assert_eq!(charged, default, "the default commission"),
        }
        match fill.side {
            Side::Buy => {
                entry(QUOTE, -fill.quote_qty, Decimal::ZERO);
                entry(m.base, fill.qty, Decimal::ZERO);
            },
            Side::Sell if from_lock => {
                entry(m.base, Decimal::ZERO, -fill.qty);
                entry(QUOTE, fill.quote_qty, Decimal::ZERO);
            },
            Side::Sell => {
                entry(m.base, -fill.qty, Decimal::ZERO);
                entry(QUOTE, fill.quote_qty, Decimal::ZERO);
            },
        }
        entry(&fill.commission_asset, -fill.commission, Decimal::ZERO);
    }
    for asset in ASSETS {
        let (f0, l0) = before.balances[asset];
        let (f1, l1) = after.balances[asset];
        let (df, dl) = expected.get(asset).copied().unwrap_or_default();
        assert_eq!(
            (f1 - f0, l1 - l0),
            (df, dl),
            "{asset} moved by exactly the fills, locks and releases"
        );
    }
    // Every check passed: the cases count as reached.
    for case in credits {
        model.hit(case);
    }
}

/// Names the failing command when a check panics.
struct Context(String);

impl Drop for Context {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("while checking {}", self.0);
        }
    }
}

/// 200 seeds × 200 commands; every invariant after every command.
#[test]
fn generated_commands_keep_the_book_consistent() {
    let mut commands = 0;
    let mut coverage = BTreeMap::new();
    for seed in 1..=SEEDS {
        let rules = MARKETS
            .iter()
            .map(|m| BookRules::new(m.symbol, m.base, QUOTE).with_limits(m.max_orders, m.max_stops))
            .collect();
        let book = SpotBook::new(rules, FEE);
        book.set_price("BTCUSDT", Decimal::new(100, 0));
        book.set_price("ETHUSDT", Decimal::new(10, 0));
        book.set_balance("BTC", Decimal::new(2, 0));
        book.set_balance("ETH", Decimal::new(20, 0));
        book.set_balance("USDT", Decimal::new(2_000, 0));
        // Enough BNB for a few scripted commissions, so later ones fall back.
        book.set_balance("BNB", Decimal::new(5, 2));
        let mut rng = Rng::new(seed);
        let mut model = Model {
            coverage: std::mem::take(&mut coverage),
            ..Model::default()
        };
        for step in 0..COMMANDS {
            let before = Snapshot::take(&book);
            let command = generate(&mut rng, &before);
            let _context = Context(format!("seed {seed}, step {step}: {command:?}"));
            let answers = run(&book, &mut model, &command, step, &before);
            let after = Snapshot::take(&book);
            check(&book, &mut model, &before, &after, &answers);
            if matches!(command, Command::ApplyDelayed) {
                let again = Snapshot::take(&book);
                assert_eq!(book.apply_delayed_cancels(), 0, "seed {seed}, step {step}");
                let twice = Snapshot::take(&book);
                assert_eq!(
                    twice.account(),
                    again.account(),
                    "seed {seed}, step {step}: applied twice"
                );
                assert_eq!(twice.events_total, again.events_total);
            }
            commands += 1;
        }
        coverage = model.coverage;
    }
    assert_eq!(commands, SEEDS as usize * COMMANDS);
    eprintln!("coverage over {commands} commands: {coverage:?}");
    for case in CASES {
        assert!(coverage.get(case).copied().unwrap_or(0) > 0, "never reached: {case}");
    }
}

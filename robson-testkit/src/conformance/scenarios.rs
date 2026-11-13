//! The scenarios behind each clause (8d plan §4), generic over the venue's
//! harness so every venue of a kind runs the same ones: [`Harness`] for the
//! clauses every venue has, [`BalanceHarness`] for a balance account over a
//! [`SpotBook`], [`PositionHarness`] for a position account. Each scenario
//! starts from a fresh harness (a price of 100, a holding of 1), asserts its
//! preconditions and effects from the venue's own state, and records what it
//! did in its trace.
//!
//! What they claim is §4's: venue-level contracts, the replacement helpers'
//! conservative outcomes and, on balance accounts, that unsafe submissions
//! are suppressed. None claims that a transition ends in protection: that is
//! the core's (8i, 8j).

use std::sync::{Arc, Mutex};

use robson_domain::{OrderSide, Price, Quantity};
use robson_exec::{
    venue::{
        capability_mismatches, AccountKind, AccountPort, CancelAnswer, EntryOrReduce, OrderLookup,
        OrderPhase, PlacedStop, ProtectionKind, ReplaceOutcome, ReplacementProtocol,
        StopCancelOutcome, StopParams, StopRef, TradingPort, Venue,
    },
    ExecError, OrderResult,
};
use rust_decimal::Decimal;

use super::{Coverage, Harness, Outcome, Property::*, Run};
use crate::{
    balance_stub::PlacementHook,
    spot_book::{BookEvent, BookStatus, OrderRef as BookRef, Script, Side as BookSide, SpotBook},
};

/// A balance account's harness: its book, and a hook at the next placement.
pub trait BalanceHarness: Harness {
    /// The venue's book: what the market and the venue do on their own.
    fn book(&self) -> &SpotBook;
    /// Run `hook` as the next placement reaches the venue, before it acts.
    fn on_next_placement(&self, hook: PlacementHook);
}

/// A position account's harness.
pub trait PositionHarness: Harness {
    /// The long position held on the symbol, the venue's own view.
    fn position(&self) -> Decimal;
    /// Run `hook` with the venue's live stops as the next stop is accepted.
    fn on_next_stop_accepted(&self, hook: Box<dyn FnOnce(Vec<String>) + Send>);
}

/// A venue whose next stop cancel can be answered by a script (`Scripted`).
pub trait ScriptsStopCancel: Harness {
    fn script_next_stop_cancel(&self, outcome: StopCancelOutcome);
}

fn d(s: &str) -> Decimal {
    s.parse().expect("a decimal")
}

fn qty(s: &str) -> Quantity {
    Quantity::new(d(s)).expect("a quantity")
}

fn price(s: &str) -> Price {
    Price::new(d(s)).expect("a price")
}

fn ok<T>(result: &Result<T, ExecError>) -> bool {
    result.is_ok()
}

fn replaced(result: &Result<ReplaceOutcome, ExecError>) -> bool {
    matches!(result, Ok(ReplaceOutcome::Replaced { .. }))
}

/// Place a SELL stop through the harness's grant, recording the call.
async fn stop<H: Harness>(
    h: &H,
    run: &mut Run,
    id: &str,
    q: &str,
    trigger: &str,
) -> Option<StopRef> {
    let symbol = h.symbol();
    run.sent(format!("place_protection {id} SELL {q} @ {trigger}"));
    let send =
        Box::pin(h.venue().place_protection(&symbol, OrderSide::Sell, qty(q), price(trigger), id));
    let placed = run.answered("place_protection", h.sending(id, send, ok).await);
    run.expect(placed.is_ok(), format!("stop {id} placed"));
    placed.ok().map(|placed: PlacedStop| placed.stop)
}

/// A market order through the harness's grant.
async fn market<H: Harness>(
    h: &H,
    run: &mut Run,
    id: &str,
    side: OrderSide,
    q: &str,
    intent: EntryOrReduce,
    accepted: fn(&Result<OrderResult, ExecError>) -> bool,
) -> Result<OrderResult, ExecError> {
    let symbol = h.symbol();
    run.sent(format!("place_market_order {id} {side:?} {q} {intent:?}"));
    let send = Box::pin(h.venue().place_market_order(&symbol, side, qty(q), id, intent));
    run.answered("place_market_order", h.sending(id, send, accepted).await)
}

/// Replace `old` with a successor, under the successor's grant.
async fn replace<H: Harness>(
    h: &H,
    run: &mut Run,
    old: &StopRef,
    id: &str,
    q: &str,
    trigger: &str,
) -> Result<ReplaceOutcome, ExecError> {
    let symbol = h.symbol();
    let params = StopParams {
        close_side: OrderSide::Sell,
        quantity: qty(q),
        trigger: price(trigger),
        client_order_id: id.to_string(),
    };
    run.sent(format!(
        "replace_protection {} -> {id} SELL {q} @ {trigger}",
        old.exchange_order_id
    ));
    let send = Box::pin(h.venue().replace_protection(&symbol, old, params));
    run.answered("replace_protection", h.sending(id, send, replaced).await)
}

async fn lookup<H: Harness>(h: &H, run: &mut Run, id: &str) -> Option<OrderLookup> {
    run.sent(format!("order_by_client_id {id}"));
    let found = h.venue().order_by_client_id(&h.symbol(), id).await;
    run.answered("order_by_client_id", found).ok()
}

// Every venue.

/// F1: the capability flags agree with the accessors.
pub async fn f1<H: Harness>(h: &H) -> Outcome {
    let mut run = Run::new(F1, Coverage::Verified);
    run.sent("capability_mismatches");
    let mismatches = run.answered("capability_mismatches", capability_mismatches(h.venue()));
    run.expect(mismatches.is_empty(), format!("mismatches {mismatches:?}"));
    let venue = h.venue();
    run.state(format!(
        "position_account {}, inventory {}, funding {}",
        venue.position_account().is_some(),
        venue.inventory().is_some(),
        venue.funding().is_some()
    ));
    run.finish()
}

/// F2: protection is market-on-trigger.
pub async fn f2<H: Harness>(h: &H) -> Outcome {
    let mut run = Run::new(F2, Coverage::Verified);
    let protection = run.answered("capabilities().protection", h.venue().capabilities().protection);
    run.expect(protection == ProtectionKind::MarketOnTrigger, "protection is MarketOnTrigger");
    run.finish()
}

/// F3: the replacement protocol follows the account kind; a balance account
/// holds no short, sends no reduce-only flag and one attempt per id.
pub async fn f3<H: Harness>(h: &H) -> Outcome {
    let mut run = Run::new(F3, Coverage::Verified);
    let caps = run.answered("capabilities", h.venue().capabilities());
    match caps.account_kind {
        AccountKind::PositionAccount => {
            run.expect(
                caps.replacement == ReplacementProtocol::PlaceThenCancel,
                "position: PlaceThenCancel",
            );
        },
        AccountKind::BalanceAccount => {
            run.expect(
                caps.replacement == ReplacementProtocol::CancelThenPlace,
                "balance: CancelThenPlace",
            );
            run.expect(!caps.supports_short, "balance: no short");
            run.expect(!caps.reduce_only_wire_flag, "balance: no reduce-only flag");
            run.expect(caps.one_attempt_per_client_id, "balance: one attempt per client id");
        },
    }
    run.finish()
}

/// N1: a never-attempted id's not-found is `Unknown`.
pub async fn n1<H: Harness>(h: &H) -> Outcome {
    let mut run = Run::new(N1, Coverage::Verified);
    let found = lookup(h, &mut run, "n1-never").await;
    run.expect(matches!(found, Some(OrderLookup::Unknown)), "a never-attempted id is Unknown");
    run.finish()
}

/// N2: an authoritatively rejected placement's not-found is `Absent` under
/// the venue's finality rule.
pub async fn n2<H: Harness>(h: &H) -> Outcome {
    let mut run = Run::new(N2, Coverage::Verified);
    let before = h.placements();
    h.reject_next_placement();
    run.market("the venue refuses the next placement");
    let symbol = h.symbol();
    run.sent("place_protection n2 SELL 0.1 @ 90");
    let send = Box::pin(h.venue().place_protection(
        &symbol,
        OrderSide::Sell,
        qty("0.1"),
        price("90"),
        "n2",
    ));
    let placed = run.answered("place_protection", h.sending("n2", send, ok).await);
    run.expect(
        matches!(placed, Err(ExecError::OrderRejected(_))),
        "the placement is OrderRejected",
    );
    run.state(format!("placements {} -> {}", before, h.placements()));
    run.expect(h.placements() == before + 1, "the placement reached the venue");
    let finality = h.venue().capabilities().order_finality;
    let found = lookup(h, &mut run, "n2").await;
    run.expect(
        matches!(found, Some(OrderLookup::Absent { finality: f }) if f == finality),
        "the rejected id is Absent under the venue's rule",
    );
    run.finish()
}

/// The lost answer of N3a and N3b: a market BUY of 0.1 that reaches the
/// venue and answers nothing, then concealed.
async fn lose_one<H: Harness>(h: &H, run: &mut Run) {
    let before = h.placements();
    h.lose_next_placement();
    run.market("the next placement's answer is lost");
    let sent = market(h, run, "n3", OrderSide::Buy, "0.1", EntryOrReduce::Entry, ok).await;
    run.expect(sent.is_err(), "the lost answer is an error");
    run.expect(
        !matches!(sent, Err(ExecError::OrderRejected(_) | ExecError::RulesRejected(_))),
        "a lost answer is never a rejection",
    );
    run.expect(h.placements() == before + 1, "the placement reached the venue");
    h.conceal("n3");
    run.market("the order is not visible to lookups");
}

/// N3a: a lost answer while the order is not visible is `Unknown`, never
/// `Absent`.
pub async fn n3a<H: Harness>(h: &H) -> Outcome {
    let mut run = Run::new(N3a, Coverage::Verified);
    lose_one(h, &mut run).await;
    let found = lookup(h, &mut run, "n3").await;
    run.expect(matches!(found, Some(OrderLookup::Unknown)), "the hidden lost order is Unknown");
    run.finish()
}

// Balance accounts.

fn live_stop(book: &SpotBook, id: &str) -> Option<(BookStatus, Decimal)> {
    let id: u64 = id.parse().ok()?;
    let order = book.all_orders().into_iter().find(|o| o.id == id)?;
    Some((order.status, order.executed_qty))
}

fn order_id(stop: &StopRef) -> u64 {
    stop.exchange_order_id.parse().expect("a book order id")
}

/// The seeded holding a balance scenario starts from.
fn seeded<H: BalanceHarness>(h: &H, run: &mut Run) {
    let base = h.book().balance(h.symbol().base());
    run.state(format!("{} free {} locked {}", h.symbol().base(), base.free, base.locked));
    run.expect(base.free >= d("1") && base.locked.is_zero(), "seeded with a free holding of 1");
    let price = h.book().price(&h.symbol().as_pair());
    run.expect(price == Some(d("100")), "seeded at a price of 100");
}

/// N3b: once the accepted order is visible, the lookup reports its real
/// lifecycle.
pub async fn n3b<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(N3b, Coverage::Verified);
    seeded(h, &mut run);
    lose_one(h, &mut run).await;
    h.reveal("n3");
    run.market("the order becomes visible");
    let found = lookup(h, &mut run, "n3").await;
    run.expect(
        matches!(found, Some(OrderLookup::Terminal { filled_qty, .. }) if filled_qty == d("0.1")),
        "the visible order is Terminal with its execution",
    );
    run.finish()
}

/// R2: a delayed cancel gives `Unknown { new: None }`, and nothing is placed.
pub async fn r2<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(R2, Coverage::Verified);
    seeded(h, &mut run);
    let Some(old) = stop(h, &mut run, "r2-old", "0.5", "90").await else {
        return run.finish();
    };
    let placed = h.placements();
    h.book().delay_cancels(true);
    run.market("cancels are delayed");
    let outcome = replace(h, &mut run, &old, "r2-new", "0.5", "92").await;
    run.expect(
        matches!(outcome, Ok(ReplaceOutcome::Unknown { new: None })),
        "Unknown { new: None }",
    );
    run.state(format!("placements {placed} -> {}, open {:?}", h.placements(), h.open_stops()));
    run.expect(h.placements() == placed, "no placement reached the venue");
    run.expect(h.open_stops() == [old.exchange_order_id.clone()], "the old stop is still live");
    // While its cancel is queued the old stop executes; the cancel, applied
    // later, leaves it Filled (terminal precedence).
    h.book().set_price(&h.symbol().as_pair(), d("85"));
    let crossed = h.book().cross(order_id(&old), d("0.5"));
    run.market("the old stop executes 0.5 at 85 while its cancel is queued");
    run.expect(crossed.is_ok(), "the cross applied");
    h.book().apply_delayed_cancels();
    run.market("the venue applies the queued cancel");
    let state = live_stop(h.book(), &old.exchange_order_id);
    run.state(format!("old stop {state:?}"));
    run.expect(state == Some((BookStatus::Filled, d("0.5"))), "the old stop stays Filled");
    run.expect(h.book().sold(&h.symbol().as_pair()) == d("0.5"), "sold exactly what executed");
    run.finish()
}

/// A successor the venue refused after the old stop's definite cancel. The
/// helper answers `OldCancelledNewRejected` only with the successor's
/// attempts all rejected; a journaled venue reports a live intent's
/// attempts as unresolved (ADR-0056 step 7, rule 4), so inside the owner's
/// operation its answer is the conservative `Unknown { new: None }`. Either
/// way the successor never rests, and once its intent is settled (the
/// harness settles it after the call) its id is `Absent`.
async fn refused_successor<H: Harness>(
    h: &H,
    run: &mut Run,
    outcome: &Result<ReplaceOutcome, ExecError>,
    id: &str,
) {
    let conservative = match outcome {
        Ok(ReplaceOutcome::OldCancelledNewRejected { old_filled, .. }) => old_filled.is_zero(),
        Ok(ReplaceOutcome::Unknown { new: None }) => true,
        _ => false,
    };
    run.expect(
        conservative,
        "OldCancelledNewRejected, or Unknown { new: None } while its intent is live",
    );
    run.state(format!("open {:?}", h.open_stops()));
    run.expect(h.open_stops().is_empty(), "the successor never rests");
    let finality = h.venue().capabilities().order_finality;
    let settled = lookup(h, run, id).await;
    run.expect(
        matches!(settled, Some(OrderLookup::Absent { finality: f }) if f == finality),
        "once settled, the refused successor's id is Absent",
    );
}

/// R3: after a definite cancel, a refused successor never rests and its id
/// is `Absent` once settled ([`refused_successor`]); a live matching
/// successor is `Replaced`.
pub async fn r3<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(R3, Coverage::Verified);
    seeded(h, &mut run);
    if let Some(old) = stop(h, &mut run, "r3-a", "0.5", "90").await {
        h.reject_next_placement();
        run.market("the venue refuses the next placement");
        let outcome = replace(h, &mut run, &old, "r3-a2", "0.5", "92").await;
        refused_successor(h, &mut run, &outcome, "r3-a2").await;
        let old_state = live_stop(h.book(), &old.exchange_order_id).map(|s| s.0);
        run.expect(old_state == Some(BookStatus::Canceled), "the old stop was cancelled first");
    }
    if let Some(old) = stop(h, &mut run, "r3-b", "0.5", "90").await {
        let outcome = replace(h, &mut run, &old, "r3-b2", "0.5", "92").await;
        match outcome {
            Ok(ReplaceOutcome::Replaced { new, old_filled }) => {
                run.expect(old_filled.is_zero(), "nothing of the old stop executed");
                run.expect(
                    h.open_stops() == [new.stop.exchange_order_id.clone()],
                    "only the successor rests",
                );
                let record = new.acceptance.record().cloned();
                run.expect(
                    record.is_some_and(|r| {
                        r.phase == OrderPhase::Live && r.stop_price == Some(d("92"))
                    }),
                    "the successor's record is live at its trigger",
                );
            },
            other => run.expect(false, format!("Replaced, not {other:?}")),
        }
    }
    run.finish()
}

/// A1a on a balance account: the price crossed the old trigger and nothing
/// executed: a definite cancel, then a successor below the market.
pub async fn a1a_balance<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(A1a, Coverage::Verified);
    seeded(h, &mut run);
    let Some(old) = stop(h, &mut run, "a1-old", "0.5", "90").await else {
        return run.finish();
    };
    h.book().set_price(&h.symbol().as_pair(), d("85"));
    run.market("the price falls to 85, below the old trigger, with nothing executed");
    run.expect(
        live_stop(h.book(), &old.exchange_order_id) == Some((BookStatus::New, Decimal::ZERO)),
        "crossed, not executed",
    );
    let outcome = replace(h, &mut run, &old, "a1-new", "0.5", "80").await;
    run.expect(
        matches!(outcome, Ok(ReplaceOutcome::Replaced { old_filled, .. }) if old_filled.is_zero()),
        "Replaced { old_filled: 0 }",
    );
    run.expect(
        live_stop(h.book(), &old.exchange_order_id).map(|s| s.0) == Some(BookStatus::Canceled),
        "the old stop is cancelled",
    );
    run.finish()
}

/// A1b: a successor whose trigger is at the price is refused (`-2010`,
/// "Order would trigger immediately.") and never rests
/// ([`refused_successor`]).
pub async fn a1b<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(A1b, Coverage::Verified);
    seeded(h, &mut run);
    let Some(old) = stop(h, &mut run, "a1b-old", "0.5", "90").await else {
        return run.finish();
    };
    let outcome = replace(h, &mut run, &old, "a1b-new", "0.5", "100").await;
    let refusal = h.book().events().into_iter().any(|event| {
        matches!(event, BookEvent::Refused { client_id, code: -2010 } if client_id == "a1b-new")
    });
    run.expect(refusal, "the venue refused the successor with -2010");
    refused_successor(h, &mut run, &outcome, "a1b-new").await;
    let refused = h.book().order(&h.symbol().as_pair(), BookRef::Client("a1b-new"));
    run.expect(refused.is_err(), "the book holds no successor");
    run.finish()
}

/// An old stop of 0.5 at 90 that executed `e` at 85 before the replacement.
async fn executed_old<H: BalanceHarness>(
    h: &H,
    run: &mut Run,
    id: &str,
    e: &str,
) -> Option<StopRef> {
    let old = stop(h, run, id, "0.5", "90").await?;
    h.book().set_price(&h.symbol().as_pair(), d("85"));
    let crossed = h.book().cross(order_id(&old), d(e));
    run.market(format!("the market crosses the old stop and executes {e}"));
    run.expect(crossed.is_ok(), "the cross applied");
    Some(old)
}

/// A2 and A3 on a balance account: the old stop executed `e` (partly or
/// whole): `OldExecuted { e }`, and nothing is placed.
async fn old_executed<H: BalanceHarness>(h: &H, property: super::Property, e: &str) -> Outcome {
    let mut run = Run::new(property, Coverage::Verified);
    seeded(h, &mut run);
    let Some(old) = executed_old(h, &mut run, &format!("{property:?}-old"), e).await else {
        return run.finish();
    };
    let placed = h.placements();
    let outcome = replace(h, &mut run, &old, &format!("{property:?}-new"), "0.3", "80").await;
    run.expect(
        matches!(outcome, Ok(ReplaceOutcome::OldExecuted { old_filled }) if old_filled == d(e)),
        format!("OldExecuted {{ {e} }}"),
    );
    run.state(format!(
        "placements {placed} -> {}, sold {}",
        h.placements(),
        h.book().sold(&h.symbol().as_pair())
    ));
    run.expect(h.placements() == placed, "no placement reached the venue");
    run.expect(h.book().sold(&h.symbol().as_pair()) == d(e), "sold exactly the old execution");
    run.finish()
}

/// A2: partial old fills.
pub async fn a2_balance<H: BalanceHarness>(h: &H) -> Outcome {
    old_executed(h, A2, "0.2").await
}

/// A3: full old fills.
pub async fn a3_balance<H: BalanceHarness>(h: &H) -> Outcome {
    old_executed(h, A3, "0.5").await
}

/// A4's valid sequence on a balance account: the old stop of 1 is
/// cancelled (unlocking 1), a flatten of 0.4 executes as the successor
/// reaches the venue, and the stale successor of 1 is refused.
async fn stale_successor<H: BalanceHarness>(h: &H, run: &mut Run) {
    let Some(old) = stop(h, run, "a4-old", "1", "90").await else {
        return;
    };
    let pair = h.symbol().as_pair();
    let flattened = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&flattened);
    h.on_next_placement(Box::new(move |book: &SpotBook| {
        let sold = book.place_market(&pair, BookSide::Sell, d("0.4"), "a4-flatten");
        *seen.lock().unwrap() = Some(sold.is_ok());
    }));
    run.market("a flatten of 0.4 executes as the successor reaches the venue");
    let outcome = replace(h, run, &old, "a4-new", "1", "92").await;
    run.expect(*flattened.lock().unwrap() == Some(true), "the flatten executed");
    refused_successor(h, run, &outcome, "a4-new").await;
    let pair = h.symbol().as_pair();
    let base = h.book().balance(h.symbol().base());
    run.state(format!(
        "sold {}, free {}, locked {}, open {:?}",
        h.book().sold(&pair),
        base.free,
        base.locked,
        h.open_stops()
    ));
    run.expect(h.book().sold(&pair) == d("0.4"), "sold the flatten exactly");
    run.expect(h.open_stops().is_empty(), "nothing rests");
    run.expect(
        base.locked.is_zero() && base.free == d("0.6"),
        "the old lock released once, the flatten taken",
    );
}

/// A4: partial flatten, the valid sequence.
pub async fn a4_balance<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(A4, Coverage::Verified);
    seeded(h, &mut run);
    stale_successor(h, &mut run).await;
    run.finish()
}

/// A5b: across R2, A2, A3 and A4, nothing is placed after an indefinite
/// cancel or an executed old stop, and the stale successor is refused, the
/// flatten sold exactly. Each case on a fresh harness from `make`.
pub async fn a5b<H: BalanceHarness>(make: impl Fn() -> H) -> Outcome {
    let mut run = Run::new(A5b, Coverage::Verified);
    let h = make();
    seeded(&h, &mut run);
    if let Some(old) = stop(&h, &mut run, "a5-r2", "0.5", "90").await {
        let placed = h.placements();
        h.book().delay_cancels(true);
        let outcome = replace(&h, &mut run, &old, "a5-r2-new", "0.5", "92").await;
        run.expect(matches!(outcome, Ok(ReplaceOutcome::Unknown { new: None })), "R2: Unknown");
        run.expect(h.placements() == placed, "R2: nothing placed after the indefinite cancel");
    }
    for (case, e) in [("a5-a2", "0.2"), ("a5-a3", "0.5")] {
        let h = make();
        if let Some(old) = executed_old(&h, &mut run, case, e).await {
            let placed = h.placements();
            let outcome = replace(&h, &mut run, &old, &format!("{case}-new"), "0.3", "80").await;
            run.expect(
                matches!(outcome, Ok(ReplaceOutcome::OldExecuted { .. })),
                format!("{case}: OldExecuted"),
            );
            run.expect(
                h.placements() == placed,
                format!("{case}: nothing placed after the execution"),
            );
        }
    }
    let h = make();
    stale_successor(&h, &mut run).await;
    run.finish()
}

/// B1: one attempt per id: a second call without a fresh grant is refused
/// and never reaches the venue.
pub async fn b1<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(B1, Coverage::Verified);
    seeded(h, &mut run);
    let first = market(h, &mut run, "b1", OrderSide::Buy, "0.1", EntryOrReduce::Entry, ok).await;
    run.expect(first.is_ok(), "the granted send goes out");
    let placed = h.placements();
    run.sent("place_market_order b1 again, with no fresh grant");
    let again = h
        .venue()
        .place_market_order(&h.symbol(), OrderSide::Buy, qty("0.1"), "b1", EntryOrReduce::Entry)
        .await;
    let again = run.answered("place_market_order", again);
    run.expect(matches!(again, Err(ExecError::SendRefused(_))), "SendRefused");
    run.state(format!("placements {placed} -> {}", h.placements()));
    run.expect(h.placements() == placed, "the second call never reached the venue");
    run.finish()
}

/// B2: cancel typing: `Cancelled` only terminal and consistent; a terminal
/// or unknown order `NotFound`; a delayed cancel `Unknown`.
pub async fn b2<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(B2, Coverage::Verified);
    seeded(h, &mut run);
    let symbol = h.symbol();
    let Some(open) = stop(h, &mut run, "b2-a", "0.5", "90").await else {
        return run.finish();
    };
    let inventory = h.venue().inventory().expect("a balance account's inventory");
    let id = open.exchange_order_id.clone();
    run.sent(format!("cancel_order_once {id}"));
    let cancelled =
        run.answered("cancel_order_once", inventory.cancel_order_once(&symbol, &id).await);
    run.expect(
        matches!(&cancelled, Ok(CancelAnswer::Cancelled(r)) if r.phase == OrderPhase::Terminal
            && r.quantities_consistent() && r.exchange_order_id == id),
        "Cancelled with a terminal, consistent record of this order",
    );
    for (what, target) in [("terminal", id.clone()), ("unknown", "999999".to_string())] {
        run.sent(format!("cancel_order_once {target} ({what})"));
        let answer =
            run.answered("cancel_order_once", inventory.cancel_order_once(&symbol, &target).await);
        run.expect(
            matches!(answer, Ok(CancelAnswer::NotFound { .. })),
            format!("{what}: NotFound"),
        );
    }
    if let Some(delayed) = stop(h, &mut run, "b2-b", "0.5", "90").await {
        h.book().delay_cancels(true);
        run.market("cancels are delayed");
        let id = delayed.exchange_order_id;
        run.sent(format!("cancel_order_once {id} (delayed)"));
        let answer =
            run.answered("cancel_order_once", inventory.cancel_order_once(&symbol, &id).await);
        run.expect(matches!(answer, Ok(CancelAnswer::Unknown { .. })), "delayed: Unknown");
    }
    run.finish()
}

/// B3: an expiry with nothing executed is never a rejection: hidden, its
/// not-found stays `Unknown`; visible, it is `Terminal { 0 }`.
pub async fn b3<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(B3, Coverage::Verified);
    seeded(h, &mut run);
    h.book().script_next(Script::ExpireUnfilled);
    run.market("the next MARKET order expires with nothing executed");
    let accepted = |r: &Result<OrderResult, ExecError>| {
        r.is_ok() || matches!(r, Err(ExecError::ExpiredUnfilled(_)))
    };
    let sent =
        market(h, &mut run, "b3", OrderSide::Buy, "0.1", EntryOrReduce::Entry, accepted).await;
    run.expect(
        matches!(&sent, Err(ExecError::ExpiredUnfilled(r)) if r.executed_qty.is_zero()),
        "ExpiredUnfilled with nothing executed",
    );
    h.conceal("b3");
    let hidden = lookup(h, &mut run, "b3").await;
    run.expect(matches!(hidden, Some(OrderLookup::Unknown)), "hidden: Unknown, never Absent");
    h.reveal("b3");
    let visible = lookup(h, &mut run, "b3").await;
    run.expect(
        matches!(visible, Some(OrderLookup::Terminal { filled_qty, .. }) if filled_qty.is_zero()),
        "visible: Terminal { 0 }",
    );
    run.finish()
}

/// B4: the fills rule: a terminal order whose trades are withheld gives no
/// fills, its terminality and execution kept.
pub async fn b4<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(B4, Coverage::Verified);
    seeded(h, &mut run);
    let Some(old) = executed_old(h, &mut run, "b4", "0.5").await else {
        return run.finish();
    };
    h.book().withhold_trades(order_id(&old));
    run.market("the venue withholds the order's trades");
    let symbol = h.symbol();
    run.sent("cancel_protection b4");
    let outcome =
        run.answered("cancel_protection", h.venue().cancel_protection(&symbol, &old).await);
    run.expect(
        matches!(outcome, Ok(StopCancelOutcome::AlreadyGone {
            final_state: OrderLookup::Terminal { filled_qty, fills: None, .. }
        }) if filled_qty == d("0.5")),
        "AlreadyGone Terminal with the execution and no fills",
    );
    h.book().release_trades(order_id(&old));
    run.market("the trades become visible");
    let fill = run.answered("protection_fill", h.venue().protection_fill(&symbol, &old).await);
    run.expect(
        matches!(fill, Ok(Some(ref r)) if r.filled_quantity.as_decimal() == d("0.5")),
        "with the trades visible the fills are established",
    );
    // The order's visibility and its trades' are independent: hidden, the
    // order's trades still read by page.
    h.book().hide_order(order_id(&old));
    run.market("the venue hides the order, its trades visible");
    let inventory = h.venue().inventory().expect("a balance account's inventory");
    let page = inventory.order_trades_page(&symbol, &old.exchange_order_id, 0).await;
    let page = run.answered("order_trades_page", page);
    run.expect(
        matches!(page, Ok(ref page) if page.trades.len() == 1),
        "a hidden order's trades still read",
    );
    run.finish()
}

/// The account observation's holdings, as (asset, free, locked).
async fn holdings<H: Harness>(h: &H) -> Vec<(String, Decimal, Decimal)> {
    let inventory = h.venue().inventory().expect("a balance account's inventory");
    match inventory.account_observation().await {
        Ok(observation) => observation
            .holdings
            .into_iter()
            .map(|(a, held)| (a, held.free, held.locked))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// One B5 step: the observation against the base's and the quote's
/// hand-computed (free, locked).
async fn balances_are<H: Harness>(
    h: &H,
    run: &mut Run,
    when: &str,
    base: (&str, &str),
    quote: (&str, &str),
) {
    let observed = holdings(h).await;
    run.state(format!("{when}: {observed:?}"));
    let symbol = h.symbol();
    let expected = vec![
        (symbol.base().to_string(), d(base.0), d(base.1)),
        (symbol.quote().to_string(), d(quote.0), d(quote.1)),
    ];
    run.expect(observed == expected, format!("{when}: {expected:?}"));
}

/// B5: balances through placement, cross, cancel, expiry and a delayed
/// cancel, as the account observation reports them, against hand-computed
/// values (fee 0.001; a BUY pays it in base, a SELL in quote).
pub async fn b5<H: BalanceHarness>(h: &H) -> Outcome {
    let mut run = Run::new(B5, Coverage::Verified);
    seeded(h, &mut run);
    let symbol = h.symbol();
    let bought = market(h, &mut run, "b5-buy", OrderSide::Buy, "1", EntryOrReduce::Entry, ok).await;
    run.expect(bought.is_ok(), "the BUY fills");
    // 1 + 1 × 0.999; 1000 − 1 × 100.
    balances_are(h, &mut run, "after BUY 1 at 100", ("1.999", "0"), ("900", "0")).await;
    let Some(first) = stop(h, &mut run, "b5-s1", "1.5", "90").await else {
        return run.finish();
    };
    balances_are(h, &mut run, "after a stop of 1.5", ("0.499", "1.5"), ("900", "0")).await;
    h.book().set_price(&symbol.as_pair(), d("85"));
    let crossed = h.book().cross(order_id(&first), d("0.5"));
    run.market("the market executes 0.5 of the stop at 85");
    run.expect(crossed.is_ok(), "the cross applied");
    // 900 + 0.5 × 85 × 0.999.
    balances_are(h, &mut run, "after 0.5 at 85", ("0.499", "1"), ("942.4575", "0")).await;
    run.sent("cancel_protection b5-s1");
    let cancelled =
        run.answered("cancel_protection", h.venue().cancel_protection(&symbol, &first).await);
    run.expect(
        matches!(cancelled, Ok(StopCancelOutcome::Cancelled { .. })),
        "the rest is cancelled",
    );
    balances_are(h, &mut run, "after the cancel", ("1.499", "0"), ("942.4575", "0")).await;
    if let Some(expiring) = stop(h, &mut run, "b5-s2", "0.4", "80").await {
        h.book().expire(order_id(&expiring));
        run.market("the venue expires the stop of 0.4");
        balances_are(h, &mut run, "after the expiry", ("1.499", "0"), ("942.4575", "0")).await;
    }
    if let Some(delayed) = stop(h, &mut run, "b5-s3", "0.3", "80").await {
        h.book().delay_cancels(true);
        let inventory = h.venue().inventory().expect("a balance account's inventory");
        run.sent("cancel_order_once b5-s3 (delayed)");
        let id = delayed.exchange_order_id;
        let answer =
            run.answered("cancel_order_once", inventory.cancel_order_once(&symbol, &id).await);
        run.expect(
            matches!(answer, Ok(CancelAnswer::Unknown { .. })),
            "the delayed cancel is Unknown",
        );
        balances_are(h, &mut run, "with the cancel queued", ("1.199", "0.3"), ("942.4575", "0"))
            .await;
        h.book().apply_delayed_cancels();
        run.market("the venue applies the delayed cancel");
        balances_are(h, &mut run, "after it applies", ("1.499", "0"), ("942.4575", "0")).await;
    }
    // A queued cancel whose order was cancelled meanwhile releases nothing
    // more when it applies, with another stop's lock there to take from.
    let held = stop(h, &mut run, "b5-held", "0.5", "80").await;
    let stale = stop(h, &mut run, "b5-s4", "0.2", "80").await;
    if let (Some(_), Some(twice)) = (held, stale) {
        let inventory = h.venue().inventory().expect("a balance account's inventory");
        let id = twice.exchange_order_id;
        run.sent("cancel_order_once b5-s4 (delayed, then immediate)");
        let queued = inventory.cancel_order_once(&symbol, &id).await;
        run.expect(
            matches!(queued, Ok(CancelAnswer::Unknown { .. })),
            "the first cancel is queued",
        );
        h.book().delay_cancels(false);
        let now =
            run.answered("cancel_order_once", inventory.cancel_order_once(&symbol, &id).await);
        run.expect(
            matches!(now, Ok(CancelAnswer::Cancelled(_))),
            "the second cancel applies at once",
        );
        h.book().apply_delayed_cancels();
        run.market("the venue applies the stale queued cancel");
        balances_are(h, &mut run, "after the stale cancel", ("0.999", "0.5"), ("942.4575", "0"))
            .await;
    }
    run.finish()
}

// Position accounts.

/// R1: under `PlaceThenCancel` the old stop is live when the successor is
/// accepted; a rejected successor leaves it live (`OldKeptNewRejected`).
pub async fn r1<H: PositionHarness>(h: &H) -> Outcome {
    let mut run = Run::new(R1, Coverage::Verified);
    run.state(format!("position {}", h.position()));
    run.expect(h.position() >= d("1"), "a position of 1 to protect");
    let Some(old) = stop(h, &mut run, "r1-old", "1", "90").await else {
        return run.finish();
    };
    let seen = Arc::new(Mutex::new(None));
    let saw = Arc::clone(&seen);
    h.on_next_stop_accepted(Box::new(move |open| *saw.lock().unwrap() = Some(open)));
    let outcome = replace(h, &mut run, &old, "r1-new", "1", "92").await;
    let open_then = seen.lock().unwrap().clone();
    run.state(format!("live stops as the successor was accepted: {open_then:?}"));
    run.expect(
        open_then.is_some_and(|open| open.contains(&old.exchange_order_id)),
        "the old stop was live when the successor was accepted",
    );
    let Ok(ReplaceOutcome::Replaced { new, old_filled }) = outcome else {
        run.expect(false, format!("Replaced, not {outcome:?}"));
        return run.finish();
    };
    run.expect(old_filled.is_zero(), "nothing of the old stop executed");
    run.expect(
        h.open_stops() == [new.stop.exchange_order_id.clone()],
        "only the successor is live",
    );
    h.reject_next_placement();
    run.market("the venue refuses the next placement");
    let refused = replace(h, &mut run, &new.stop, "r1-rej", "1", "93").await;
    run.expect(
        matches!(refused, Ok(ReplaceOutcome::OldKeptNewRejected { .. })),
        "OldKeptNewRejected",
    );
    run.expect(
        h.open_stops() == [new.stop.exchange_order_id.clone()],
        "the kept stop is still live",
    );
    run.finish()
}

/// A1a on a position account: zero old fills: `Replaced { old_filled: 0 }`.
pub async fn a1a_position<H: PositionHarness>(h: &H) -> Outcome {
    let mut run = Run::new(A1a, Coverage::Verified);
    run.expect(h.position() >= d("1"), "a position of 1 to protect");
    let Some(old) = stop(h, &mut run, "a1-old", "1", "90").await else {
        return run.finish();
    };
    let outcome = replace(h, &mut run, &old, "a1-new", "1", "92").await;
    run.expect(
        matches!(outcome, Ok(ReplaceOutcome::Replaced { old_filled, .. }) if old_filled.is_zero()),
        "Replaced { old_filled: 0 }",
    );
    run.finish()
}

/// A2 and A3 on a position account, `Scripted`: the old stop's cancel
/// answers with the execution `e`; the successor is placed first, and the
/// helper reports `Replaced { old_filled: e }`. Nothing about the venue's
/// own execution is claimed.
async fn scripted_execution<H: PositionHarness + ScriptsStopCancel>(
    h: &H,
    property: super::Property,
    answer: StopCancelOutcome,
    e: &str,
) -> Outcome {
    let mut run = Run::new(property, Coverage::Scripted);
    run.expect(h.position() >= d("1"), "a position of 1 to protect");
    let Some(old) = stop(h, &mut run, &format!("{property:?}-old"), "1", "90").await else {
        return run.finish();
    };
    h.script_next_stop_cancel(answer.clone());
    run.market(format!("the old stop's cancel will answer {answer:?}"));
    let seen = Arc::new(Mutex::new(None));
    let saw = Arc::clone(&seen);
    h.on_next_stop_accepted(Box::new(move |open| *saw.lock().unwrap() = Some(open)));
    let outcome = replace(h, &mut run, &old, &format!("{property:?}-new"), "1", "92").await;
    let open_then = seen.lock().unwrap().clone();
    run.expect(
        open_then.is_some_and(|open| open.contains(&old.exchange_order_id)),
        "the successor was placed before the old stop's cancel",
    );
    run.expect(
        matches!(outcome, Ok(ReplaceOutcome::Replaced { old_filled, .. }) if old_filled == d(e)),
        format!("Replaced {{ old_filled: {e} }}"),
    );
    run.finish()
}

/// A2, scripted: the cancel answers a partial execution.
pub async fn a2_scripted<H: PositionHarness + ScriptsStopCancel>(h: &H) -> Outcome {
    let answer = StopCancelOutcome::Cancelled {
        filled_before_cancel: d("0.3"),
        fills: None,
    };
    scripted_execution(h, A2, answer, "0.3").await
}

/// A3, scripted: the cancel answers the stop gone after a full execution.
pub async fn a3_scripted<H: PositionHarness + ScriptsStopCancel>(h: &H) -> Outcome {
    let final_state = OrderLookup::Terminal {
        filled_qty: d("1"),
        cancelled_qty: Decimal::ZERO,
        fills: None,
    };
    scripted_execution(h, A3, StopCancelOutcome::AlreadyGone { final_state }, "1").await
}

/// A4 on a position account, `Characterized`: after a flatten of 0.4 the
/// venue accepts a stale successor of 1 against a position of 0.6: pinned
/// as `Replaced` with the stale quantity. Its execution bound is A5p's,
/// pending.
pub async fn a4_characterized<H: PositionHarness>(h: &H) -> Outcome {
    let mut run = Run::new(A4, Coverage::Characterized);
    run.expect(h.position() == d("1"), "a position of 1");
    let Some(old) = stop(h, &mut run, "a4-old", "1", "90").await else {
        return run.finish();
    };
    let flat =
        market(h, &mut run, "a4-flat", OrderSide::Sell, "0.4", EntryOrReduce::Reduce, ok).await;
    run.expect(flat.is_ok(), "the flatten executes");
    run.state(format!("position {}", h.position()));
    run.expect(h.position() == d("0.6"), "the flatten reduced the position to 0.6");
    let outcome = replace(h, &mut run, &old, "a4-new", "1", "92").await;
    run.expect(
        matches!(&outcome, Ok(ReplaceOutcome::Replaced { new, old_filled })
            if old_filled.is_zero() && new.acceptance.exchange_order_id == new.stop.exchange_order_id),
        "pinned: Replaced { old_filled: 0 }",
    );
    run.expect(h.open_stops().len() == 1, "the stale successor of 1 is live");
    run.finish()
}

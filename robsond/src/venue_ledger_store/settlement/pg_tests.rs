//! 8l-2 proofs on PostgreSQL (PLAN-r4 §8): each barrier condition, a late
//! historical fill, the ±2 s edge, generated ledgers against an independent
//! oracle, and the statement's one round trip and bound at PLAN-r4 §7.2's
//! cardinality. Fixture rows are written directly: their writers are other
//! slices'.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use anyhow::{ensure, Result};
use proptest::{
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngSeed, TestRunner},
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use rust_decimal::Decimal;
use sqlx::{postgres::PgPoolOptions, PgPool};

use super::*;

const SYMBOL: &str = "BTCUSDT"; // A labelled fixture pair, never a policy restriction.
const ASSETS: &[CommissionAssets<'static>] = &[CommissionAssets {
    symbol: SYMBOL,
    base: "BTC",
    quote: "USDT",
}];
static IDS: AtomicU64 = AtomicU64::new(1);

fn at(text: &str) -> DateTime<Utc> {
    text.parse().expect("RFC 3339 fixture time")
}
fn month(y: i32, m: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, 1).expect("fixture month")
}
fn venue() -> VenueId {
    VenueId::new(VenueId::BINANCE_SPOT)
}

/// One order of a leg under `intent`: `filled` of `executed` booked, in
/// `asset` when set.
struct Order<'a> {
    intent: Uuid,
    buy: bool,
    phase: &'a str,
    intent_at: Option<DateTime<Utc>>,
    live: DateTime<Utc>,
    terminal: Option<DateTime<Utc>>,
    executed: Decimal,
    filled: Decimal,
    asset: Option<&'a str>,
}

impl Order<'static> {
    /// A sell sent and first seen at `born`, seen terminal at `seen` if so.
    fn sell(
        phase: &'static str,
        born: DateTime<Utc>,
        seen: Option<DateTime<Utc>>,
        executed: Decimal,
        filled: Decimal,
    ) -> Self {
        let (intent_at, live, terminal, asset) = (Some(born), born, seen, None);
        Self {
            intent: Uuid::now_v7(),
            buy: false,
            phase,
            intent_at,
            live,
            terminal,
            executed,
            filled,
            asset,
        }
    }
}

/// One tenant's balance-account venue.
struct Book {
    tenant: Uuid,
    scope: String,
}

impl Book {
    async fn new(pool: &PgPool) -> Result<Self> {
        let tenant = Uuid::now_v7();
        sqlx::query("INSERT INTO accounting_state (tenant_id,venue_id,budget_model) VALUES ($1,'binance:spot','net_from_start_non_expanding_v1')")
            .bind(tenant).execute(pool).await?;
        Ok(Self {
            tenant,
            scope: format!("barrier-{tenant}"),
        })
    }

    /// A leg with Q = 1, C = 100, B = 0.001 and X = 0.9 when `executable`.
    async fn leg(
        &self,
        pool: &PgPool,
        created: DateTime<Utc>,
        closed: Option<DateTime<Utc>>,
        executable: bool,
        fee_final: bool,
        consumed: Decimal,
    ) -> Result<Uuid> {
        let (position, admission) = (Uuid::now_v7(), Uuid::now_v7());
        let decided = month(created.year(), created.month());
        sqlx::query("INSERT INTO leg_routes VALUES ($1,$2,'binance:spot',$3,$4)")
            .bind(position)
            .bind(self.tenant)
            .bind(&self.scope)
            .bind(created)
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO balance_leg_admissions (entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,reference_price,remainder_reserve,admission_charge,initial_trigger,created_at)
            VALUES ($1,$2,$3,'binance:spot',$4,$5,$6,10,10,2,5,0.00001,'base','fixture rules',$7,'fixture-hash',1,1,100,0.1,1,90,$7)")
            .bind(admission).bind(position).bind(self.tenant).bind(&self.scope).bind(SYMBOL).bind(decided).bind(created)
            .execute(pool).await?;
        let quantity = |value: &str| executable.then(|| value.parse::<Decimal>().expect("fixture"));
        sqlx::query("INSERT INTO balance_legs (position_id,tenant_id,venue_id,account_scope,symbol,admission_intent_id,decided_month,target_trigger,gross_qty,acquisition_cost,base_commission,base_commission_final,executable_qty,consumed_cum,created_at,closed_at)
            VALUES ($1,$2,'binance:spot',$3,$4,$5,$6,90,$7,$8,$9,$10,$11,$12,$13,$14)")
            .bind(position).bind(self.tenant).bind(&self.scope).bind(SYMBOL).bind(admission).bind(decided)
            .bind(quantity("1")).bind(quantity("100")).bind(quantity("0.001")).bind(fee_final && executable)
            .bind(quantity("0.9")).bind(consumed).bind(created).bind(closed)
            .execute(pool).await?;
        Ok(position)
    }

    /// A settled open leg: X set, B final, its entry terminal and booked.
    async fn settled_leg(&self, pool: &PgPool, created: DateTime<Utc>) -> Result<Uuid> {
        let leg = self.leg(pool, created, None, true, true, Decimal::ZERO).await?;
        let entry = Order::sell("terminal", created, Some(created), Decimal::ONE, Decimal::ONE);
        self.order(pool, leg, &Order { buy: true, ..entry }).await?;
        Ok(leg)
    }

    /// The order, its intent when `intent_at` is set, and one fill of
    /// `filled` in `asset` or else the expected asset. Returns its id.
    async fn order(&self, pool: &PgPool, leg: Uuid, order: &Order<'_>) -> Result<String> {
        let (id, intent) = (IDS.fetch_add(1, Ordering::Relaxed).to_string(), order.intent);
        let (kind, side, journal) = if order.buy {
            ("entry", "BUY", "entry")
        } else {
            ("sell", "SELL", "exit")
        };
        if let Some(created) = order.intent_at {
            self.intent(pool, Some(leg), journal, Some("success"), false, created, Some(intent))
                .await?;
        }
        sqlx::query("INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,status,phase,executed_cum,fills_qty,last_checked_at,live_seen_at,terminal_seen_at)
            VALUES ('binance:spot',$1,$2,$3,$4,$3,$3,'leg',$5,$6,$7,$8,'MARKET',2,'FIXTURE',$9,$10,$11,$12,$12,$13)")
            .bind(&self.scope).bind(SYMBOL).bind(&id).bind(self.tenant).bind(intent).bind(leg).bind(kind).bind(side)
            .bind(order.phase).bind(order.executed).bind(order.filled).bind(order.live).bind(order.terminal)
            .execute(pool).await?;
        if order.filled > Decimal::ZERO {
            let asset = order.asset.unwrap_or(if order.buy { "BTC" } else { "USDT" });
            self.fill(pool, &id, order.filled, asset, order.live).await?;
        }
        Ok(id)
    }

    async fn fill(
        &self,
        pool: &PgPool,
        order: &str,
        qty: Decimal,
        asset: &str,
        time: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO order_fills VALUES ('binance:spot',$1,$2,$3,$4,$5,100,$6,$6*100,0.001,$7,$8,$8)")
            .bind(&self.scope).bind(SYMBOL).bind(IDS.fetch_add(1, Ordering::Relaxed).to_string()).bind(order)
            .bind(self.tenant).bind(qty).bind(asset).bind(time)
            .execute(pool).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn intent(
        &self,
        pool: &PgPool,
        leg: Option<Uuid>,
        kind: &str,
        outcome: Option<&str>,
        not_sent: bool,
        created: DateTime<Utc>,
        id: Option<Uuid>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,venue_outcome,not_sent,created_at)
            VALUES ($1,$2,'binance:spot',$3,$1::text,$4,$5,'{}',$6,$7,$8)")
            .bind(id.unwrap_or_else(Uuid::now_v7)).bind(self.tenant).bind(&self.scope).bind(leg).bind(kind)
            .bind(outcome).bind(not_sent).bind(created)
            .execute(pool).await?;
        Ok(())
    }

    async fn read(
        &self,
        pool: &PgPool,
        month: NaiveDate,
        now: DateTime<Utc>,
    ) -> Result<SettlementBarrier> {
        let venue = venue();
        let query = SettlementQuery {
            tenant_id: self.tenant,
            venue: &venue,
            month,
            now,
            assets: ASSETS,
        };
        Ok(read_settlement(&mut *pool.acquire().await?, &query).await?)
    }
}

fn only(set: impl FnOnce(&mut UnsettledLegs)) -> UnsettledLegs {
    let mut legs = UnsettledLegs { total: 1, ..UnsettledLegs::default() };
    set(&mut legs);
    legs
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn barrier_holds_an_unknown_order(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    let (created, now, november) =
        (at("2026-11-02T10:00:00Z"), at("2026-11-20T12:00:00Z"), month(2026, 11));
    let leg = book.settled_leg(&pool, created).await?;
    let barrier = book.read(&pool, november, now).await?;
    ensure!(barrier.settled() && !barrier.month_invalid(), "{barrier:?}");

    // A stop placed and never answered: nothing executed, nothing to book.
    let unknown = Order::sell("unknown", created, None, Decimal::ZERO, Decimal::ZERO);
    let stop = book.order(&pool, leg, &unknown).await?;
    let barrier = book.read(&pool, november, now).await?;
    ensure!(
        barrier.unsettled == only(|l| l.unknown_order = 1) && barrier.life_meets_month,
        "{barrier:?}"
    );

    sqlx::query("UPDATE venue_orders SET phase='terminal',status='EXPIRED',terminal_seen_at=$1 WHERE exchange_order_id=$2 AND account_scope=$3")
        .bind(created).bind(&stop).bind(&book.scope).execute(&pool).await?;
    let barrier = book.read(&pool, november, now).await?;
    ensure!(barrier.settled() && !barrier.month_invalid(), "resolved: {barrier:?}");

    sqlx::query("UPDATE balance_legs SET recovery_state='entry_unknown' WHERE position_id=$1")
        .bind(leg)
        .execute(&pool)
        .await?;
    let barrier = book.read(&pool, november, now).await?;
    ensure!(
        barrier.unsettled == only(|l| l.unknown_order = 1) && barrier.month_invalid(),
        "{barrier:?}"
    );
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn barrier_holds_an_unexpected_commission_asset(pool: PgPool) -> Result<()> {
    let (t, now, november) =
        (at("2026-11-02T10:00:00Z"), at("2026-11-20T12:00:00Z"), month(2026, 11));
    let sold = |asset| Order {
        asset,
        ..Order::sell("terminal", t, Some(t), Decimal::ONE, Decimal::ONE)
    };
    for (asset, unexpected) in [(None, false), (Some("BTC"), true), (Some("BNB"), true)] {
        let book = Book::new(&pool).await?;
        let leg = book.settled_leg(&pool, t).await?;
        book.order(&pool, leg, &sold(asset)).await?;
        let barrier = book.read(&pool, november, now).await?;
        let expected = if unexpected {
            only(|l| l.unexpected_asset = 1)
        } else {
            UnsettledLegs::default()
        };
        ensure!(
            barrier.unsettled == expected && barrier.month_invalid() == unexpected,
            "{asset:?}: {barrier:?}"
        );
    }
    // A buy pays in base: quote on the entry holds the leg.
    let book = Book::new(&pool).await?;
    let leg = book.leg(&pool, t, None, true, true, Decimal::ZERO).await?;
    book.order(&pool, leg, &Order {
        buy: true,
        asset: Some("USDT"),
        ..sold(None)
    })
    .await?;
    ensure!(book.read(&pool, november, now).await?.unsettled == only(|l| l.unexpected_asset = 1));
    // A symbol without expected assets expects none: every fill is unexpected.
    let book = Book::new(&pool).await?;
    book.settled_leg(&pool, t).await?;
    let other = [CommissionAssets {
        symbol: "ETHUSDT",
        base: "ETH",
        quote: "USDT",
    }];
    let venue = venue();
    let query = SettlementQuery {
        tenant_id: book.tenant,
        venue: &venue,
        month: november,
        now,
        assets: &other,
    };
    let barrier = read_settlement(&mut *pool.acquire().await?, &query).await?;
    ensure!(barrier.unsettled == only(|l| l.unexpected_asset = 1), "{barrier:?}");
    Ok(())
}

/// Only an intent's own order, of its tenant, venue, scope and leg, is
/// evidence for it: another tenant's or another leg's row never discharges it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn foreign_order_evidence_never_discharges_an_intent(pool: PgPool) -> Result<()> {
    let (t, now, november) =
        (at("2026-11-02T10:00:00Z"), at("2026-11-20T12:00:00Z"), month(2026, 11));
    let (book, other) = (Book::new(&pool).await?, Book::new(&pool).await?);
    let (leg, sibling) = (book.settled_leg(&pool, t).await?, book.settled_leg(&pool, t).await?);
    let (foreign, local) = (Uuid::now_v7(), Uuid::now_v7());
    for intent in [foreign, local] {
        book.intent(&pool, Some(leg), "exit", Some("success"), false, t, Some(intent))
            .await?;
    }
    let held = |intent| Order {
        intent,
        intent_at: None,
        ..Order::sell("terminal", t, Some(t), Decimal::ZERO, Decimal::ZERO)
    };
    let elsewhere = other.settled_leg(&pool, t).await?;
    other.order(&pool, elsewhere, &held(foreign)).await?;
    book.order(&pool, sibling, &held(local)).await?;
    let barrier = book.read(&pool, november, now).await?;
    ensure!(
        barrier.unsettled == only(|l| l.unrowed_intent = 1) && barrier.life_meets_month,
        "{barrier:?}"
    );
    ensure!(
        other.read(&pool, november, now).await?.settled(),
        "the foreign row's own book is settled"
    );
    Ok(())
}

/// No accounting row, no answer: a wrong tenant or venue never reads settled.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_venue_without_its_accounting_row_has_no_answer(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    let (november, now) = (month(2026, 11), at("2026-11-20T12:00:00Z"));
    ensure!(book.read(&pool, november, now).await?.settled());
    let usdm = VenueId::new(VenueId::BINANCE_USDM);
    for (tenant, venue) in [(Uuid::now_v7(), venue()), (book.tenant, usdm)] {
        let query = SettlementQuery {
            tenant_id: tenant,
            venue: &venue,
            month: november,
            now,
            assets: ASSETS,
        };
        let answer = read_settlement(&mut *pool.acquire().await?, &query).await;
        ensure!(answer == Err(LedgerError::NoAccountingRow), "{answer:?}");
    }
    Ok(())
}

/// A sell's fill dated 31 October, booked late: October is invalid until it
/// is, November never was; an order unknown across both holds both.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn late_historical_fill_invalidates_its_own_month_until_settled(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    let now = at("2026-11-20T12:00:00Z");
    let leg = book.settled_leg(&pool, at("2026-10-05T09:00:00Z")).await?;
    let (sent, seen) = (at("2026-10-30T10:00:00Z"), at("2026-10-31T12:00:00Z"));
    let late = Order::sell("terminal", sent, Some(seen), dec("0.5"), Decimal::ZERO);
    let sell = book.order(&pool, leg, &late).await?;
    let validity = |b: SettlementBarrier| (b.month_invalid(), b.unsettled.incomplete_fills);
    for (m, expected) in [(9, (false, 1)), (10, (true, 1)), (11, (false, 1))] {
        let barrier = book.read(&pool, month(2026, m), now).await?;
        ensure!(validity(barrier) == expected, "month {m}: {barrier:?}");
    }
    book.fill(&pool, &sell, dec("0.5"), "USDT", at("2026-10-31T11:59:00Z")).await?;
    sqlx::query("UPDATE venue_orders SET fills_qty=executed_cum WHERE exchange_order_id=$1 AND account_scope=$2")
        .bind(&sell).bind(&book.scope).execute(&pool).await?;
    let barrier = book.read(&pool, month(2026, 10), now).await?;
    ensure!(barrier.settled() && !barrier.month_invalid(), "settled: {barrier:?}");

    let unknown = Order::sell("unknown", sent, None, Decimal::ZERO, Decimal::ZERO);
    let unknown = book.order(&pool, leg, &unknown).await?;
    for (m, invalid) in [(9, false), (10, true), (11, true)] {
        ensure!(
            book.read(&pool, month(2026, m), now).await?.month_invalid() == invalid,
            "unknown, month {m}"
        );
    }
    sqlx::query("UPDATE venue_orders SET phase='terminal',status='CANCELED',terminal_seen_at=$1 WHERE exchange_order_id=$2 AND account_scope=$3")
        .bind(at("2026-11-03T08:00:00Z")).bind(&unknown).bind(&book.scope).execute(&pool).await?;
    for m in [10, 11] {
        ensure!(
            !book.read(&pool, month(2026, m), now).await?.month_invalid(),
            "resolved, month {m}"
        );
    }
    Ok(())
}

fn dec(value: &str) -> Decimal {
    value.parse().expect("fixture decimal")
}

/// Lives widen by 2 s on both sides; a life ending exactly at a month's
/// first instant still meets it (right-closed, PLAN-r4 P3's reading).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn order_life_widening_keeps_the_edge_month_invalid(pool: PgPool) -> Result<()> {
    let (november, now) = (month(2026, 11), at("2026-12-20T12:00:00Z"));
    let micro = chrono::TimeDelta::microseconds(1);
    let (first, next) = (at("2026-11-01T00:00:00Z"), at("2026-12-01T00:00:00Z"));
    let two = chrono::TimeDelta::seconds(2);
    let half = chrono::TimeDelta::milliseconds(500);
    let cases = [
        // (born, seen terminal, November invalid)
        (at("2026-10-20T00:00:00Z"), first - two + half, true),
        (at("2026-10-20T00:00:00Z"), first - two, true),
        (at("2026-10-20T00:00:00Z"), first - two - micro, false),
        (next + two - half, next + two, true),
        (next + two - micro, next + two, true),
        (next + two, next + two, false),
    ];
    for (born, seen, invalid) in cases {
        let book = Book::new(&pool).await?;
        let leg = book
            .leg(&pool, at("2026-10-01T00:00:00Z"), None, false, false, Decimal::ZERO)
            .await?;
        let order = Order::sell("terminal", born, Some(seen), Decimal::ONE, Decimal::ZERO);
        book.order(&pool, leg, &order).await?;
        let barrier = book.read(&pool, november, now).await?;
        ensure!(
            barrier.month_invalid() == invalid && !barrier.settled(),
            "{born} {seen}: {barrier:?}"
        );
    }
    Ok(())
}

/// Fixture instants: mid-month points and every second from 3 s before to
/// 2 s after the October and November boundaries.
const POINTS: [&str; 15] = [
    "2026-09-15T08:00:00Z",
    "2026-09-30T23:59:57Z",
    "2026-09-30T23:59:58Z",
    "2026-09-30T23:59:59Z",
    "2026-10-01T00:00:00Z",
    "2026-10-01T00:00:01Z",
    "2026-10-01T00:00:02Z",
    "2026-10-15T08:00:00Z",
    "2026-10-31T23:59:57Z",
    "2026-10-31T23:59:58Z",
    "2026-10-31T23:59:59Z",
    "2026-11-01T00:00:00Z",
    "2026-11-01T00:00:01Z",
    "2026-11-01T00:00:02Z",
    "2026-11-10T08:00:00Z",
];
const NOW: &str = "2026-11-20T12:00:00Z";
const MONTHS: [u32; 5] = [8, 9, 10, 11, 12];

#[derive(Debug, Clone)]
struct GenOrder {
    buy: bool,
    phase: &'static str,
    intent: Option<usize>,
    live: usize,
    terminal: usize,
    complete: bool,
    odd: bool,
    filled: bool,
}

#[derive(Debug, Clone)]
struct GenIntent {
    kind: &'static str,
    outcome: Option<&'static str>,
    not_sent: bool,
    created: usize,
    of_leg: bool,
}

#[derive(Debug, Clone)]
struct GenLeg {
    created: usize,
    closed: Option<usize>,
    executable: bool,
    fee_final: bool,
    identity: bool,
    dust: bool,
    entry_unknown: bool,
    /// A suspension on record: `Some(true)` open, `Some(false)` cleared.
    suspension: Option<bool>,
    orders: Vec<GenOrder>,
    intents: Vec<GenIntent>,
}

prop_compose! {
    fn gen_order()(buy in any::<bool>(), phase in prop::sample::select(vec!["live", "terminal", "terminal", "unknown"]),
        intent in prop::option::of(0..POINTS.len()), live in 0..POINTS.len(), terminal in 0..POINTS.len(),
        complete in prop::bool::weighted(0.8), odd in prop::bool::weighted(0.15), filled in any::<bool>()) -> GenOrder {
        GenOrder { buy, phase, intent, live, terminal, complete, odd, filled }
    }
}

prop_compose! {
    fn gen_intent()(kind in prop::sample::select(vec!["entry", "protection", "exit", "cancel"]),
        outcome in prop::sample::select(vec![None, Some("success"), Some("rejected"), Some("rejected"), Some("gone_terminal")]),
        not_sent in any::<bool>(), created in 0..POINTS.len(), of_leg in prop::bool::weighted(0.9)) -> GenIntent {
        GenIntent { kind, outcome, not_sent, created, of_leg }
    }
}

prop_compose! {
    fn gen_leg()(created in 0..POINTS.len(), closed in prop::option::of(0..POINTS.len()),
        executable in prop::bool::weighted(0.8), fee_final in prop::bool::weighted(0.7),
        identity in prop::bool::weighted(0.8), dust in prop::bool::weighted(0.8),
        entry_unknown in prop::bool::weighted(0.1),
        suspension in prop::sample::select(vec![None, None, None, Some(true), Some(false)]),
        orders in prop::collection::vec(gen_order(), 0..3), intents in prop::collection::vec(gen_intent(), 0..3)) -> GenLeg {
        GenLeg { created, closed, executable, fee_final, identity, dust, entry_unknown, suspension, orders, intents }
    }
}

prop_compose! {
    fn generated_case()(legs in prop::collection::vec(gen_leg(), 0..4),
        invalidated in prop::collection::vec(prop::sample::select(MONTHS.to_vec()), 0..3)) -> (Vec<GenLeg>, Vec<u32>) {
        (legs, invalidated)
    }
}

/// The independent oracle: PLAN-r4 §6.3 over the generated model.
fn oracle(legs: &[GenLeg], invalidated: &[u32], m: u32) -> SettlementBarrier {
    let time = |index: usize| at(POINTS[index]);
    let (now, two) = (at(NOW), chrono::TimeDelta::seconds(2));
    let (first, next) = (
        month(2026, m).and_hms_opt(0, 0, 0).unwrap().and_utc(),
        if m == 12 {
            month(2027, 1)
        } else {
            month(2026, m + 1)
        }
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc(),
    );
    let mut lives: Vec<(DateTime<Utc>, Option<DateTime<Utc>>)> = Vec::new();
    let mut unsettled = UnsettledLegs::default();
    let rowless =
        |i: &GenIntent| i.kind != "cancel" && !i.not_sent && i.outcome != Some("rejected");
    for leg in legs {
        let order_unsettled = |o: &GenOrder| o.phase == "unknown" || !o.complete || o.odd;
        let unknown = leg.entry_unknown || leg.orders.iter().any(|o| o.phase == "unknown");
        let provisional = leg.executable && !leg.fee_final;
        let suspended = leg.suspension == Some(true);
        let broken = leg.closed.is_some() && ((leg.executable && !leg.dust) || !leg.identity);
        let wide = leg.entry_unknown || provisional || suspended || broken;
        let flags = [
            unknown,
            leg.intents.iter().any(|i| i.of_leg && rowless(i)),
            leg.orders.iter().any(|o| !o.complete),
            provisional,
            leg.orders.iter().any(|o| o.odd),
            suspended,
            broken,
        ];
        for o in leg.orders.iter().filter(|&o| wide || order_unsettled(o)) {
            let born = o.intent.map_or(time(o.live), |i| time(i).min(time(o.live)));
            lives.push((born, (o.phase == "terminal").then(|| time(o.terminal))));
        }
        if wide {
            lives.push((time(leg.created), leg.closed.map(time)));
        }
        if flags.iter().any(|f| *f) {
            let counts = [
                &mut unsettled.unknown_order,
                &mut unsettled.unrowed_intent,
                &mut unsettled.incomplete_fills,
                &mut unsettled.provisional_fee,
                &mut unsettled.unexpected_asset,
                &mut unsettled.suspended,
                &mut unsettled.broken_identity,
            ];
            for (count, flag) in counts.into_iter().zip(flags) {
                *count += u64::from(flag);
            }
            unsettled.total += 1;
        }
    }
    // Intents naming no leg of the venue share one key.
    if legs.iter().flat_map(|l| &l.intents).any(|i| !i.of_leg && rowless(i)) {
        unsettled.total += 1;
        unsettled.unrowed_intent += 1;
    }
    for intent in legs.iter().flat_map(|l| &l.intents).filter(|&i| rowless(i)) {
        lives.push((time(intent.created), None));
    }
    let meets = lives.iter().any(|(born, ended)| {
        let ended = ended.unwrap_or(now).max(*born);
        *born - two < next && ended + two >= first
    });
    SettlementBarrier {
        unsettled,
        life_meets_month: meets,
        invalidations: invalidated.iter().filter(|x| **x == m).count() as u64,
        not_begun: now < first,
    }
}

async fn write_case(
    pool: &PgPool,
    book: &Book,
    legs: &[GenLeg],
    invalidated: &[u32],
) -> Result<()> {
    let time = |index: usize| at(POINTS[index]);
    for leg in legs {
        let x = dec(if leg.executable { "0.9" } else { "0" });
        let r = dec(if leg.executable && leg.dust {
            "0.1"
        } else {
            "0"
        });
        let consumed = x - r + dec(if leg.identity { "0" } else { "0.05" });
        let position = book
            .leg(
                pool,
                time(leg.created),
                leg.closed.map(time),
                leg.executable,
                leg.fee_final,
                consumed,
            )
            .await?;
        if leg.entry_unknown {
            sqlx::query(
                "UPDATE balance_legs SET recovery_state='entry_unknown' WHERE position_id=$1",
            )
            .bind(position)
            .execute(pool)
            .await?;
        }
        if leg.dust {
            sqlx::query("INSERT INTO dust_ledger (position_id,tenant_id,venue_id,account_scope,symbol,entry_dust,write_off,pooled,close_at,last_fill_at,loss_within_charge,unsellable_in_envelope,min_notional,write_off_price)
                VALUES ($1,$2,'binance:spot',$3,$4,0.099,$5,0.099+$5,$6,$6,true,true,5,90)")
                .bind(position).bind(book.tenant).bind(&book.scope).bind(SYMBOL).bind(r).bind(time(leg.created))
                .execute(pool).await?;
        }
        if let Some(open) = leg.suspension {
            sqlx::query("INSERT INTO stream_suspensions (suspension_id,tenant_id,stream_key,stream_kind,error,suspended_at,cleared_at)
                VALUES ($1,$2,'position:'||$3::text,'position','fixture',$4,$5)")
                .bind(Uuid::now_v7()).bind(book.tenant).bind(position).bind(time(leg.created))
                .bind((!open).then(|| at(NOW))).execute(pool).await?;
        }
        for o in &leg.orders {
            let filled = dec(if o.filled || o.odd { "1" } else { "0" });
            let executed = filled + dec(if o.complete { "0" } else { "1" });
            let seen = (o.phase == "terminal").then(|| time(o.terminal));
            let order = Order::sell(o.phase, time(o.live), seen, executed, filled);
            let asset = o.odd.then_some(if o.buy { "USDT" } else { "BTC" });
            let intent_at = o.intent.map(time);
            book.order(pool, position, &Order { buy: o.buy, intent_at, asset, ..order })
                .await?;
        }
        for i in &leg.intents {
            book.intent(
                pool,
                i.of_leg.then_some(position),
                i.kind,
                i.outcome,
                i.not_sent,
                time(i.created),
                None,
            )
            .await?;
        }
    }
    for m in invalidated {
        let trade = month(2026, *m).and_hms_opt(12, 0, 0).unwrap().and_utc();
        sqlx::query("INSERT INTO accounting_invalidations VALUES ($1,$2,'binance:spot',$3,'unattributed_fill',$1::text,$4,$5,'1',$1::text,$6)")
            .bind(Uuid::now_v7()).bind(book.tenant).bind(month(2026, *m)).bind(&book.scope).bind(SYMBOL).bind(trade)
            .execute(pool).await?;
    }
    // Another venue's outstanding intent of the same tenant counts nowhere.
    sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at)
        VALUES ($1,$2,'binance:usdm',$3,$1::text,'entry','{}',$4)")
        .bind(Uuid::now_v7()).bind(book.tenant).bind(&book.scope).bind(at(NOW)).execute(pool).await?;
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn generated_ledgers_match_the_barrier_oracle(pool: PgPool) -> Result<()> {
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0x8122_5e77),
        failure_persistence: None,
        ..Config::default()
    });
    let (mut legs_seen, mut settled_seen, mut invalid_seen, mut numeric) = (0, 0, 0, 0);
    let mut held = UnsettledLegs::default();
    for case in 0..40 {
        let (legs, invalidated) =
            generated_case().new_tree(&mut runner).expect("bounded case").current();
        let book = Book::new(&pool).await?;
        write_case(&pool, &book, &legs, &invalidated).await?;
        for m in MONTHS {
            let actual = book.read(&pool, month(2026, m), at(NOW)).await?;
            let expected = oracle(&legs, &invalidated, m);
            ensure!(
                actual == expected,
                "case {case} month {m}: {actual:?} != {expected:?}\n{legs:#?}"
            );
            invalid_seen += u32::from(actual.life_meets_month);
        }
        let now = oracle(&legs, &invalidated, 11).unsettled;
        legs_seen += legs.len();
        // A closed leg broken by X != consumed_cum + R alone, its R on record
        // or no X: the witness a missing dust row cannot stand in for.
        numeric += legs
            .iter()
            .filter(|l| l.closed.is_some() && !l.identity && (l.dust || !l.executable))
            .count();
        settled_seen += legs.len().saturating_sub(now.total as usize);
        for (sum, count) in [
            (&mut held.unknown_order, now.unknown_order),
            (&mut held.unrowed_intent, now.unrowed_intent),
            (&mut held.incomplete_fills, now.incomplete_fills),
            (&mut held.provisional_fee, now.provisional_fee),
            (&mut held.unexpected_asset, now.unexpected_asset),
            (&mut held.suspended, now.suspended),
            (&mut held.broken_identity, now.broken_identity),
        ] {
            *sum += count;
        }
    }
    // Every barrier condition and both outcomes were generated.
    let every = [
        held.unknown_order,
        held.unrowed_intent,
        held.incomplete_fills,
        held.provisional_fee,
        held.unexpected_asset,
        held.suspended,
        held.broken_identity,
    ];
    ensure!(
        every.iter().all(|n| *n > 0)
            && numeric > 0
            && legs_seen >= 40
            && settled_seen >= 3
            && invalid_seen >= 20,
        "{held:?} {numeric} {legs_seen} {settled_seen} {invalid_seen}"
    );
    eprintln!("GENERATED_LEDGERS cases=40 legs={legs_seen} settled={settled_seen} invalid_month_reads={invalid_seen} numeric_identity_breaks={numeric} held={held:?}");
    Ok(())
}

/// PLAN-r4 §7.2's cardinality (200 legs, 2,000 intents, 1,000 orders, 10,000
/// fills) and one unknown order: one statement on the wire, every read well
/// inside the 1 s statement budget of an 8l transaction.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn settlement_barrier_is_one_statement_within_its_bound(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    let (tenant, scope) = (book.tenant, &book.scope);
    sqlx::raw_sql(&format!("
        CREATE TEMP TABLE g AS SELECT n, ('10000000-0000-0000-0000-'||lpad(n::text,12,'0'))::uuid AS p,
            ('20000000-0000-0000-0000-'||lpad(n::text,12,'0'))::uuid AS a, n % 2 = 0 AS closed
            FROM generate_series(1,200) n;
        INSERT INTO leg_routes SELECT p,'{tenant}','binance:spot','{scope}','2026-10-01T00:00:00Z' FROM g;
        INSERT INTO balance_leg_admissions (entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,reference_price,remainder_reserve,admission_charge,initial_trigger,created_at)
            SELECT a,p,'{tenant}','binance:spot','{scope}','{SYMBOL}','2026-10-01',10,10,2,5,0.00001,'base','fixture rules','2026-10-01T00:00:00Z','fixture-hash',1,1,100,0.1,1,90,'2026-10-01T00:00:00Z' FROM g;
        INSERT INTO balance_legs (position_id,tenant_id,venue_id,account_scope,symbol,admission_intent_id,decided_month,target_trigger,gross_qty,acquisition_cost,base_commission,base_commission_final,executable_qty,consumed_cum,created_at,closed_at)
            SELECT p,'{tenant}','binance:spot','{scope}','{SYMBOL}',a,'2026-10-01',90,1,100,0.001,true,0.9,
                   CASE WHEN closed THEN 0.9 ELSE 0 END,'2026-10-01T00:00:00Z',CASE WHEN closed THEN timestamptz '2026-10-20T00:00:00Z' END FROM g;
        INSERT INTO dust_ledger (position_id,tenant_id,venue_id,account_scope,symbol,entry_dust,write_off,pooled,close_at,last_fill_at)
            SELECT p,'{tenant}','binance:spot','{scope}','{SYMBOL}',0.099,0,0.099,'2026-10-20T00:00:00Z','2026-10-20T00:00:00Z' FROM g WHERE closed;
        CREATE TEMP TABLE j AS SELECT g.p, k, ('30000000-0000-0000-'||lpad(g.n::text,4,'0')||'-'||lpad(k::text,12,'0'))::uuid AS i
            FROM g, generate_series(1,10) k;
        INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,venue_outcome,created_at)
            SELECT i,'{tenant}','binance:spot','{scope}',i::text,p,CASE WHEN k=1 THEN 'entry' WHEN k<=5 THEN 'protection' ELSE 'cancel' END,
                   '{{}}','success','2026-10-02T00:00:00Z' FROM j;
        INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,stop_price,status,phase,executed_cum,fills_qty,last_checked_at,live_seen_at,terminal_seen_at)
            SELECT 'binance:spot','{scope}','{SYMBOL}',(1000000000+row_number() OVER ())::text,'{tenant}',i::text,i::text,'leg',i,p,
                   CASE WHEN k=1 THEN 'entry' ELSE 'stop' END,CASE WHEN k=1 THEN 'BUY' ELSE 'SELL' END,'MARKET',1,
                   CASE WHEN k>1 THEN 90 END,'FILLED','terminal',1,1,'2026-10-03T00:00:00Z','2026-10-02T00:00:00Z','2026-10-03T00:00:00Z' FROM j WHERE k<=5;
        INSERT INTO order_fills SELECT 'binance:spot','{scope}','{SYMBOL}',(o.exchange_order_id::bigint*10+t)::text,o.exchange_order_id,
                   '{tenant}',100,0.1,10,0.0001,CASE o.side WHEN 'BUY' THEN 'BTC' ELSE 'USDT' END,'2026-10-02T00:00:00Z','2026-10-03T00:00:00Z'
            FROM venue_orders o, generate_series(1,10) t WHERE o.tenant_id='{tenant}';
    ")).execute(&pool).await?;
    let leg = book.settled_leg(&pool, at("2026-11-02T10:00:00Z")).await?;
    let unknown =
        Order::sell("unknown", at("2026-11-03T10:00:00Z"), None, Decimal::ZERO, Decimal::ZERO);
    let unknown = book.order(&pool, leg, &Order { intent_at: None, ..unknown }).await?;
    // Production statistics: autovacuum would have analyzed these by now.
    sqlx::raw_sql("ANALYZE balance_legs, dust_ledger, venue_orders, order_fills, order_intents")
        .execute(&pool)
        .await?;
    let counts: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM balance_legs),(SELECT count(*) FROM order_intents),(SELECT count(*) FROM venue_orders),(SELECT count(*) FROM order_fills)")
        .fetch_one(&pool).await?;
    ensure!(counts.0 >= 200 && counts.1 >= 2_000 && counts.3 >= 10_000, "{counts:?}");

    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .expect("host");
    let proxy = DbProxy::start(upstream).await?;
    let url = database_url_through(
        &std::env::var("DATABASE_URL")?,
        proxy.addr(),
        options.get_database().expect("db"),
    )?;
    let proxied = PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect(&url)
        .await?;
    let mut connection = proxied.acquire().await?;
    let (venue, now) = (venue(), at("2026-11-20T12:00:00Z"));
    let query = SettlementQuery {
        tenant_id: tenant,
        venue: &venue,
        month: month(2026, 11),
        now,
        assets: ASSETS,
    };
    let held = only(|l| l.unknown_order = 1);
    proxy.enable_exact_query_trace(32);
    let mut phases = vec![("auto_held", timed(&mut connection, &query, 21, held).await?)];
    let trace = proxy.query_trace();
    ensure!(
        trace.len() == 21 && trace.iter().all(|sql| sql == SETTLEMENT_BARRIER),
        "one statement per read: {trace:?}"
    );
    // The cached statement's generic plan, and every row scanned to the end
    // once nothing is unsettled.
    sqlx::query("SET plan_cache_mode = force_generic_plan")
        .execute(&mut *connection)
        .await?;
    phases.push(("generic_held", timed(&mut connection, &query, 20, held).await?));
    sqlx::query("UPDATE venue_orders SET phase='terminal',status='CANCELED',terminal_seen_at=$1 WHERE exchange_order_id=$2 AND account_scope=$3")
        .bind(now).bind(&unknown).bind(&book.scope).execute(&pool).await?;
    let settled = UnsettledLegs::default();
    phases.push(("generic_settled", timed(&mut connection, &query, 20, settled).await?));
    sqlx::query("RESET plan_cache_mode").execute(&mut *connection).await?;
    phases.push(("auto_settled", timed(&mut connection, &query, 20, settled).await?));
    let slowest = phases.iter().filter_map(|(_, times)| times.last()).max().copied();
    ensure!(slowest < Some(Duration::from_millis(1000)), "slowest read {slowest:?}");
    let plan: Vec<String> =
        sqlx::query_scalar(&format!("EXPLAIN (ANALYZE, BUFFERS) {SETTLEMENT_BARRIER}"))
            .bind(tenant)
            .bind(venue.as_str())
            .bind(month(2026, 11))
            .bind(now)
            .bind(vec![SYMBOL])
            .bind(vec!["BTC"])
            .bind(vec!["USDT"])
            .fetch_all(&pool)
            .await?;
    let summary: Vec<String> = phases
        .iter()
        .map(|(phase, times)| {
            let micros = |at: usize| times[at].as_micros();
            format!(
                "{phase}: median_us={} max_us={}",
                micros(times.len() / 2),
                micros(times.len() - 1)
            )
        })
        .collect();
    eprintln!(
        "SETTLEMENT_BARRIER_BOUND {} legs={} intents={} orders={} fills={}\n{}",
        summary.join(" "),
        counts.0,
        counts.1,
        counts.2,
        counts.3,
        plan.join("\n")
    );
    Ok(())
}

/// `reads` barrier reads on one connection, each expecting `held`; sorted
/// durations.
async fn timed(
    connection: &mut PgConnection,
    query: &SettlementQuery<'_>,
    reads: usize,
    held: UnsettledLegs,
) -> Result<Vec<Duration>> {
    let mut times = Vec::with_capacity(reads);
    for _ in 0..reads {
        let started = Instant::now();
        let barrier = read_settlement(connection, query).await?;
        times.push(started.elapsed());
        ensure!(barrier.unsettled == held, "{barrier:?}");
    }
    times.sort();
    Ok(times)
}

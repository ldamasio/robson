//! 8p-4 proofs on PostgreSQL (8p PLAN-r3 §8 row 8p-4): the leg-facts reader
//! against generated ledgers and an independent oracle, with 8p-3's snapshot
//! unchanged by the summary; the candidate and summary rules at their edges;
//! the first attempt's verdict; every fact as stored; the owner's (b1) seam
//! through a fixture owner; and the statement's one round trip and bound at
//! PLAN-r3 §7.2's cardinality. Fixture rows are written directly, under
//! `session_replication_role = replica`: their writers are other slices', and
//! their foreign keys (clock samples, attempts) and insert triggers (version
//! bumps, the daily budget's projection) would each need fixtures of their
//! own. Every CHECK constraint still holds; the reader runs as in production.
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

use anyhow::{ensure, Result};
use chrono::{DateTime, TimeDelta, Utc};
use proptest::{
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngSeed, TestRunner},
};
use robson_exec::{
    boundary_carry::{
        snapshot, Boundary, CancelIntent, Dust, Fill, Hold, LegOrder, LegTerms, OpenAfterB,
        RobsonTime, VenueRecordTimes,
    },
    venue::OrderPhase,
    venue_ledger::{ClockEvidence, ClockVerdict, LegOrderKind, SeenAt, VenueIntId},
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, Transaction};

use super::*;
use crate::boundary::{
    mark::{BoundaryLookup, BoundaryOwner, LookupAnswer},
    Failure,
};

const SYMBOL: &str = "BTCUSDT"; // A labelled fixture pair, never a policy restriction.
const DAY: i64 = 86_400_000;
const VALID: Option<ClockVerdict> = Some(ClockVerdict::Valid);
const INVALID: Option<ClockVerdict> = Some(ClockVerdict::Invalid);
static IDS: AtomicU64 = AtomicU64::new(1_000_000);

fn month(year: i32, month: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, 1).unwrap()
}

/// Month K, December 2026; B is its first instant.
fn k() -> NaiveDate {
    month(2026, 12)
}

/// `ms` from B.
fn at(ms: i64) -> DateTime<Utc> {
    k().and_time(NaiveTime::MIN).and_utc() + TimeDelta::milliseconds(ms)
}

/// A fresh venue id of the same width, so text and numeric orders agree.
fn next() -> u64 {
    IDS.fetch_add(1, Ordering::Relaxed)
}

fn oid(id: u64) -> VenueIntId {
    id.to_string().parse().unwrap()
}

/// A Robson time `ms` from B with its stored verdict.
#[derive(Debug, Clone, Copy)]
struct Stamp {
    ms: i64,
    verdict: Option<ClockVerdict>,
}

impl Stamp {
    fn time(self) -> RobsonTime {
        RobsonTime { at: at(self.ms), verdict: self.verdict }
    }

    /// Its verdict column and the evidence columns a verdict carries.
    fn columns(self) -> (Option<&'static str>, Option<Uuid>, Option<i64>) {
        match self.verdict {
            None => (None, None, None),
            Some(ClockVerdict::Valid) => (Some("valid"), Some(Uuid::now_v7()), Some(0)),
            Some(ClockVerdict::Invalid) => (Some("invalid"), Some(Uuid::now_v7()), Some(0)),
        }
    }
}

fn valid(ms: i64) -> Stamp {
    Stamp { ms, verdict: VALID }
}

#[derive(Debug, Clone)]
struct Trade {
    id: u64,
    ms: i64,
    qty: Decimal,
    price: Decimal,
    fee: Decimal,
}

fn trade(ms: i64, qty: Decimal) -> Trade {
    Trade {
        id: 0,
        ms,
        qty,
        price: dec!(91.25),
        fee: dec!(0.0091),
    }
}

/// One order of a leg, its intent's attempts and its trades.
#[derive(Debug, Clone)]
struct GenOrder {
    id: u64,
    intent: Uuid,
    kind: LegOrderKind,
    status: &'static str,
    qty: Decimal,
    stop: Option<Decimal>,
    live: Stamp,
    terminal: Option<Stamp>,
    attempts: Vec<Stamp>,
    trades: Vec<Trade>,
}

/// Robson's cancel of the leg's order at `pick` (modulo their count), with
/// its attempts.
#[derive(Debug, Clone)]
struct GenCancel {
    intent: Uuid,
    pick: usize,
    target: u64,
    outcome: Option<&'static str>,
    class: Option<&'static str>,
    attempts: Vec<Stamp>,
}

#[derive(Debug, Clone)]
struct GenLeg {
    position: Uuid,
    cohort: Option<NaiveDate>,
    executable: Option<Decimal>,
    terms: LegTerms,
    /// R, the close and the last fill's time.
    dust: Option<(Decimal, Stamp, i64)>,
    orders: Vec<GenOrder>,
    cancels: Vec<GenCancel>,
}

fn terms() -> LegTerms {
    LegTerms {
        gap_bps: dec!(10),
        taker_bps: dec!(10.5),
        remainder_reserve: dec!(0.0000000000000000000000000008),
        admission_charge: dec!(19.70677056),
    }
}

/// An entry filled a month before B and acknowledged then.
fn entry() -> GenOrder {
    let trades = vec![trade(-30 * DAY, dec!(0.6)), trade(-30 * DAY + 5, dec!(0.4))];
    GenOrder {
        id: 0,
        intent: Uuid::nil(),
        kind: LegOrderKind::Entry,
        status: "FILLED",
        qty: dec!(1),
        stop: None,
        live: valid(-30 * DAY),
        terminal: Some(valid(-30 * DAY + 1_000)),
        attempts: vec![valid(-30 * DAY - 1_000)],
        trades,
    }
}

fn order(kind: LegOrderKind, status: &'static str, terminal: Option<Stamp>) -> GenOrder {
    let stop = (kind == LegOrderKind::Stop).then_some(dec!(90.123456789));
    let (qty, live) = (dec!(0.9), valid(-20 * DAY));
    GenOrder {
        kind,
        status,
        stop,
        qty,
        live,
        terminal,
        trades: vec![],
        ..entry()
    }
}

fn leg(cohort: Option<NaiveDate>, orders: Vec<GenOrder>) -> GenLeg {
    GenLeg {
        position: Uuid::nil(),
        cohort,
        executable: Some(dec!(0.9)),
        terms: terms(),
        dust: None,
        orders,
        cancels: vec![],
    }
}

/// Fresh ids for the legs, orders, intents, trades and cancel targets.
fn identify(legs: &mut [GenLeg]) {
    for leg in legs {
        leg.position = Uuid::now_v7();
        for order in &mut leg.orders {
            (order.id, order.intent) = (next(), Uuid::now_v7());
            order.trades.iter_mut().for_each(|trade| trade.id = next());
        }
        for cancel in &mut leg.cancels {
            cancel.intent = Uuid::now_v7();
            let ids: Vec<u64> = leg.orders.iter().map(|order| order.id).collect();
            cancel.target = ids.get(cancel.pick % ids.len().max(1)).copied().unwrap_or_else(next);
        }
    }
}

/// One tenant's balance-account venue, as 8l-1's tables and 8m would hold it.
struct Book {
    tenant: Uuid,
    venue: &'static str,
    scope: String,
}

impl Book {
    async fn new(pool: &PgPool) -> Result<Self> {
        let book = Self {
            tenant: Uuid::now_v7(),
            venue: VenueId::BINANCE_SPOT,
            scope: String::new(),
        };
        book.on(pool, VenueId::BINANCE_SPOT).await
    }

    /// The same tenant on `venue`, with a scope of its own.
    async fn on(&self, pool: &PgPool, venue: &'static str) -> Result<Self> {
        sqlx::query("INSERT INTO accounting_state (tenant_id,venue_id,budget_model) VALUES ($1,$2,'net_from_start_non_expanding_v1')")
            .bind(self.tenant).bind(venue).execute(pool).await?;
        Ok(Self {
            tenant: self.tenant,
            venue,
            scope: format!("facts-{}", Uuid::now_v7()),
        })
    }

    async fn read(&self, pool: &PgPool) -> Result<Vec<CandidateLeg>, LedgerError> {
        let mut connection = pool.acquire().await.map_err(backend)?;
        read_leg_facts(&mut connection, self.tenant, &VenueId::new(self.venue), k()).await
    }

    async fn write(&self, pool: &PgPool, legs: &[GenLeg]) -> Result<()> {
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL session_replication_role = replica")
            .execute(&mut *tx)
            .await?;
        for leg in legs {
            self.leg(&mut tx, leg).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn leg(&self, tx: &mut Transaction<'_, Postgres>, leg: &GenLeg) -> Result<()> {
        let (position, admission, created) = (leg.position, Uuid::now_v7(), at(-60 * DAY));
        let decided = leg.cohort.unwrap_or(k());
        sqlx::query("INSERT INTO leg_routes VALUES ($1,$2,$3,$4,$5)")
            .bind(position)
            .bind(self.tenant)
            .bind(self.venue)
            .bind(&self.scope)
            .bind(created)
            .execute(&mut **tx)
            .await?;
        // A superseded admission with other terms: only the current one counts.
        let stale = LegTerms {
            gap_bps: dec!(99),
            taker_bps: dec!(99),
            remainder_reserve: dec!(0.5),
            admission_charge: dec!(99),
        };
        for (id, terms, successor) in [
            (Uuid::now_v7(), stale, Some(admission)),
            (admission, leg.terms, None),
        ] {
            sqlx::query("INSERT INTO balance_leg_admissions (entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,reference_price,remainder_reserve,admission_charge,initial_trigger,created_at,superseded_at,superseded_by)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,2,5,0.00001,'base','fixture rules',$10,'fixture-hash',1,1,100,$11,$12,90,$10,$13,$14)")
                .bind(id).bind(position).bind(self.tenant).bind(self.venue).bind(&self.scope).bind(SYMBOL).bind(decided)
                .bind(terms.taker_bps).bind(terms.gap_bps).bind(created).bind(terms.remainder_reserve)
                .bind(terms.admission_charge).bind(successor.map(|_| created)).bind(successor)
                .execute(&mut **tx).await?;
        }
        let quantities = leg.executable.map(|x| [dec!(1), dec!(100), dec!(0.001), x]);
        let quantity = |at: usize| quantities.map(|q| q[at]);
        sqlx::query("INSERT INTO balance_legs (position_id,tenant_id,venue_id,account_scope,symbol,admission_intent_id,decided_month,cohort_month,target_trigger,gross_qty,acquisition_cost,base_commission,base_commission_final,executable_qty,consumed_cum,created_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,90,$9,$10,$11,$12,$13,0,$14)")
            .bind(position).bind(self.tenant).bind(self.venue).bind(&self.scope).bind(SYMBOL).bind(admission).bind(decided)
            .bind(leg.cohort).bind(quantity(0)).bind(quantity(1)).bind(quantity(2)).bind(quantities.is_some()).bind(quantity(3))
            .bind(created).execute(&mut **tx).await?;
        for order in &leg.orders {
            self.order(tx, position, order).await?;
        }
        for cancel in &leg.cancels {
            self.intent(
                tx,
                position,
                cancel.intent,
                "cancel",
                Some(cancel.target.to_string()),
                cancel.outcome,
                cancel.class,
            )
            .await?;
            for attempt in &cancel.attempts {
                self.attempt(tx, cancel.intent, *attempt).await?;
            }
        }
        if let Some((write_off, close, last_fill)) = leg.dust {
            let (verdict, sample, delta) = close.columns();
            let covered = (write_off > Decimal::ZERO).then_some(true);
            sqlx::query("INSERT INTO dust_ledger (position_id,tenant_id,venue_id,account_scope,symbol,entry_dust,write_off,pooled,close_at,close_clock_sample_id,close_clock_wall_delta_ms,close_clock_mono_delta_ms,close_clock_verdict,last_fill_at,loss_within_charge,unsellable_in_envelope,min_notional,write_off_price)
                VALUES ($1,$2,$3,$4,$5,0.0001,$6,0.0001+$6,$7,$8,$9,$9,$10,$11,$12,$12,$13,$13)")
                .bind(position).bind(self.tenant).bind(self.venue).bind(&self.scope).bind(SYMBOL).bind(write_off)
                .bind(at(close.ms)).bind(sample).bind(delta).bind(verdict).bind(at(last_fill)).bind(covered)
                .bind(covered.map(|_| dec!(5))).execute(&mut **tx).await?;
        }
        Ok(())
    }

    /// The order with venue times that would mislead if read, its own intent
    /// (never a cancel), its attempts and its trades.
    async fn order(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        position: Uuid,
        order: &GenOrder,
    ) -> Result<()> {
        let (live_verdict, live_sample, live_delta) = order.live.columns();
        let (terminal_verdict, terminal_sample, terminal_delta) =
            order.terminal.map_or((None, None, None), Stamp::columns);
        let phase = match OrderPhase::from_binance_spot_status(order.status) {
            OrderPhase::Live => "live",
            OrderPhase::Terminal => "terminal",
            OrderPhase::Unknown => "unknown",
        };
        let (kind, side, journal) = match order.kind {
            LegOrderKind::Entry => ("entry", "BUY", "entry"),
            LegOrderKind::Stop => ("stop", "SELL", "protection"),
            LegOrderKind::Sell => ("sell", "SELL", "exit"),
        };
        let executed: Decimal = order.trades.iter().map(|trade| trade.qty).sum();
        sqlx::query("INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,stop_price,status,phase,executed_cum,fills_qty,last_checked_at,venue_time_ms,venue_update_time_ms,venue_transact_time_ms,venue_working_time_ms,live_seen_at,live_clock_sample_id,live_clock_wall_delta_ms,live_clock_mono_delta_ms,live_clock_verdict,terminal_seen_at,terminal_clock_sample_id,terminal_clock_wall_delta_ms,terminal_clock_mono_delta_ms,terminal_clock_verdict)
            VALUES ($1,$2,$3,$4,$5,$4,$4,'leg',$6,$7,$8,$9,'STOP_LOSS_LIMIT',$10,$11,$12,$13,$14,$14,$15,$16,$17,$17,$16,$18,$19,$20,$20,$21,$22,$23,$24,$24,$25)")
            .bind(self.venue).bind(&self.scope).bind(SYMBOL).bind(order.id.to_string()).bind(self.tenant).bind(order.intent)
            .bind(position).bind(kind).bind(side).bind(order.qty).bind(order.stop).bind(order.status).bind(phase).bind(executed)
            .bind(at(3 * DAY)).bind(at(DAY).timestamp_millis()).bind(at(-DAY).timestamp_millis())
            .bind(at(order.live.ms)).bind(live_sample).bind(live_delta).bind(live_verdict)
            .bind(order.terminal.map(|stamp| at(stamp.ms))).bind(terminal_sample).bind(terminal_delta).bind(terminal_verdict)
            .execute(&mut **tx).await?;
        self.intent(tx, position, order.intent, journal, None, Some("success"), None)
            .await?;
        for attempt in &order.attempts {
            self.attempt(tx, order.intent, *attempt).await?;
        }
        for trade in &order.trades {
            let asset = if order.kind == LegOrderKind::Entry {
                "BTC"
            } else {
                "USDT"
            };
            sqlx::query(
                "INSERT INTO order_fills VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$12)",
            )
            .bind(self.venue)
            .bind(&self.scope)
            .bind(SYMBOL)
            .bind(trade.id.to_string())
            .bind(order.id.to_string())
            .bind(self.tenant)
            .bind(trade.price)
            .bind(trade.qty)
            .bind((trade.qty * trade.price).round_dp(8))
            .bind(trade.fee)
            .bind(asset)
            .bind(at(trade.ms))
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn intent(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        position: Uuid,
        intent: Uuid,
        kind: &str,
        target: Option<String>,
        outcome: Option<&str>,
        class: Option<&str>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,cancels_order_id,params,venue_outcome,cancel_class,created_at)
            VALUES ($1,$2,$3,$4,$1::text,$5,$6,$7,'{}',$8,$9,$10)")
            .bind(intent).bind(self.tenant).bind(self.venue).bind(&self.scope).bind(position).bind(kind)
            .bind(target).bind(outcome).bind(class).bind(at(-DAY))
            .execute(&mut **tx).await?;
        Ok(())
    }

    async fn attempt(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        intent: Uuid,
        attempt: Stamp,
    ) -> Result<()> {
        let (verdict, sample, delta) = attempt.columns();
        sqlx::query("INSERT INTO spot_intent_clock (attempt_id,intent_id,tenant_id,venue_id,attempt_created_at,clock_sample_id,clock_wall_delta_ms,clock_mono_delta_ms,clock_verdict)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$7,$8)")
            .bind(Uuid::now_v7()).bind(intent).bind(self.tenant).bind(self.venue).bind(at(attempt.ms))
            .bind(sample).bind(delta).bind(verdict).execute(&mut **tx).await?;
        Ok(())
    }
}

/// The aggregate an intent's attempts give (PLAN-r3 §6.3's first attempt):
/// the earliest, proven only when every attempt's verdict is valid.
fn first_attempt(attempts: &[Stamp]) -> Option<RobsonTime> {
    let earliest = attempts.iter().map(|attempt| attempt.ms).min()?;
    let verdict = if attempts.iter().all(|attempt| attempt.verdict == VALID) {
        VALID
    } else if attempts.iter().any(|attempt| attempt.verdict == INVALID) {
        INVALID
    } else {
        None
    };
    Some(RobsonTime { at: at(earliest), verdict })
}

/// Carried into K: a cohort before K and no close proven before B.
fn carried(leg: &GenLeg) -> bool {
    let proven = |(_, close, last): &(Decimal, Stamp, i64)| {
        close.verdict == VALID && close.ms <= -2_000 && *last < 0
    };
    leg.cohort.is_some_and(|cohort| cohort < k()) && !leg.dust.as_ref().is_some_and(proven)
}

/// Summarized: a stop or sell seen terminal by B - 2 s with a valid verdict
/// and every trade before B.
fn hidden(order: &GenOrder) -> bool {
    let terminal = order.terminal.is_some_and(|t| t.verdict == VALID && t.ms <= -2_000);
    order.kind != LegOrderKind::Entry && terminal && order.trades.iter().all(|trade| trade.ms < 0)
}

fn leg_order(order: &GenOrder) -> LegOrder {
    let mut trades = order.trades.clone();
    trades.sort_by_key(|trade| (trade.ms, trade.id));
    let fill = |t: &Trade| Fill {
        trade_time: at(t.ms),
        qty: t.qty,
        price: t.price,
        commission: t.fee,
    };
    LegOrder {
        exchange_order_id: oid(order.id),
        kind: order.kind,
        status: order.status.into(),
        phase: OrderPhase::from_binance_spot_status(order.status),
        orig_qty: order.qty,
        stop_price: order.stop,
        live_seen: order.live.time(),
        terminal_seen: order.terminal.map(Stamp::time),
        first_attempt: first_attempt(&order.attempts),
        fills: trades.iter().map(fill).collect(),
        lookup: None,
        venue_record_times: VenueRecordTimes::default(),
    }
}

/// The independent oracle: PLAN-r3 §6.4's candidates as 8p-3 wants them, by
/// position, with or without the summary.
fn expected(legs: &[GenLeg], summarize: bool) -> Vec<CandidateLeg> {
    let mut out: Vec<CandidateLeg> = (legs.iter().filter(|leg| carried(leg)))
        .map(|leg| {
            let (gone, kept): (Vec<&GenOrder>, Vec<&GenOrder>) =
                leg.orders.iter().partition(|order| summarize && hidden(order));
            let mut orders: Vec<LegOrder> = kept.into_iter().map(leg_order).collect();
            orders.sort_by_key(|order| order.exchange_order_id);
            let sold = gone.iter().flat_map(|order| &order.trades).filter(|trade| trade.ms < 0);
            let mut cancels = leg.cancels.clone();
            cancels.sort_by_key(|cancel| cancel.intent);
            let cancel = |c: GenCancel| CancelIntent {
                position_id: leg.position,
                cancels_order_id: oid(c.target),
                venue_outcome: c.outcome.map(Into::into),
                cancel_class: c.class.map(Into::into),
                first_attempt: first_attempt(&c.attempts),
            };
            CandidateLeg {
                position_id: leg.position,
                cohort_month: leg.cohort,
                executable: leg.executable,
                terms: leg.terms,
                orders,
                summarized_sold_before_b: sold.map(|trade| trade.qty).sum(),
                cancels: cancels.into_iter().map(cancel).collect(),
                dust: leg.dust.map(|(write_off, close, last)| Dust {
                    write_off,
                    close_at: close.time(),
                    last_fill_at: at(last),
                }),
            }
        })
        .collect();
    out.sort_by_key(|leg| leg.position_id);
    out
}

const POINTS: [i64; 12] = [
    -30 * DAY,
    -10_000,
    -2_001,
    -2_000,
    -1_999,
    -1,
    0,
    1,
    1_999,
    2_000,
    10_000,
    3 * DAY,
];

fn stamp() -> impl Strategy<Value = Stamp> {
    let verdicts = vec![VALID, VALID, INVALID, None];
    (prop::sample::select(POINTS.to_vec()), prop::sample::select(verdicts))
        .prop_map(|(ms, verdict)| Stamp { ms, verdict })
}

/// Mostly the sane story (attempted, acknowledged, then terminal near B),
/// sometimes any stamp at all.
fn sane(ms: Vec<i64>) -> impl Strategy<Value = Stamp> {
    prop_oneof![3 => prop::sample::select(ms).prop_map(valid), 1 => stamp()]
}

fn gen_trade() -> impl Strategy<Value = Trade> {
    let quantities = vec![dec!(0.1), dec!(0.05), dec!(0.0000000000000000000000000001)];
    (prop::sample::select(POINTS[1..].to_vec()), prop::sample::select(quantities))
        .prop_map(|(ms, qty)| trade(ms, qty))
}

fn gen_order() -> impl Strategy<Value = GenOrder> {
    let statuses = vec![
        "NEW",
        "PARTIALLY_FILLED",
        "FILLED",
        "CANCELED",
        "CANCELED",
        "EXPIRED",
        "PENDING_CANCEL",
    ];
    let quantities = vec![dec!(0.9), dec!(0.5), dec!(0.3000000000000000000000000001)];
    (
        any::<bool>(),
        prop::sample::select(statuses),
        prop::sample::select(quantities),
        (sane(vec![-20 * DAY]), sane(vec![-10_000, -2_001, -2_000, 10_000])),
        prop::collection::vec(sane(vec![-21 * DAY]), 0..3),
        prop::collection::vec(gen_trade(), 0..3),
    )
        .prop_map(|(stop, status, qty, (live, terminal), attempts, trades)| {
            let kind = if stop {
                LegOrderKind::Stop
            } else {
                LegOrderKind::Sell
            };
            let terminal = (OrderPhase::from_binance_spot_status(status) == OrderPhase::Terminal)
                .then_some(terminal);
            GenOrder {
                qty,
                live,
                attempts,
                trades,
                ..order(kind, status, terminal)
            }
        })
}

fn gen_entry() -> impl Strategy<Value = GenOrder> {
    let fills = vec![-30 * DAY, -30 * DAY, -1, 0, 10_000];
    (
        prop::sample::select(fills),
        sane(vec![-30 * DAY + 1_000]),
        prop::collection::vec(stamp(), 0..2),
    )
        .prop_map(|(ms, terminal, attempts)| {
            let mut entry = GenOrder {
                terminal: Some(terminal),
                attempts,
                ..entry()
            };
            entry.trades[0].ms = ms;
            entry
        })
}

fn gen_cancel() -> impl Strategy<Value = GenCancel> {
    let outcomes = vec![
        None,
        Some("success"),
        Some("success"),
        Some("rejected"),
        Some("gone_terminal"),
    ];
    let classes = vec![None, Some("cancelled"), Some("cancelled"), Some("gone")];
    (
        0..8_usize,
        prop::sample::select(outcomes),
        prop::sample::select(classes),
        prop::collection::vec(stamp(), 0..3),
    )
        .prop_map(|(pick, outcome, class, attempts)| GenCancel {
            intent: Uuid::nil(),
            pick,
            target: 0,
            outcome,
            class,
            attempts,
        })
}

fn gen_leg() -> impl Strategy<Value = GenLeg> {
    let cohorts = vec![
        Some(month(2026, 10)),
        Some(month(2026, 11)),
        Some(month(2026, 11)),
        Some(k()),
        Some(month(2027, 1)),
        None,
    ];
    let dust = (
        prop::sample::select(vec![dec!(0), dec!(0.001)]),
        stamp(),
        prop::sample::select(POINTS.to_vec()),
    );
    (
        prop::sample::select(cohorts),
        prop::sample::select(vec![Some(dec!(0.9)), Some(dec!(0.999)), None]),
        prop::option::weighted(0.4, dust),
        gen_entry(),
        prop::collection::vec(gen_order(), 0..4),
        prop::collection::vec(gen_cancel(), 0..3),
    )
        .prop_map(|(cohort, executable, dust, entry, mut orders, cancels)| {
            orders.insert(0, entry);
            GenLeg {
                executable,
                dust,
                cancels,
                ..leg(cohort, orders)
            }
        })
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn leg_facts_match_the_generated_ledger_oracle(pool: PgPool) -> Result<()> {
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0x8b4f_ac75),
        failure_persistence: None,
        ..Config::default()
    });
    let boundary = Boundary::new(k(), dec!(91.5)).unwrap();
    let (mut read, mut listed, mut summarized, mut agreed, mut shorter, mut held) =
        (0, 0, 0, 0, 0, 0);
    for case in 0..60 {
        let tree = prop::collection::vec(gen_leg(), 0..6).new_tree(&mut runner);
        let mut legs = tree.expect("bounded case").current();
        identify(&mut legs);
        let book = Book::new(&pool).await?;
        book.write(&pool, &legs).await?;
        let actual = book.read(&pool).await?;
        let wanted = expected(&legs, true);
        ensure!(actual == wanted, "case {case}: {actual:#?}\n!=\n{wanted:#?}\n{legs:#?}");
        // The summary never changes what 8p-3 concludes from a leg's facts it
        // would accept whole: the same carried leg, or the same hold.
        for (summary, whole) in actual.iter().zip(&expected(&legs, false)) {
            let whole_b = snapshot(dec!(1000), std::slice::from_ref(whole), &boundary);
            if whole_b != Err(Hold::Invalid) {
                let summary_b = snapshot(dec!(1000), std::slice::from_ref(summary), &boundary);
                ensure!(
                    summary_b == whole_b,
                    "case {case}: {summary_b:?} != {whole_b:?}\n{legs:#?}"
                );
                agreed += 1;
                shorter += usize::from(summary.orders.len() < whole.orders.len());
                held += usize::from(whole_b.is_err());
            }
        }
        read += actual.len();
        listed += actual.iter().map(|leg| leg.orders.len()).sum::<usize>();
        let carried_legs = legs.iter().filter(|leg| carried(leg));
        summarized +=
            carried_legs.flat_map(|leg| &leg.orders).filter(|order| hidden(order)).count();
    }
    ensure!(
        read >= 30 && listed >= 50 && summarized >= 8 && agreed >= 10 && shorter >= 3,
        "{read} {listed} {summarized} {agreed} {shorter} {held}"
    );
    eprintln!("GENERATED_LEG_FACTS cases=60 legs={read} listed={listed} summarized={summarized} legs_agreed={agreed} with_summary={shorter} held={held}");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn leg_facts_read_only_legs_carried_into_k(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    let with_dust = |dust| GenLeg {
        dust: Some(dust),
        ..leg(Some(month(2026, 10)), vec![entry()])
    };
    let mut legs = vec![
        leg(Some(month(2026, 11)), vec![entry()]),
        // Proven finished before B: a valid close by B - 2 s, last fill before B.
        with_dust((dec!(0), valid(-2_000), -1)),
        with_dust((dec!(0.001), valid(-30 * DAY), -30 * DAY)),
        // Not proven: within the margin, an invalid or no verdict, a fill at B.
        with_dust((dec!(0), valid(-1_999), -1)),
        with_dust((dec!(0), Stamp { ms: -10_000, verdict: INVALID }, -1)),
        with_dust((dec!(0.001), Stamp { ms: -10_000, verdict: None }, -10_000)),
        with_dust((dec!(0), valid(-10_000), 0)),
        // No cohort before K: none, K's own, the next.
        leg(None, vec![entry()]),
        leg(Some(k()), vec![entry()]),
        leg(Some(month(2027, 1)), vec![entry()]),
    ];
    identify(&mut legs);
    book.write(&pool, &legs).await?;
    // The same tenant's other venue and another tenant on this one.
    let (mut usdm, mut foreign) = (vec![leg(Some(month(2026, 11)), vec![entry()])], legs.clone());
    identify(&mut usdm);
    identify(&mut foreign);
    book.on(&pool, VenueId::BINANCE_USDM).await?.write(&pool, &usdm).await?;
    Book::new(&pool).await?.write(&pool, &foreign).await?;
    let read: Vec<Uuid> = book.read(&pool).await?.iter().map(|leg| leg.position_id).collect();
    let mut carried: Vec<Uuid> = [0, 3, 4, 5, 6].iter().map(|at| legs[*at].position).collect();
    carried.sort();
    ensure!(read == carried, "{read:?} != {carried:?}");
    ensure!(book.read(&pool).await? == expected(&legs, true));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn leg_facts_summarize_only_orders_shown_not_open_at_b(pool: PgPool) -> Result<()> {
    use LegOrderKind::{Sell, Stop};
    let with_trades = |order: GenOrder, trades: Vec<Trade>| GenOrder { trades, ..order };
    let orders = vec![
        entry(),
        // Seen terminal by B - 2 s, every trade before B: summarized.
        with_trades(order(Stop, "CANCELED", Some(valid(-2_000))), vec![trade(-10_000, dec!(0.1))]),
        with_trades(order(Sell, "FILLED", Some(valid(-30 * DAY))), vec![
            trade(-31 * DAY, dec!(0.05)),
            trade(-30 * DAY - 1, dec!(0.1)),
        ]),
        // A trade at B, within the margin, an invalid verdict, still live.
        with_trades(order(Sell, "FILLED", Some(valid(-2_000))), vec![trade(0, dec!(0.1))]),
        order(Stop, "CANCELED", Some(valid(-1_999))),
        order(Stop, "EXPIRED", Some(Stamp { ms: -10_000, verdict: INVALID })),
        with_trades(order(Stop, "PARTIALLY_FILLED", None), vec![trade(-5_000, dec!(0.2))]),
    ];
    let mut legs = vec![leg(Some(month(2026, 11)), orders)];
    identify(&mut legs);
    let book = Book::new(&pool).await?;
    book.write(&pool, &legs).await?;
    let read = book.read(&pool).await?;
    let listed: Vec<VenueIntId> =
        read[0].orders.iter().map(|order| order.exchange_order_id).collect();
    let wanted: Vec<VenueIntId> =
        [0, 3, 4, 5, 6].iter().map(|at| oid(legs[0].orders[*at].id)).collect();
    ensure!(listed == wanted, "{listed:?} != {wanted:?}");
    ensure!(read[0].summarized_sold_before_b == dec!(0.25), "{read:?}");
    ensure!(read == expected(&legs, true));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_first_attempt_is_proven_only_when_every_attempt_is_valid(pool: PgPool) -> Result<()> {
    let some = |ms, verdict| Stamp { ms, verdict };
    let cases = [
        (vec![valid(20_000), valid(10_000)], Some(valid(10_000))),
        (vec![valid(10_000), some(30_000, INVALID)], Some(some(10_000, INVALID))),
        (vec![valid(10_000), some(30_000, None)], Some(some(10_000, None))),
        (vec![some(5_000, None), some(30_000, INVALID)], Some(some(5_000, INVALID))),
        (vec![], None),
    ];
    let orders = (cases.iter())
        .map(|(attempts, _)| GenOrder {
            attempts: attempts.clone(),
            ..order(LegOrderKind::Stop, "NEW", None)
        })
        .collect();
    let cancel = |attempts| GenCancel {
        intent: Uuid::nil(),
        pick: 0,
        target: 0,
        outcome: Some("success"),
        class: Some("cancelled"),
        attempts,
    };
    let cancels = vec![
        cancel(vec![valid(3_000), some(2_000, INVALID)]),
        cancel(vec![valid(4_000)]),
    ];
    let mut legs = vec![GenLeg {
        cancels,
        ..leg(Some(month(2026, 11)), orders)
    }];
    identify(&mut legs);
    let book = Book::new(&pool).await?;
    book.write(&pool, &legs).await?;
    let read = book.read(&pool).await?;
    for (order, (_, first)) in legs[0].orders.iter().zip(&cases) {
        let found = read[0].orders.iter().find(|o| o.exchange_order_id == oid(order.id));
        let first_attempt = found.map(|o| o.first_attempt);
        ensure!(first_attempt == Some(first.map(Stamp::time)), "{first_attempt:?} != {first:?}");
    }
    let mut firsts: Vec<_> = read[0].cancels.iter().map(|cancel| cancel.first_attempt).collect();
    firsts.sort_by_key(|first| first.map(|time| time.at));
    ensure!(
        firsts == [Some(some(2_000, INVALID).time()), Some(valid(4_000).time())],
        "{firsts:?}"
    );
    ensure!(read == expected(&legs, true));
    Ok(())
}

/// Every fact as stored, written out by hand rather than by the oracle: exact
/// 28-place decimals, every verdict, the current admission's terms, venue
/// times that are never read, every cancel of the leg and none of another
/// kind; and a fact 8p-3's types cannot hold refuses the whole read.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn leg_facts_hand_over_every_fact_as_stored(pool: PgPool) -> Result<()> {
    let fine = dec!(0.0000000000000000000000000001);
    let stop = GenOrder {
        live: Stamp { ms: -10_000, verdict: None },
        attempts: vec![valid(-20 * DAY)],
        trades: vec![trade(5_000, fine), Trade {
            price: dec!(123456.7890123456789012345678),
            ..trade(-1, dec!(0.1))
        }],
        ..order(LegOrderKind::Stop, "PARTIALLY_FILLED", None)
    };
    let sell = order(LegOrderKind::Sell, "EXPIRED", Some(Stamp { ms: -5_000, verdict: INVALID }));
    let cancel = |outcome, class| GenCancel {
        intent: Uuid::nil(),
        pick: 2,
        target: 0,
        outcome,
        class,
        attempts: vec![valid(3_000)],
    };
    let mut legs = vec![GenLeg {
        dust: Some((dec!(0.001), valid(-1_000), -30 * DAY)),
        cancels: vec![
            cancel(Some("success"), Some("cancelled")),
            cancel(None, None),
        ],
        ..leg(Some(month(2026, 11)), vec![entry(), stop, sell])
    }];
    identify(&mut legs);
    let book = Book::new(&pool).await?;
    book.write(&pool, &legs).await?;
    let read = book.read(&pool).await?;
    let gen = &legs[0];
    let (entry_id, stop_id, sell_id) =
        (oid(gen.orders[0].id), oid(gen.orders[1].id), oid(gen.orders[2].id));
    let time = |ms, verdict| RobsonTime { at: at(ms), verdict };
    let fill = |ms, qty, price| Fill {
        trade_time: at(ms),
        qty,
        price,
        commission: dec!(0.0091),
    };
    let listed =
        |id, kind, status: &str, phase, stop_price, live, terminal_seen, first_attempt, fills| {
            LegOrder {
                exchange_order_id: id,
                kind,
                status: status.into(),
                phase,
                orig_qty: dec!(0.9),
                stop_price,
                live_seen: live,
                terminal_seen,
                first_attempt,
                fills,
                lookup: None,
                venue_record_times: VenueRecordTimes::default(),
            }
        };
    let mut cancels = vec![
        (gen.cancels[0].intent, sell_id, Some("success"), Some("cancelled")),
        (gen.cancels[1].intent, sell_id, None, None),
    ];
    cancels.sort_by_key(|cancel| cancel.0);
    let wanted = vec![CandidateLeg {
        position_id: gen.position,
        cohort_month: Some(month(2026, 11)),
        executable: Some(dec!(0.9)),
        terms: LegTerms {
            gap_bps: dec!(10),
            taker_bps: dec!(10.5),
            remainder_reserve: dec!(0.0000000000000000000000000008),
            admission_charge: dec!(19.70677056),
        },
        orders: vec![
            LegOrder {
                orig_qty: dec!(1),
                fills: vec![
                    fill(-30 * DAY, dec!(0.6), dec!(91.25)),
                    fill(-30 * DAY + 5, dec!(0.4), dec!(91.25)),
                ],
                ..listed(
                    entry_id,
                    LegOrderKind::Entry,
                    "FILLED",
                    OrderPhase::Terminal,
                    None,
                    time(-30 * DAY, VALID),
                    Some(time(-30 * DAY + 1_000, VALID)),
                    Some(time(-30 * DAY - 1_000, VALID)),
                    vec![],
                )
            },
            listed(
                stop_id,
                LegOrderKind::Stop,
                "PARTIALLY_FILLED",
                OrderPhase::Live,
                Some(dec!(90.123456789)),
                time(-10_000, None),
                None,
                Some(time(-20 * DAY, VALID)),
                vec![
                    fill(-1, dec!(0.1), dec!(123456.7890123456789012345678)),
                    fill(5_000, fine, dec!(91.25)),
                ],
            ),
            listed(
                sell_id,
                LegOrderKind::Sell,
                "EXPIRED",
                OrderPhase::Terminal,
                None,
                time(-20 * DAY, VALID),
                Some(time(-5_000, INVALID)),
                Some(time(-30 * DAY - 1_000, VALID)),
                vec![],
            ),
        ],
        summarized_sold_before_b: Decimal::ZERO,
        cancels: cancels
            .into_iter()
            .map(|(_, target, outcome, class)| CancelIntent {
                position_id: gen.position,
                cancels_order_id: target,
                venue_outcome: outcome.map(Into::into),
                cancel_class: class.map(Into::into),
                first_attempt: Some(time(3_000, VALID)),
            })
            .collect(),
        dust: Some(Dust {
            write_off: dec!(0.001),
            close_at: time(-1_000, VALID),
            last_fill_at: at(-30 * DAY),
        }),
    }];
    ensure!(read == wanted, "{read:#?}\n!=\n{wanted:#?}");
    let (scale, fine_scale) =
        (read[0].terms.remainder_reserve.scale(), read[0].orders[1].fills[1].qty.scale());
    ensure!((scale, fine_scale) == (28, 28), "{scale} {fine_scale}");
    // A cancel naming no venue integer id, or one not in its canonical text, is
    // a fact 8p-3 cannot hold: no answer.
    for target in [None, Some("000123"), Some("12a")] {
        let broken = Book::new(&pool).await?;
        let mut legs = vec![leg(Some(month(2026, 11)), vec![entry()])];
        identify(&mut legs);
        broken.write(&pool, &legs).await?;
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL session_replication_role = replica")
            .execute(&mut *tx)
            .await?;
        let (position, intent) = (legs[0].position, Uuid::now_v7());
        let target = target.map(String::from);
        broken
            .intent(&mut tx, position, intent, "cancel", target.clone(), Some("success"), None)
            .await?;
        tx.commit().await?;
        let refused = broken.read(&pool).await;
        ensure!(matches!(refused, Err(LedgerError::Backend(_))), "{target:?}: {refused:?}");
    }
    Ok(())
}

/// A summary whose exact sum no ledger amount can hold (two sells of
/// 4.0000000000000000000000000001: 29 significant digits) refuses the read
/// instead of being rounded; one of them alone is summed exactly.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_summary_beyond_a_ledger_amount_refuses_the_read(pool: PgPool) -> Result<()> {
    let big = dec!(4.0000000000000000000000000001);
    let sell = || GenOrder {
        qty: dec!(5),
        trades: vec![trade(-30 * DAY + 10, big)],
        ..order(LegOrderKind::Sell, "FILLED", Some(valid(-10_000)))
    };
    let mut one = vec![leg(Some(month(2026, 11)), vec![entry(), sell()])];
    identify(&mut one);
    let book = Book::new(&pool).await?;
    book.write(&pool, &one).await?;
    let read = book.read(&pool).await?;
    ensure!(read[0].summarized_sold_before_b == big && read[0].orders.len() == 1, "{read:?}");
    ensure!(read[0].summarized_sold_before_b.scale() == 28, "{read:?}");
    let mut two = vec![leg(Some(month(2026, 11)), vec![entry(), sell(), sell()])];
    identify(&mut two);
    let book = Book::new(&pool).await?;
    book.write(&pool, &two).await?;
    let refused = book.read(&pool).await;
    ensure!(matches!(refused, Err(LedgerError::Backend(_))), "{refused:?}");
    Ok(())
}

/// What the fixture owner was asked, and the one answer it gives.
struct FixtureOwner {
    answer: LookupAnswer,
    asked: Mutex<Vec<BoundaryLookup>>,
}

impl BoundaryOwner for FixtureOwner {
    fn lookup(
        &self,
        request: BoundaryLookup,
    ) -> impl Future<Output = Result<LookupAnswer, Failure>> + Send {
        self.asked.lock().unwrap().push(request);
        let answer = self.answer.clone();
        async move { Ok(answer) }
    }
}

/// §C4 (b1) end to end on stored facts: a stop acknowledged before B, with
/// neither a trade from B nor Robson's cancel, holds K until the owner
/// answers; the fixture owner's answer is handed to 8p-3 as stored evidence
/// of the send. `NEW` sent by B + 2 s on a valid clock protects the leg;
/// anything else floors it at 0.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_owner_seam_answers_b1_through_a_fixture_owner(pool: PgPool) -> Result<()> {
    let stop = GenOrder {
        live: valid(-10_000),
        ..order(LegOrderKind::Stop, "NEW", None)
    };
    // The review's terms (R-hat = 0.00008): the floor of a protected leg is
    // then exactly representable (8p-3 holds K on an inexact product).
    let terms = LegTerms {
        remainder_reserve: dec!(0.00008),
        ..terms()
    };
    let mut legs = vec![GenLeg {
        terms,
        ..leg(Some(month(2026, 11)), vec![entry(), stop])
    }];
    identify(&mut legs);
    let book = Book::new(&pool).await?;
    book.write(&pool, &legs).await?;
    let (position, stop) = (legs[0].position, oid(legs[0].orders[1].id));
    let boundary = Boundary::new(k(), dec!(91.5)).unwrap();
    let facts = book.read(&pool).await?;
    ensure!(snapshot(dec!(1000), &facts, &boundary) == Err(Hold::LookupNeeded(stop)));
    let evidence = |verdict| ClockEvidence {
        sample_id: Uuid::now_v7(),
        wall_delta_ms: 2,
        mono_delta_ms: 3,
        verdict,
    };
    let answers = [
        ("NEW", 2_000, Some(evidence(ClockVerdict::Valid)), true),
        ("NEW", 1_999, Some(evidence(ClockVerdict::Valid)), false),
        ("NEW", 60_000, Some(evidence(ClockVerdict::Invalid)), false),
        ("NEW", 60_000, None, false),
        ("FILLED", 60_000, Some(evidence(ClockVerdict::Valid)), false),
    ];
    for (status, sent_ms, clock, protects) in answers {
        let sent = SeenAt { at: at(sent_ms), clock, capture: None };
        let owner = FixtureOwner {
            answer: LookupAnswer { status: status.into(), sent },
            asked: Mutex::new(vec![]),
        };
        // 8p-5's flow: the hold names the stop; the owner is asked outside any
        // transaction and its answer attached to that stop of that leg.
        let request = BoundaryLookup {
            position_id: position,
            exchange_order_id: stop,
        };
        let answer = owner.lookup(request).await.map_err(|error| anyhow::anyhow!(error))?;
        let mut facts = facts.clone();
        let order = (facts.iter_mut().find(|leg| leg.position_id == position))
            .and_then(|leg| leg.orders.iter_mut().find(|order| order.exchange_order_id == stop))
            .expect("the held stop");
        order.lookup = Some(answer.lookup());
        let (carried, _) =
            snapshot(dec!(1000), &facts, &boundary).map_err(|hold| anyhow::anyhow!("{hold:?}"))?;
        let protection = carried[0].protection.as_ref().map(|p| (p.stop, p.open_after_b));
        let wanted = protects.then_some((stop, OpenAfterB::NewAtCompletion));
        ensure!(protection == wanted, "{status} at {sent_ms}: {protection:?}");
        let floor = carried[0].floor;
        ensure!((floor > Decimal::ZERO) == protects, "{status} at {sent_ms}: floor {floor}");
        ensure!(owner.asked.lock().unwrap().as_slice() == [request]);
    }
    Ok(())
}

/// PLAN-r3 §7.2's cardinality (200 carried legs, 2,000 order intents, 10,000
/// fills): one statement on the wire per read, every read inside the 1 s
/// statement budget of the completion transaction, and a misnamed month
/// refused before any statement.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn leg_facts_are_one_statement_within_their_bound(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    let (tenant, scope) = (book.tenant, &book.scope);
    // Each leg: its entry and 9 sell-capable orders (6 seen terminal a month
    // before B, so summarized; 3 still live), 5 trades each, one attempt per
    // order intent, and two cancel intents with an attempt each.
    sqlx::raw_sql(&format!("
        SET session_replication_role = replica;
        CREATE TEMP TABLE g AS SELECT n, ('10000000-0000-0000-0000-'||lpad(n::text,12,'0'))::uuid AS p,
            ('20000000-0000-0000-0000-'||lpad(n::text,12,'0'))::uuid AS a FROM generate_series(1,200) n;
        INSERT INTO leg_routes SELECT p,'{tenant}','binance:spot','{scope}','2026-10-01T00:00:00Z' FROM g;
        INSERT INTO balance_leg_admissions (entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,reference_price,remainder_reserve,admission_charge,initial_trigger,created_at)
            SELECT a,p,'{tenant}','binance:spot','{scope}','{SYMBOL}','2026-10-01',10,10,2,5,0.00001,'base','fixture rules','2026-10-01T00:00:00Z','fixture-hash',1,1,100,0.1,1,90,'2026-10-01T00:00:00Z' FROM g;
        INSERT INTO balance_legs (position_id,tenant_id,venue_id,account_scope,symbol,admission_intent_id,decided_month,cohort_month,target_trigger,gross_qty,acquisition_cost,base_commission,base_commission_final,executable_qty,consumed_cum,created_at)
            SELECT p,'{tenant}','binance:spot','{scope}','{SYMBOL}',a,'2026-10-01','2026-10-01',90,1,100,0.001,true,0.9,0,'2026-10-01T00:00:00Z' FROM g;
        CREATE TEMP TABLE j AS SELECT g.p, g.n, k, ('30000000-0000-0000-'||lpad(g.n::text,4,'0')||'-'||lpad(k::text,12,'0'))::uuid AS i
            FROM g, generate_series(1,12) k;
        INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,cancels_order_id,params,venue_outcome,cancel_class,created_at)
            SELECT i,'{tenant}','binance:spot','{scope}',i::text,p,CASE WHEN k=1 THEN 'entry' WHEN k<=10 THEN 'protection' ELSE 'cancel' END,
                   CASE WHEN k>10 THEN (1000000000+(n-1)*10+k-9)::text END,'{{}}','success',CASE WHEN k>10 THEN 'cancelled' END,'2026-10-02T00:00:00Z' FROM j;
        INSERT INTO spot_intent_clock (attempt_id,intent_id,tenant_id,venue_id,attempt_created_at,clock_sample_id,clock_wall_delta_ms,clock_mono_delta_ms,clock_verdict)
            SELECT md5(i::text)::uuid,i,'{tenant}','binance:spot','2026-10-02T00:00:00Z',md5(p::text)::uuid,0,0,'valid' FROM j;
        INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,stop_price,status,phase,executed_cum,fills_qty,last_checked_at,live_seen_at,live_clock_sample_id,live_clock_wall_delta_ms,live_clock_mono_delta_ms,live_clock_verdict,terminal_seen_at,terminal_clock_sample_id,terminal_clock_wall_delta_ms,terminal_clock_mono_delta_ms,terminal_clock_verdict)
            SELECT 'binance:spot','{scope}','{SYMBOL}',(1000000000+(n-1)*10+k)::text,'{tenant}',i::text,i::text,'leg',i,p,
                   CASE WHEN k=1 THEN 'entry' ELSE 'stop' END,CASE WHEN k=1 THEN 'BUY' ELSE 'SELL' END,'STOP_LOSS_LIMIT',1,
                   CASE WHEN k>1 THEN 90 END,CASE WHEN k<=7 THEN 'CANCELED' ELSE 'NEW' END,CASE WHEN k<=7 THEN 'terminal' ELSE 'live' END,
                   0.05,0.05,'2026-11-03T00:00:00Z','2026-10-02T00:00:00Z',md5(p::text)::uuid,0,0,'valid',
                   CASE WHEN k<=7 THEN timestamptz '2026-10-03T00:00:00Z' END,CASE WHEN k<=7 THEN md5(p::text)::uuid END,
                   CASE WHEN k<=7 THEN 0 END,CASE WHEN k<=7 THEN 0 END,CASE WHEN k<=7 THEN 'valid' END FROM j WHERE k<=10;
        INSERT INTO order_fills SELECT 'binance:spot','{scope}','{SYMBOL}',(o.exchange_order_id::bigint*10+t)::text,o.exchange_order_id,
                   '{tenant}',100,0.01,1,0.0001,CASE o.side WHEN 'BUY' THEN 'BTC' ELSE 'USDT' END,'2026-10-02T12:00:00Z','2026-10-03T00:00:00Z'
            FROM venue_orders o, generate_series(1,5) t WHERE o.tenant_id='{tenant}';
        RESET session_replication_role;
        ANALYZE balance_legs, balance_leg_admissions, dust_ledger, venue_orders, order_fills, order_intents, spot_intent_clock;
    ")).execute(&pool).await?;
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM balance_legs),(SELECT count(*) FROM order_intents),(SELECT count(*) FROM order_fills)")
        .fetch_one(&pool).await?;
    ensure!(counts.0 >= 200 && counts.1 >= 2_000 && counts.2 >= 10_000, "{counts:?}");

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
    let venue = VenueId::new(VenueId::BINANCE_SPOT);
    proxy.enable_exact_query_trace(32);
    let misnamed =
        read_leg_facts(&mut connection, tenant, &venue, month(2026, 12).succ_opt().unwrap()).await;
    ensure!(
        matches!(misnamed, Err(LedgerError::InvalidEvidence(_))) && proxy.query_trace().is_empty()
    );
    let mut times = Vec::new();
    for _ in 0..21 {
        let started = Instant::now();
        let legs = read_leg_facts(&mut connection, tenant, &venue, k()).await?;
        times.push(started.elapsed());
        let shape = (
            legs.len(),
            legs[0].orders.len(),
            legs[0].orders[0].fills.len(),
            legs[0].cancels.len(),
        );
        ensure!(shape == (200, 4, 5, 2), "{shape:?}");
        ensure!(
            legs[0].summarized_sold_before_b == dec!(0.30),
            "{:?}",
            legs[0].summarized_sold_before_b
        );
    }
    let trace = proxy.query_trace();
    ensure!(
        trace.len() == 21 && trace.iter().all(|sql| sql == LEG_FACTS),
        "one statement per read: {trace:?}"
    );
    times.sort();
    ensure!(times[20] < Duration::from_millis(1000), "slowest read {:?}", times[20]);
    let plan: Vec<String> = sqlx::query_scalar(&format!("EXPLAIN (ANALYZE, BUFFERS) {LEG_FACTS}"))
        .bind(tenant)
        .bind(venue.as_str())
        .bind(at(0))
        .fetch_all(&pool)
        .await?;
    eprintln!(
        "LEG_FACTS_BOUND median_us={} max_us={} legs={} intents={} fills={}\n{}",
        times[10].as_micros(),
        times[20].as_micros(),
        counts.0,
        counts.1,
        counts.2,
        plan.join("\n")
    );
    Ok(())
}

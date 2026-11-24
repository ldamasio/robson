//! 8l-3a proofs on PostgreSQL (PLAN-r4 §8): `E(t)` against an independent
//! integer oracle, the anchor at exactly B, an anchor without `as_of`, the
//! (A)/(R) impairment spans against an independent oracle over generated
//! anchor chains, the effective minimum and V5 with dirty sightings, V6 and
//! the withdrawal fence, and the statement's one round trip and bound at
//! PLAN-r4 §7.2's cardinality. Fixture rows are written directly: their
//! writers are 8l-3b's, 8l-5's and 8m's.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use anyhow::{ensure, Result};
use chrono::TimeDelta;
use proptest::{
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngSeed, TestRunner},
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use sqlx::{postgres::PgPoolOptions, PgPool};

use super::*;

const SYMBOL: &str = "BTCUSDT"; // A labelled fixture pair, never a policy restriction.
pub(super) const ASSETS: &[CommissionAssets<'static>] = &[CommissionAssets {
    symbol: SYMBOL,
    base: "BTC",
    quote: "USDT",
}];
static IDS: AtomicU64 = AtomicU64::new(1);

/// `ms` milliseconds after 2026-11-10T12:00:00Z.
pub(super) fn at(ms: i64) -> DateTime<Utc> {
    "2026-11-10T12:00:00Z".parse::<DateTime<Utc>>().expect("fixture time")
        + TimeDelta::milliseconds(ms)
}

pub(super) fn utc(text: &str) -> DateTime<Utc> {
    text.parse().expect("RFC 3339 fixture time")
}

pub(super) fn micro() -> TimeDelta {
    TimeDelta::microseconds(1)
}

/// One order and its one fill.
pub(super) struct Fill<'a> {
    pub(super) leg: bool,
    pub(super) buy: bool,
    pub(super) qty: Decimal,
    pub(super) price: Decimal,
    pub(super) fee: Decimal,
    pub(super) asset: &'a str,
    pub(super) time: DateTime<Utc>,
}

impl Fill<'static> {
    /// A discovered order's fill: no leg, no `E` flow, but late evidence.
    pub(super) fn foreign(time: DateTime<Utc>) -> Self {
        Self {
            leg: false,
            buy: true,
            qty: dec!(0.1),
            price: dec!(100),
            fee: dec!(0),
            asset: "BTC",
            time,
        }
    }
}

/// One tenant's balance-account venue: its version row, a process epoch, a
/// clock sample, and one leg for leg-owned orders.
pub(super) struct Book {
    pub(super) tenant: Uuid,
    pub(super) scope: String,
    pub(super) sample: Uuid,
    pub(super) epoch: Uuid,
    pub(super) leg: Uuid,
}

impl Book {
    pub(super) async fn new(pool: &PgPool) -> Result<Self> {
        let (tenant, epoch, sample) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let (leg, admission, scope) = (Uuid::now_v7(), Uuid::now_v7(), format!("cash-{tenant}"));
        sqlx::query("INSERT INTO accounting_state (tenant_id,venue_id,budget_model) VALUES ($1,'binance:spot','net_from_start_non_expanding_v1')")
            .bind(tenant).execute(pool).await?;
        sqlx::query("INSERT INTO process_epochs (epoch_id,tenant_id,binary_version) VALUES ($1,$2,'8l-3a-fixture')")
            .bind(epoch).bind(tenant).execute(pool).await?;
        sqlx::query("INSERT INTO clock_samples (sample_id,tenant_id,venue_id,process_epoch,clock_domain,send_wall_seconds,send_wall_nanoseconds,send_tick_before,send_tick_after,receive_wall_seconds,receive_wall_nanoseconds,receive_tick_before,receive_tick_after,server_ms,theta2_ns,r_low_ns,r_high_ns,u2_ns,numeric_verdict,numeric_failures,algorithm_version,recorded_at) VALUES ($1,$2,'binance:spot',$3,ROW('boottime',$4::uuid,0,0)::budget_clock_domain_v38,1794614400,0,0,0,1794614400,0,0,0,0,-3589228800000000000,0,0,3589228800200000000,'invalid',1,1,now())")
            .bind(sample).bind(tenant).bind(epoch).bind(Uuid::now_v7())
            .execute(pool).await?;
        let created = utc("2026-09-01T00:00:00Z");
        sqlx::query("INSERT INTO leg_routes VALUES ($1,$2,'binance:spot',$3,$4)")
            .bind(leg)
            .bind(tenant)
            .bind(&scope)
            .bind(created)
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO balance_leg_admissions (entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,reference_price,remainder_reserve,admission_charge,initial_trigger,created_at)
            VALUES ($1,$2,$3,'binance:spot',$4,$5,'2026-09-01',10,10,2,5,0.00001,'base','fixture rules',$6,'fixture-hash',1,1,100,0.1,1,90,$6)")
            .bind(admission).bind(leg).bind(tenant).bind(&scope).bind(SYMBOL).bind(created)
            .execute(pool).await?;
        sqlx::query("INSERT INTO balance_legs (position_id,tenant_id,venue_id,account_scope,symbol,admission_intent_id,decided_month,target_trigger,gross_qty,acquisition_cost,base_commission,base_commission_final,executable_qty,consumed_cum,created_at)
            VALUES ($1,$2,'binance:spot',$3,$4,$5,'2026-09-01',90,1,100,0.001,true,0.9,0,$6)")
            .bind(leg).bind(tenant).bind(&scope).bind(SYMBOL).bind(admission).bind(created)
            .execute(pool).await?;
        Ok(Self { tenant, scope, sample, epoch, leg })
    }

    /// An observed anchor whose O2 interval is [as_of, as_of + 500 ms], on
    /// the hull [hull_start, as_of + 500 ms].
    pub(super) async fn anchor(
        &self,
        pool: &PgPool,
        as_of: DateTime<Utc>,
        hull_start: DateTime<Utc>,
        quote: Decimal,
    ) -> Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO cash_anchors (anchor_id,tenant_id,venue_id,kind,quote,as_of,send_at,receive_at,u_ns,sample_id,epoch_id,interval_start,interval_end,depends_start,depends_end)
            VALUES ($1,$2,'binance:spot','reanchor',$3,$4,$4+interval '150 ms',$4+interval '350 ms',150000000,$5,$6,$4,$4+interval '500 ms',$7,$4+interval '500 ms')")
            .bind(id).bind(self.tenant).bind(quote).bind(as_of).bind(self.sample).bind(self.epoch).bind(hull_start)
            .execute(pool).await?;
        Ok(id)
    }

    /// A cutover anchor before completion: no `as_of` and no observation.
    pub(super) async fn pending(&self, pool: &PgPool, quote: Decimal) -> Result<Uuid> {
        let (id, cutover) = (Uuid::now_v7(), Uuid::now_v7());
        // Its cutover run (M56's foreign key).
        sqlx::query(include_str!("../../../tests/fixtures/cutover_run.sql"))
            .bind("binance:spot")
            .bind(self.tenant)
            .bind(cutover)
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO cash_anchors (anchor_id,tenant_id,venue_id,kind,cutover_id,quote) VALUES ($1,$2,'binance:spot','cutover',$3,$4)")
            .bind(id).bind(self.tenant).bind(cutover).bind(quote).execute(pool).await?;
        Ok(id)
    }

    pub(super) async fn fill(&self, pool: &PgPool, fill: &Fill<'_>) -> Result<()> {
        let id = IDS.fetch_add(1, Ordering::Relaxed).to_string();
        let (owner, leg) = if fill.leg {
            ("leg", Some(self.leg))
        } else {
            ("discovered", None)
        };
        let (side, kind) = if fill.buy {
            ("BUY", "entry")
        } else {
            ("SELL", "sell")
        };
        sqlx::query("INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,status,phase,executed_cum,fills_qty,last_checked_at,live_seen_at,terminal_seen_at)
            VALUES ('binance:spot',$1,$2,$3,$4,$3,$3,$5,$6,$7,$8,$9,'MARKET',$10,'FILLED','terminal',$10,$10,$11,$11,$11)")
            .bind(&self.scope).bind(SYMBOL).bind(&id).bind(self.tenant).bind(owner)
            .bind(leg.map(|_| Uuid::now_v7())).bind(leg).bind(leg.map(|_| kind)).bind(side)
            .bind(fill.qty).bind(fill.time)
            .execute(pool).await?;
        sqlx::query("INSERT INTO order_fills VALUES ('binance:spot',$1,$2,$3,$3,$4,$5,$6,$5*$6,$7,$8,$9,$9)")
            .bind(&self.scope).bind(SYMBOL).bind(&id).bind(self.tenant).bind(fill.price).bind(fill.qty)
            .bind(fill.fee).bind(fill.asset).bind(fill.time)
            .execute(pool).await?;
        Ok(())
    }

    /// An attested re-anchor from `old` to `new` (whose cut is `new_as_of`):
    /// Δ = 0, dating copy [dating, dating + 500 ms], and the sightings of
    /// `resolved` it used, less `excluded`.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn reanchor(
        &self,
        pool: &PgPool,
        old: Uuid,
        new: Uuid,
        new_as_of: DateTime<Utc>,
        dating: DateTime<Utc>,
        resolved: Option<Uuid>,
        excluded: &[i32],
    ) -> Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO cash_reanchors (reanchor_id,tenant_id,venue_id,operator_subject,reason,
                o1_send_at,o1_receive_at,o1_u_ns,o1_sample_id,o1_epoch_id,o1_interval_start,o1_interval_end,o1_total,
                o2_send_at,o2_receive_at,o2_u_ns,o2_sample_id,o2_epoch_id,o2_interval_start,o2_interval_end,o2_total,
                mode,checked_version,old_anchor_id,new_anchor_id,dating_cut,dating_interval_start,dating_interval_end,
                delta,min_delta,withdrawal,contribution,resolved_discrepancy_id,excluded_sighting_seqs)
            VALUES ($1,$2,'binance:spot','operator-fixture','fixture re-anchor',
                $3-interval '9850 ms',$3-interval '9650 ms',150000000,$4,$5,$3-interval '10 s',$3-interval '9500 ms',1000,
                $3+interval '150 ms',$3+interval '350 ms',150000000,$4,$5,$3,$3+interval '500 ms',1000,
                'attested',1,$6,$7,$8,$8,$8+interval '500 ms',0,0,0,0,$9,$10)")
            .bind(id).bind(self.tenant).bind(new_as_of).bind(self.sample).bind(self.epoch).bind(old).bind(new)
            .bind(dating).bind(resolved).bind(excluded)
            .execute(pool).await?;
        Ok(id)
    }

    pub(super) async fn flow(
        &self,
        pool: &PgPool,
        reanchor: Uuid,
        withdrawal: bool,
        amount: Decimal,
        date: DateTime<Utc>,
    ) -> Result<()> {
        let (direction, rule) = if withdrawal {
            ("withdrawal", "checkpoint_cut")
        } else {
            ("contribution", "o1_receipt_plus_u")
        };
        sqlx::query("INSERT INTO cash_attested_flows (tenant_id,venue_id,economic_date,flow_id,reanchor_id,direction,amount,date_rule,attributed_month,reanchor_mode)
            VALUES ($1,'binance:spot',$2,$3,$4,$5,$6,$7,date_trunc('month',$2 AT TIME ZONE 'UTC')::date,'attested')")
            .bind(self.tenant).bind(date).bind(Uuid::now_v7()).bind(reanchor).bind(direction).bind(amount).bind(rule)
            .execute(pool).await?;
        Ok(())
    }

    /// A discrepancy in `state` whose first sighting (seq 1) is `first` at
    /// cut `first_cut`, with the checkpoint copy at `checkpoint`; then its
    /// further sightings, seq 2 onward, each [cut, cut + 500 ms].
    pub(super) async fn discrepancy(
        &self,
        pool: &PgPool,
        state: &str,
        checkpoint: DateTime<Utc>,
        sightings: &[(DateTime<Utc>, Decimal)],
    ) -> Result<Uuid> {
        let id = Uuid::now_v7();
        let (first_cut, first) = sightings[0];
        let direction = if first < Decimal::ZERO {
            "missing"
        } else {
            "extra"
        };
        sqlx::query("INSERT INTO cash_discrepancies (discrepancy_id,tenant_id,venue_id,direction,state,first_send_at,first_receive_at,first_u_ns,first_sample_id,first_epoch_id,first_interval_start,first_interval_end,first_delta,checkpoint_cut,checkpoint_interval_start,checkpoint_interval_end,opened_version,last_seen_at,min_delta,closed_at)
            VALUES ($1,$2,'binance:spot',$3,$4,$5+interval '150 ms',$5+interval '350 ms',150000000,$6,$7,$5,$5+interval '500 ms',$8,$9,$9,$9+interval '500 ms',1,$5+interval '350 ms',$8,CASE WHEN $4<>'open' THEN $5 END)")
            .bind(id).bind(self.tenant).bind(direction).bind(state).bind(first_cut).bind(self.sample).bind(self.epoch)
            .bind(first).bind(checkpoint)
            .execute(pool).await?;
        for (seq, (cut, delta)) in (1..).zip(sightings) {
            sqlx::query("INSERT INTO cash_sightings VALUES ($1,$2,$3,'binance:spot',$4+interval '150 ms',$4+interval '350 ms',150000000,$5,$6,$4,$4+interval '500 ms',$7)")
                .bind(id).bind(seq).bind(self.tenant).bind(*cut).bind(self.sample).bind(self.epoch).bind(*delta)
                .execute(pool).await?;
        }
        Ok(id)
    }

    pub(super) async fn state(
        &self,
        pool: &PgPool,
        t: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<CashState> {
        let venue = VenueId::new(VenueId::BINANCE_SPOT);
        let query = CashQuery {
            tenant_id: self.tenant,
            venue: &venue,
            t,
            now,
            assets: ASSETS,
        };
        Ok(read_cash_state(&mut *pool.acquire().await?, &query).await?)
    }

    pub(super) async fn expected(&self, pool: &PgPool, t: DateTime<Utc>) -> Result<ExpectedQuote> {
        Ok(self.state(pool, t, t).await?.expected)
    }
}

/// v16:1930: an anchor at exactly B does not count for B and does for the
/// next boundary; a flow at exactly an anchor's `as_of` is after it. (A fill
/// at an anchor's `as_of` lies in its own interval: late evidence, below.)
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn anchor_at_exactly_b_counts_from_the_next_boundary(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    let (b, next) = (utc("2026-12-01T00:00:00Z"), utc("2027-01-01T00:00:00Z"));
    let early = b - TimeDelta::hours(1);
    let first = book.anchor(&pool, early, early - TimeDelta::seconds(10), dec!(1000)).await?;
    let second = TimeDelta::seconds(1);
    let sold = Fill {
        leg: true,
        buy: false,
        fee: dec!(0.25),
        asset: "USDT",
        ..Fill::foreign(early + second)
    };
    book.fill(&pool, &sold).await?;
    let at_b = book.anchor(&pool, b, b - TimeDelta::seconds(10), dec!(1500)).await?;
    book.fill(&pool, &Fill {
        leg: true,
        qty: dec!(2),
        ..Fill::foreign(b + second)
    })
    .await?;
    let r = book.reanchor(&pool, first, at_b, b, early, None, &[]).await?;
    book.flow(&pool, r, true, dec!(3), early).await?;
    book.flow(&pool, r, false, dec!(2), b).await?;
    // E(B) rests on the earlier anchor: the sale (10 - 0.25) and the
    // withdrawal dated at that anchor's as_of count; the contribution dated
    // at B and the buy after B do not.
    let quote_b = book.expected(&pool, b).await?;
    ensure!(quote_b.anchor == Some((first, early)) && !quote_b.impaired, "{quote_b:?}");
    ensure!(quote_b.value == Some(dec!(1006.75)), "{quote_b:?}");
    // Just after B and at the next boundary, the anchor at B, with the
    // contribution dated at its as_of, then the buy (2 * 100).
    let just_after = book.expected(&pool, b + micro()).await?;
    ensure!(
        just_after.anchor == Some((at_b, b)) && just_after.value == Some(dec!(1502)),
        "{just_after:?}"
    );
    let quote_next = book.expected(&pool, next).await?;
    ensure!(
        quote_next.anchor == Some((at_b, b)) && quote_next.value == Some(dec!(1302)),
        "{quote_next:?}"
    );
    // A fill booked later at exactly B lies in the anchor's own interval:
    // its hull is dirty, so E is impaired after it (A) and, through the
    // re-anchor that wrote it, from the replaced anchor's as_of on (R).
    book.fill(&pool, &Fill::foreign(b)).await?;
    for t in [early + micro(), b, next] {
        ensure!(book.expected(&pool, t).await?.impaired, "{t}");
    }
    Ok(())
}

/// The cutover anchor's `as_of` is set at completion only: until then it
/// never counts (ZB test 9); with no leg and no flow, `quote_B` is the
/// observed anchor's quote (ZB test 4).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn anchor_without_as_of_never_counts(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    let b = utc("2026-12-01T00:00:00Z");
    let cutover = book.pending(&pool, dec!(7777)).await?;
    let alone = book.expected(&pool, b).await?;
    ensure!(
        alone
            == ExpectedQuote {
                anchor: None,
                impaired: false,
                value: None
            },
        "{alone:?}"
    );
    let as_of = b - TimeDelta::days(3);
    let observed = book.anchor(&pool, as_of, as_of - TimeDelta::seconds(10), dec!(2000)).await?;
    book.pending(&pool, dec!(9999)).await?;
    let quote_b = book.expected(&pool, b).await?;
    ensure!(
        quote_b
            == ExpectedQuote {
                anchor: Some((observed, as_of)),
                impaired: false,
                value: Some(dec!(2000))
            },
        "{quote_b:?}"
    );
    // Completion sets the cutover anchor's observation once; from then on it
    // is an observed anchor like any other.
    let completed = as_of + TimeDelta::hours(1);
    sqlx::query("UPDATE cash_anchors SET as_of=$2,send_at=$2+interval '150 ms',receive_at=$2+interval '350 ms',u_ns=150000000,sample_id=$3,epoch_id=$4,interval_start=$2,interval_end=$2+interval '500 ms',depends_start=$2-interval '10 s',depends_end=$2+interval '500 ms' WHERE anchor_id=$1")
        .bind(cutover).bind(completed).bind(book.sample).bind(book.epoch).execute(&pool).await?;
    let completed_b = book.expected(&pool, b).await?;
    ensure!(completed_b.anchor == Some((cutover, completed)), "{completed_b:?}");
    ensure!(completed_b.value == Some(dec!(7777)), "{completed_b:?}");
    let still = book.expected(&pool, completed).await?;
    ensure!(still.anchor == Some((observed, as_of)), "at its own as_of it is not yet used");
    Ok(())
}

/// PLAN-r4 §6.10 (A): a fill booked later inside an anchor's O1..O2 hull
/// impairs `E(t)` for t in (that anchor's as_of, the next clean-hull
/// anchor's as_of], through any later anchor whose hull is dirty too.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn late_fill_in_an_anchor_hull_impairs_until_the_next_clean_anchor(
    pool: PgPool,
) -> Result<()> {
    let book = Book::new(&pool).await?;
    let hull = TimeDelta::seconds(20);
    let cuts = [at(0), at(100_000), at(200_000), at(300_000)];
    let mut anchors = Vec::new();
    for (cut, quote) in cuts.iter().zip([dec!(1000), dec!(1100), dec!(1200), dec!(1300)]) {
        anchors.push(book.anchor(&pool, *cut, *cut - hull, quote).await?);
    }
    let defined = |e: ExpectedQuote, anchor: usize, quote: Decimal| {
        e.anchor == Some((anchors[anchor], cuts[anchor])) && !e.impaired && e.value == Some(quote)
    };
    let points = |cut: DateTime<Utc>| [cut - micro(), cut, cut + micro()];
    for point in cuts.iter().flat_map(|cut| points(*cut)) {
        ensure!(!book.expected(&pool, point).await?.impaired, "clean chain at {point}");
    }
    // Late evidence inside A1's hull, before its O2 cut (O1's side).
    book.fill(&pool, &Fill::foreign(cuts[1] - TimeDelta::seconds(5))).await?;
    let read = |t| book.expected(&pool, t);
    ensure!(defined(read(cuts[1]).await?, 0, dec!(1000)), "A1's own cut still uses A0");
    for t in [cuts[1] + micro(), cuts[2] - micro(), cuts[2]] {
        let e = read(t).await?;
        ensure!(e.impaired && e.value.is_none() && e.anchor.is_some(), "{t}: {e:?}");
    }
    ensure!(defined(read(cuts[2] + micro()).await?, 2, dec!(1200)), "A2 is clean");
    // A2's hull turns dirty too: the span runs on to the next clean anchor.
    book.fill(&pool, &Fill::foreign(cuts[2] + TimeDelta::milliseconds(500))).await?;
    for t in [cuts[2] + micro(), cuts[3] - micro(), cuts[3]] {
        ensure!(read(t).await?.impaired, "{t}");
    }
    ensure!(defined(read(cuts[3] + micro()).await?, 3, dec!(1300)), "A3 is clean");
    let state = book.state(&pool, cuts[3], cuts[3] + micro()).await?;
    ensure!(state.expected.impaired && !state.impaired_now, "{state:?}");
    ensure!(book.state(&pool, cuts[3], cuts[2] + micro()).await?.impaired_now);
    Ok(())
}

/// A dirty sighting is a voided observation: excluded from the effective
/// minimum while the clean ones keep the missing-cash fence (PLAN-r4 §6.4).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn dirty_sighting_is_excluded_from_the_effective_minimum(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    book.anchor(&pool, at(0), at(-10_000), dec!(1000)).await?;
    let sightings = [
        (at(60_000), dec!(-100)),
        (at(120_000), dec!(0.004)),
        (at(180_000), dec!(-150)),
    ];
    book.discrepancy(&pool, "open", at(0), &sightings).await?;
    let now = at(240_000);
    let before = book.state(&pool, now, now).await?;
    ensure!(
        before.open_missing && before.effective_minimum == Some(dec!(-150)),
        "{before:?}"
    );
    // A fill booked later inside the third sighting's interval.
    book.fill(&pool, &Fill::foreign(at(180_250))).await?;
    let after = book.state(&pool, now, now).await?;
    ensure!(after.open_missing && after.effective_minimum == Some(dec!(-100)), "{after:?}");
    ensure!(
        !after.expected.impaired && after.expected.value == Some(dec!(1000)),
        "{after:?}"
    );
    Ok(())
}

/// A discrepancy whose every sighting of its direction is dirty has no
/// surviving evidence: it no longer fences (V5, V6), and the next cash write
/// voids it durably (8l-3b).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn discrepancy_without_surviving_evidence_is_void(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    let b = utc("2026-12-01T00:00:00Z");
    book.anchor(&pool, at(0), at(-10_000), dec!(1000)).await?;
    let sightings = [(at(60_000), dec!(-100)), (at(120_000), dec!(0))];
    book.discrepancy(&pool, "open", at(0), &sightings).await?;
    let fenced = book.state(&pool, b, at(240_000)).await?;
    ensure!(fenced.open_missing && fenced.missing_before_t, "{fenced:?}");
    book.fill(&pool, &Fill::foreign(at(60_000))).await?;
    let void = book.state(&pool, b, at(240_000)).await?;
    ensure!(!void.open_missing && !void.missing_before_t, "{void:?}");
    ensure!(void.effective_minimum == Some(dec!(0)), "the clean sighting stays: {void:?}");
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cash_discrepancies WHERE tenant_id=$1 AND state='open'",
    )
    .bind(book.tenant)
    .fetch_one(&pool)
    .await?;
    ensure!(open == 1, "reading writes nothing");
    Ok(())
}

/// V5's clause, V6 at t and the withdrawal fence at now's UTC month.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cash_predicates_read_v5_v6_and_the_withdrawal_fence(pool: PgPool) -> Result<()> {
    let (november, december) = (utc("2026-11-20T00:00:00Z"), utc("2026-12-02T00:00:00Z"));
    let b = utc("2026-12-01T00:00:00Z");
    let book = Book::new(&pool).await?;
    let first = book.anchor(&pool, at(0), at(-10_000), dec!(1000)).await?;
    let none = book.state(&pool, b, november).await?;
    ensure!(
        !none.open_missing
            && !none.missing_before_t
            && !none.withdrawal_fence
            && !none.impaired_now,
        "{none:?}"
    );
    ensure!(none.effective_minimum.is_none());
    // Extra cash only alarms.
    let extra = Book::new(&pool).await?;
    extra.anchor(&pool, at(0), at(-10_000), dec!(1000)).await?;
    extra.discrepancy(&pool, "open", at(0), &[(at(60_000), dec!(50))]).await?;
    let state = extra.state(&pool, b, november).await?;
    ensure!(!state.open_missing && state.effective_minimum == Some(dec!(50)), "{state:?}");
    // Missing cash whose checkpoint copy is at `checkpoint`: V6 holds only
    // for a t strictly after it.
    let checkpoint = at(30_000);
    book.discrepancy(&pool, "open", checkpoint, &[(at(60_000), dec!(-40))]).await?;
    for (t, before) in [(checkpoint, false), (checkpoint + micro(), true), (b, true)] {
        let state = book.state(&pool, t, november).await?;
        ensure!(state.open_missing && state.missing_before_t == before, "{t}: {state:?}");
    }
    // A withdrawal attributed to November fences November only; a
    // contribution never fences.
    let second = book.anchor(&pool, at(600_000), at(590_000), dec!(960)).await?;
    let r = book.reanchor(&pool, first, second, at(600_000), checkpoint, None, &[]).await?;
    book.flow(&pool, r, false, dec!(5), at(500_000)).await?;
    ensure!(!book.state(&pool, b, november).await?.withdrawal_fence, "a contribution");
    book.flow(&pool, r, true, dec!(45), checkpoint).await?;
    ensure!(book.state(&pool, b, november).await?.withdrawal_fence);
    ensure!(!book.state(&pool, b, december).await?.withdrawal_fence);
    let resolved = book.state(&pool, b, november).await?;
    ensure!(resolved.expected.value == Some(dec!(960)), "{resolved:?}");
    Ok(())
}

// ---- E(t) against an independent integer oracle ----

/// One generated fill: amounts in units of 0.001 (qty), 0.01 (price) and
/// 0.000001 (fee); `odd` pays its commission in an unexpected asset.
#[derive(Debug, Clone)]
struct GenFill {
    leg: bool,
    buy: bool,
    qty: u32,
    price: u32,
    fee: u32,
    odd: bool,
    slot: u32,
}

/// A flow's amount in cents and its slot, when present.
type GenFlow = Option<(u32, u32)>;

#[derive(Debug, Clone)]
struct ECase {
    /// Distinct anchor slots (minutes) with their quotes in cents.
    anchors: Vec<(u32, u32)>,
    pending: bool,
    fills: Vec<GenFill>,
    /// Per re-anchor between consecutive anchors: a withdrawal and a
    /// contribution.
    flows: Vec<(GenFlow, GenFlow)>,
}

/// Anchors sit on whole minutes with hulls [cut - 2 s, cut + 500 ms]; fills
/// and flows sit at 15 s steps plus 7.25 s, so no fill makes a hull dirty
/// and every interval point can be asked exactly.
fn anchor_time(slot: u32) -> DateTime<Utc> {
    at(i64::from(slot) * 60_000)
}

fn event_time(slot: u32) -> DateTime<Utc> {
    at(i64::from(slot) * 15_000 + 7_250)
}

prop_compose! {
    fn gen_fill()(leg in any::<bool>(), buy in any::<bool>(), qty in 1..5_000u32, price in 1..10_000_000u32,
                  fee in 0..2_000_000u32, odd in prop::bool::weighted(0.1), slot in 0..40u32) -> GenFill {
        GenFill { leg, buy, qty, price, fee, odd, slot }
    }
}

prop_compose! {
    fn gen_flow()(present in any::<bool>(), cents in 1..1_000_000u32, slot in 0..40u32) -> GenFlow {
        present.then_some((cents, slot))
    }
}

prop_compose! {
    fn gen_e_case()(slots in prop::sample::subsequence((0..10u32).collect::<Vec<_>>(), 1..5),
                    quotes in prop::collection::vec(0..100_000_000u32, 4),
                    pending in any::<bool>(),
                    fills in prop::collection::vec(gen_fill(), 0..9),
                    flows in prop::collection::vec((gen_flow(), gen_flow()), 3)) -> ECase {
        let anchors = slots.iter().zip(quotes).map(|(slot, quote)| (*slot, quote)).collect::<Vec<_>>();
        let flows = flows.into_iter().take(anchors.len() - 1).collect();
        ECase { anchors, pending, fills, flows }
    }
}

/// The oracle, in integer micro-units of the quote asset: the anchor index
/// and E(t), or `None` for the value when a counted fill is odd.
fn e_oracle(case: &ECase, t: DateTime<Utc>) -> (Option<usize>, Option<i128>) {
    let Some(index) = (0..case.anchors.len())
        .filter(|i| anchor_time(case.anchors[*i].0) < t)
        .max_by_key(|i| case.anchors[*i].0)
    else {
        return (None, None);
    };
    let from = anchor_time(case.anchors[index].0);
    let within = |time: DateTime<Utc>| from <= time && time < t;
    let mut total = i128::from(case.anchors[index].1) * 10_000;
    for fill in case.fills.iter().filter(|f| f.leg && within(event_time(f.slot))) {
        if fill.odd {
            return (Some(index), None);
        }
        let notional = i128::from(fill.qty) * i128::from(fill.price) * 10;
        total += if fill.buy {
            -notional
        } else {
            notional - i128::from(fill.fee)
        };
    }
    for (withdrawal, contribution) in &case.flows {
        for (sign, flow) in [(-1, withdrawal), (1, contribution)] {
            if let Some((cents, slot)) = flow {
                if within(event_time(*slot)) {
                    total += sign * i128::from(*cents) * 10_000;
                }
            }
        }
    }
    (Some(index), Some(total))
}

async fn write_e_case(pool: &PgPool, case: &ECase) -> Result<(Book, Vec<Uuid>)> {
    let book = Book::new(pool).await?;
    if case.pending {
        book.pending(pool, dec!(123.45)).await?;
    }
    let mut ids = Vec::new();
    for (slot, cents) in &case.anchors {
        let cut = anchor_time(*slot);
        ids.push(
            book.anchor(pool, cut, cut - TimeDelta::seconds(2), Decimal::new((*cents).into(), 2))
                .await?,
        );
    }
    let mut order: Vec<usize> = (0..ids.len()).collect();
    order.sort_by_key(|i| case.anchors[*i].0);
    for (pair, (withdrawal, contribution)) in order.windows(2).zip(&case.flows) {
        let (old, new) = (pair[0], pair[1]);
        let (old_cut, new_cut) =
            (anchor_time(case.anchors[old].0), anchor_time(case.anchors[new].0));
        let r = book.reanchor(pool, ids[old], ids[new], new_cut, old_cut, None, &[]).await?;
        for (is_withdrawal, flow) in [(true, withdrawal), (false, contribution)] {
            if let Some((cents, slot)) = flow {
                book.flow(
                    pool,
                    r,
                    is_withdrawal,
                    Decimal::new((*cents).into(), 2),
                    event_time(*slot),
                )
                .await?;
            }
        }
    }
    for fill in &case.fills {
        let asset = match (fill.odd, fill.buy) {
            (true, _) => "BNB",
            (false, true) => "BTC",
            (false, false) => "USDT",
        };
        book.fill(pool, &Fill {
            leg: fill.leg,
            buy: fill.buy,
            qty: Decimal::new(fill.qty.into(), 3),
            price: Decimal::new(fill.price.into(), 2),
            fee: Decimal::new(fill.fee.into(), 6),
            asset,
            time: event_time(fill.slot),
        })
        .await?;
    }
    Ok((book, ids))
}

fn micro_units(value: Decimal) -> Option<i128> {
    let scaled = value * Decimal::from(1_000_000);
    scaled.fract().is_zero().then(|| scaled.to_i128()).flatten()
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn expected_quote_matches_an_integer_oracle(pool: PgPool) -> Result<()> {
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0x81_3a),
        failure_persistence: None,
        ..Config::default()
    });
    let (mut reads, mut defined, mut odd, mut counted) = (0, 0, 0, 0);
    for _ in 0..40 {
        let case = gen_e_case()
            .new_tree(&mut runner)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .current();
        let (book, ids) = write_e_case(&pool, &case).await?;
        let mut points = vec![at(-1), at(11 * 60_000)];
        for (slot, _) in &case.anchors {
            points.extend([
                anchor_time(*slot) - micro(),
                anchor_time(*slot),
                anchor_time(*slot) + micro(),
            ]);
        }
        for slot in case
            .fills
            .iter()
            .map(|f| f.slot)
            .chain(case.flows.iter().flat_map(|(w, c)| [w, c]).flatten().map(|(_, slot)| *slot))
        {
            points.extend([event_time(slot), event_time(slot) + micro()]);
        }
        for t in points {
            let e = book.expected(&pool, t).await?;
            let (index, value) = e_oracle(&case, t);
            let anchor = index.map(|i| (ids[i], anchor_time(case.anchors[i].0)));
            ensure!(e.anchor == anchor && !e.impaired, "{case:?} at {t}: {e:?}");
            ensure!(
                e.value.map(micro_units) == value.map(Some),
                "{case:?} at {t}: {e:?} vs {value:?}"
            );
            reads += 1;
            defined += usize::from(value.is_some());
            odd += usize::from(index.is_some() && value.is_none());
        }
        counted += case.fills.iter().filter(|f| f.leg).count();
    }
    ensure!(defined > 100 && odd > 0 && counted > 40, "coverage {defined} {odd} {counted}");
    eprintln!("E_ORACLE cases=40 reads={reads} defined={defined} odd={odd} leg_fills={counted}");
    Ok(())
}

// ---- the (A)/(R) spans against an independent oracle ----

#[derive(Debug, Clone)]
struct SpanAnchor {
    /// Hull start, seconds before the cut.
    hull: u32,
    /// The re-anchor that wrote it (from the second anchor on): its dating
    /// copy, seconds before the cut, and its resolved discrepancy's
    /// sightings, seconds before the cut, each excluded or not.
    dating: u32,
    sightings: Vec<(u32, bool)>,
}

#[derive(Debug, Clone)]
struct SpanCase {
    anchors: Vec<SpanAnchor>,
    /// Late fills: an interval of the chain (by index) and where in it.
    fills: Vec<(usize, u8)>,
}

prop_compose! {
    fn gen_span_anchor()(hull in 1..40u32, dating in 1..150u32,
                         sightings in prop::collection::vec((1..90u32, prop::bool::weighted(0.3)), 0..4)) -> SpanAnchor {
        SpanAnchor { hull, dating, sightings }
    }
}

prop_compose! {
    fn gen_span_case()(anchors in prop::collection::vec(gen_span_anchor(), 2..6),
                       fills in prop::collection::vec((0..64usize, 0..5u8), 0..5)) -> SpanCase {
        SpanCase { anchors, fills }
    }
}

fn span_cut(k: usize) -> DateTime<Utc> {
    at((k as i64 + 1) * 100_000)
}

/// Every interval the chain rests on: (lo, hi) as stored.
fn intervals(case: &SpanCase) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut out = Vec::new();
    for (k, a) in case.anchors.iter().enumerate() {
        let cut = span_cut(k);
        let end = cut + TimeDelta::milliseconds(500);
        out.push((cut - TimeDelta::seconds(a.hull.into()), end));
        if k > 0 {
            let dating = cut - TimeDelta::seconds(a.dating.into());
            out.push((dating, dating + TimeDelta::milliseconds(500)));
            for (back, _) in &a.sightings {
                let at = cut - TimeDelta::seconds((*back).into());
                out.push((at, at + TimeDelta::milliseconds(500)));
            }
        }
    }
    out
}

fn fill_times(case: &SpanCase) -> Vec<DateTime<Utc>> {
    let all = intervals(case);
    case.fills
        .iter()
        .map(|(index, place)| {
            let (lo, hi) = all[index % all.len()];
            match place {
                0 => lo - micro(),
                1 => lo,
                2 => lo + (hi - lo) / 2,
                3 => hi,
                _ => hi + micro(),
            }
        })
        .collect()
}

/// PLAN-r4 §6.10's spans, literally: (A) (D.as_of, D'.as_of] for each
/// dirty-hull anchor D, D' the next anchor whose hull is clean; (R)
/// (O.as_of, A_R.as_of] for each re-anchor with a dirty dependency.
fn span_oracle(
    case: &SpanCase,
    fills: &[DateTime<Utc>],
) -> Vec<(char, DateTime<Utc>, Option<DateTime<Utc>>)> {
    let dirty = |lo: DateTime<Utc>, hi: DateTime<Utc>| fills.iter().any(|f| lo <= *f && *f <= hi);
    let window = |cut: DateTime<Utc>, back: u32| {
        let lo = cut - TimeDelta::seconds(back.into());
        (lo, lo + TimeDelta::milliseconds(500))
    };
    let hull_dirty = |k: usize| {
        let cut = span_cut(k);
        dirty(
            cut - TimeDelta::seconds(case.anchors[k].hull.into()),
            cut + TimeDelta::milliseconds(500),
        )
    };
    let mut spans = Vec::new();
    for k in 0..case.anchors.len() {
        if hull_dirty(k) {
            let next = (k + 1..case.anchors.len()).find(|j| !hull_dirty(*j)).map(span_cut);
            spans.push(('A', span_cut(k), next));
        }
        if k > 0 {
            let a = &case.anchors[k];
            let (dating_lo, dating_hi) = window(span_cut(k), a.dating);
            let used = a.sightings.iter().filter(|(_, excluded)| !excluded).any(|(back, _)| {
                let (lo, hi) = window(span_cut(k), *back);
                dirty(lo, hi)
            });
            if dirty(dating_lo, dating_hi) || hull_dirty(k) || used {
                spans.push(('R', span_cut(k - 1), Some(span_cut(k))));
            }
        }
    }
    spans
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn impairment_spans_match_an_independent_oracle(pool: PgPool) -> Result<()> {
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(0x81_3a_5a),
        failure_persistence: None,
        ..Config::default()
    });
    let (mut reads, mut impaired_reads, mut a_spans, mut r_spans) = (0, 0, 0, 0);
    for _ in 0..32 {
        let case = gen_span_case()
            .new_tree(&mut runner)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .current();
        let book = Book::new(&pool).await?;
        let mut ids = Vec::new();
        for (k, a) in case.anchors.iter().enumerate() {
            let cut = span_cut(k);
            let quote = Decimal::from(1000 + k as i64);
            ids.push(
                book.anchor(&pool, cut, cut - TimeDelta::seconds(a.hull.into()), quote).await?,
            );
            if k == 0 {
                continue;
            }
            let resolved = match a.sightings.first() {
                None => None,
                Some(_) => {
                    let sightings: Vec<_> = a
                        .sightings
                        .iter()
                        .map(|(back, _)| (cut - TimeDelta::seconds((*back).into()), dec!(-5)))
                        .collect();
                    let earliest = sightings.iter().map(|(at, _)| *at).min().expect("sighting");
                    let copy = earliest.min(span_cut(k - 1));
                    Some(book.discrepancy(&pool, "resolved", copy, &sightings).await?)
                },
            };
            let excluded: Vec<i32> =
                (1..).zip(&a.sightings).filter(|(_, (_, ex))| *ex).map(|(seq, _)| seq).collect();
            let excluded = if resolved.is_some() {
                excluded
            } else {
                Vec::new()
            };
            let dating = cut - TimeDelta::seconds(a.dating.into());
            book.reanchor(&pool, ids[k - 1], ids[k], cut, dating, resolved, &excluded)
                .await?;
        }
        let fills = fill_times(&case);
        for time in &fills {
            book.fill(&pool, &Fill::foreign(*time)).await?;
        }
        let spans = span_oracle(&case, &fills);
        let mut points = vec![at(0), span_cut(case.anchors.len()) + TimeDelta::seconds(1)];
        for k in 0..case.anchors.len() {
            let cut = span_cut(k);
            points.extend([
                cut - micro(),
                cut,
                cut + micro(),
                cut + TimeDelta::seconds(50),
            ]);
        }
        for t in points {
            let expected = spans.iter().any(|(_, lo, hi)| *lo < t && hi.is_none_or(|hi| t <= hi));
            let state = book.state(&pool, t, t).await?;
            let e = state.expected;
            ensure!(
                e.impaired == expected && state.impaired_now == expected,
                "{case:?} {fills:?} at {t}: {e:?}"
            );
            let anchor = (0..ids.len()).rev().find(|k| span_cut(*k) < t);
            let quote = anchor.filter(|_| !expected).map(|k| Decimal::from(1000 + k as i64));
            ensure!(e.value == quote, "{case:?} at {t}: {e:?}");
            reads += 1;
            impaired_reads += usize::from(expected);
        }
        a_spans += spans.iter().filter(|(kind, ..)| *kind == 'A').count();
        r_spans += spans.iter().filter(|(kind, ..)| *kind == 'R').count();
    }
    ensure!(
        impaired_reads > 30 && a_spans > 5 && r_spans > 5,
        "coverage {impaired_reads} {a_spans} {r_spans}"
    );
    eprintln!("SPAN_ORACLE cases=32 reads={reads} impaired_reads={impaired_reads} a_spans={a_spans} r_spans={r_spans}");
    Ok(())
}

// ---- one statement and its bound at PLAN-r4 §7.2's cardinality ----

/// PLAN-r4 §7.2's cardinality on `book`'s venue: 200 legs (half closed with
/// their dust rows), 2,000 intents, 1,000 orders and 10,000 fills (8l-2's
/// shape, all settled), 51 anchors and 50 re-anchors each resolving a
/// discrepancy of 10 sightings, and an open missing discrepancy of 500
/// sightings (-100). Shared with 8l-3b's writer bound. Returns the counts of
/// fills, sightings, re-anchors, legs and intents.
pub(super) async fn bound_fixture(pool: &PgPool, book: &Book) -> Result<(i64, i64, i64, i64, i64)> {
    let (tenant, scope, sample, epoch) = (book.tenant, &book.scope, book.sample, book.epoch);
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
            SELECT 'binance:spot','{scope}','{SYMBOL}',(2000000000+row_number() OVER ())::text,'{tenant}',i::text,i::text,'leg',i,p,
                   CASE WHEN k=1 THEN 'entry' ELSE 'stop' END,CASE WHEN k=1 THEN 'BUY' ELSE 'SELL' END,'MARKET',1,
                   CASE WHEN k>1 THEN 90 END,'FILLED','terminal',1,1,'2026-10-03T00:00:00Z','2026-10-02T00:00:00Z','2026-10-03T00:00:00Z' FROM j WHERE k<=5;
        INSERT INTO order_fills SELECT 'binance:spot','{scope}','{SYMBOL}',(o.exchange_order_id::bigint*10+t)::text,o.exchange_order_id,
                   '{tenant}',100,0.1,10,0.0001,CASE o.side WHEN 'BUY' THEN 'BTC' ELSE 'USDT' END,'2026-10-02T00:00:00Z','2026-10-03T00:00:00Z'
            FROM venue_orders o, generate_series(1,10) t WHERE o.tenant_id='{tenant}' AND o.exchange_order_id::bigint >= 2000000000;
        CREATE TEMP TABLE c AS SELECT n, timestamptz '2026-09-01T00:00:00Z' + n * interval '1 hour' AS cut,
            ('40000000-0000-0000-0000-'||lpad(n::text,12,'0'))::uuid AS anchor,
            ('50000000-0000-0000-0000-'||lpad(n::text,12,'0'))::uuid AS discrepancy
            FROM generate_series(0,50) n;
        INSERT INTO cash_anchors (anchor_id,tenant_id,venue_id,kind,quote,as_of,send_at,receive_at,u_ns,sample_id,epoch_id,interval_start,interval_end,depends_start,depends_end)
            SELECT anchor,'{tenant}','binance:spot','reanchor',10000,cut,cut+interval '150 ms',cut+interval '350 ms',150000000,'{sample}','{epoch}',cut,cut+interval '500 ms',cut-interval '10 s',cut+interval '500 ms' FROM c;
        INSERT INTO cash_discrepancies (discrepancy_id,tenant_id,venue_id,direction,state,first_send_at,first_receive_at,first_u_ns,first_sample_id,first_epoch_id,first_interval_start,first_interval_end,first_delta,checkpoint_cut,checkpoint_interval_start,checkpoint_interval_end,opened_version,last_seen_at,min_delta,closed_at)
            SELECT discrepancy,'{tenant}','binance:spot','missing','resolved',cut-interval '30 min'+interval '150 ms',cut-interval '30 min'+interval '350 ms',150000000,'{sample}','{epoch}',
                   cut-interval '30 min',cut-interval '30 min'+interval '500 ms',-1,cut-interval '1 hour',cut-interval '1 hour',cut-interval '1 hour'+interval '500 ms',1,cut-interval '30 min',-1,cut
            FROM c WHERE n > 0;
        INSERT INTO cash_sightings SELECT discrepancy,k,'{tenant}','binance:spot',cut-interval '30 min'+k*interval '1 min'+interval '150 ms',
                   cut-interval '30 min'+k*interval '1 min'+interval '350 ms',150000000,'{sample}','{epoch}',cut-interval '30 min'+k*interval '1 min',
                   cut-interval '30 min'+k*interval '1 min'+interval '500 ms',-1
            FROM c, generate_series(1,10) k WHERE n > 0;
        INSERT INTO cash_reanchors (reanchor_id,tenant_id,venue_id,operator_subject,reason,o1_send_at,o1_receive_at,o1_u_ns,o1_sample_id,o1_epoch_id,o1_interval_start,o1_interval_end,o1_total,
                o2_send_at,o2_receive_at,o2_u_ns,o2_sample_id,o2_epoch_id,o2_interval_start,o2_interval_end,o2_total,mode,checked_version,old_anchor_id,new_anchor_id,
                dating_cut,dating_interval_start,dating_interval_end,delta,min_delta,withdrawal,contribution,resolved_discrepancy_id,excluded_sighting_seqs)
            SELECT ('60000000-0000-0000-0000-'||lpad(c.n::text,12,'0'))::uuid,'{tenant}','binance:spot','operator-fixture','bound fixture',
                   c.cut-interval '9850 ms',c.cut-interval '9650 ms',150000000,'{sample}','{epoch}',c.cut-interval '10 s',c.cut-interval '9500 ms',10000,
                   c.cut+interval '150 ms',c.cut+interval '350 ms',150000000,'{sample}','{epoch}',c.cut,c.cut+interval '500 ms',10000,'attested',1,p.anchor,c.anchor,
                   p.cut,p.cut,p.cut+interval '500 ms',0,-1,1,1,c.discrepancy,ARRAY[1]
            FROM c JOIN c p ON p.n = c.n - 1;
        INSERT INTO cash_discrepancies (discrepancy_id,tenant_id,venue_id,direction,state,first_send_at,first_receive_at,first_u_ns,first_sample_id,first_epoch_id,first_interval_start,first_interval_end,first_delta,checkpoint_cut,checkpoint_interval_start,checkpoint_interval_end,opened_version,last_seen_at,min_delta)
            VALUES ('70000000-0000-0000-0000-000000000001','{tenant}','binance:spot','missing','open','2026-11-19T00:00:00.15Z','2026-11-19T00:00:00.35Z',150000000,'{sample}','{epoch}',
                    '2026-11-19T00:00:00Z','2026-11-19T00:00:00.5Z',-100,'2026-09-03T02:00:00Z','2026-09-03T02:00:00Z','2026-09-03T02:00:00.5Z',2,'2026-11-19T00:00:00.35Z',-100);
        INSERT INTO cash_sightings SELECT '70000000-0000-0000-0000-000000000001',k,'{tenant}','binance:spot',ts+interval '150 ms',ts+interval '350 ms',150000000,'{sample}','{epoch}',ts,ts+interval '500 ms',-100
            FROM (SELECT k, timestamptz '2026-11-19T00:00:00Z' + (k-1) * interval '10 s' AS ts FROM generate_series(1,500) k) s;
    ")).execute(pool).await?;
    // Production statistics: autovacuum would have analyzed these by now.
    sqlx::raw_sql("ANALYZE balance_legs, dust_ledger, venue_orders, order_fills, order_intents, cash_anchors, cash_reanchors, cash_discrepancies, cash_sightings")
        .execute(pool)
        .await?;
    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM order_fills),(SELECT count(*) FROM cash_sightings),(SELECT count(*) FROM cash_reanchors),(SELECT count(*) FROM balance_legs),(SELECT count(*) FROM order_intents)")
        .fetch_one(pool).await?;
    ensure!(
        counts.0 >= 10_000
            && counts.1 >= 1_000
            && counts.2 >= 50
            && counts.3 >= 200
            && counts.4 >= 2_000,
        "{counts:?}"
    );
    Ok(counts)
}

/// PLAN-r4 §7.2's cardinality ([`bound_fixture`]): one statement per read,
/// each well inside the 1 s statement budget of an 8l transaction.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cash_state_is_one_statement_within_its_bound(pool: PgPool) -> Result<()> {
    let book = Book::new(&pool).await?;
    let counts = bound_fixture(&pool, &book).await?;
    let tenant = book.tenant;

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
    let (b, now, last) = (
        utc("2026-11-01T00:00:00Z"),
        utc("2026-11-20T00:00:00Z"),
        utc("2026-09-03T02:00:00Z"),
    );
    // E(now) on the last anchor sums all 10,000 fills; t at the last
    // anchor's cut lies in its re-anchor's (R) span, so every dependency of
    // that re-anchor is read.
    let mut phases = Vec::new();
    proxy.enable_exact_query_trace(64);
    // E(B) rests on the last anchor and sums every fill; E at that anchor's
    // own cut rests on the one before it, and V6 needs a cut before t.
    for (phase, t, quote, before_t) in [
        ("boundary", b, dec!(69999.2), true),
        ("in_last_span", last, dec!(10000), false),
    ] {
        let query = CashQuery {
            tenant_id: tenant,
            venue: &venue,
            t,
            now,
            assets: ASSETS,
        };
        let mut times = Vec::new();
        for _ in 0..20 {
            let started = Instant::now();
            let state = read_cash_state(&mut connection, &query).await?;
            times.push(started.elapsed());
            ensure!(
                !state.impaired_now && state.open_missing && state.missing_before_t == before_t,
                "{state:?}"
            );
            ensure!(state.effective_minimum == Some(dec!(-100)), "{state:?}");
            ensure!(
                !state.expected.impaired && state.expected.value == Some(quote),
                "{phase}: {state:?}"
            );
        }
        times.sort();
        phases.push((phase, times));
    }
    let trace = proxy.query_trace();
    ensure!(
        trace.len() == 40 && trace.iter().all(|sql| *sql == *CASH_STATE),
        "one statement per read"
    );
    sqlx::query("SET plan_cache_mode = force_generic_plan")
        .execute(&mut *connection)
        .await?;
    let query = CashQuery {
        tenant_id: tenant,
        venue: &venue,
        t: b,
        now,
        assets: ASSETS,
    };
    let mut generic = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        read_cash_state(&mut connection, &query).await?;
        generic.push(started.elapsed());
    }
    generic.sort();
    phases.push(("generic", generic));
    let slowest = phases.iter().filter_map(|(_, times)| times.last()).max().copied();
    ensure!(slowest < Some(Duration::from_millis(1000)), "slowest read {slowest:?}");
    let plan: Vec<String> =
        sqlx::query_scalar(&format!("EXPLAIN (ANALYZE, BUFFERS) {}", *CASH_STATE))
            .bind(tenant)
            .bind(venue.as_str())
            .bind(last)
            .bind(now)
            .bind(vec![SYMBOL])
            .bind(vec!["BTC"])
            .bind(vec!["USDT"])
            .fetch_all(&pool)
            .await?;
    let summary: Vec<String> = phases
        .iter()
        .map(|(phase, times)| {
            format!(
                "{phase}: median_us={} max_us={}",
                times[times.len() / 2].as_micros(),
                times[times.len() - 1].as_micros()
            )
        })
        .collect();
    eprintln!("CASH_STATE_BOUND {} counts={counts:?}\n{}", summary.join(" "), plan.join("\n"));
    Ok(())
}

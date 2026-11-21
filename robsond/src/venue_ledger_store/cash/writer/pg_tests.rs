//! 8l-3b proofs on PostgreSQL (PLAN-r4 §8): the cash writer's branch table,
//! its refusals and final qualification, generated schedules of writers and
//! invalidators at every point of pre, request, post and publication, round
//! 12's sequences, restart, late evidence at the interval's ends, R11-4,
//! progress with the generation CAS, statement counts by proxy trace and the
//! row-holding bound with the barrier embedded at PLAN-r4 §7.2's cardinality.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{ensure, Result};
use chrono::TimeDelta;
use robson_exec::{
    gate::{IncidentReason, PendingEvidence},
    order_journal::{OrderJournal, StubJournalBackend},
    venue::{AccountKind, VenueId},
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use rust_decimal_macros::dec;
use sqlx::{postgres::PgPoolOptions, PgPool};

use super::{
    super::{
        pg_tests::{at, bound_fixture, micro, utc, Book, Fill, ASSETS},
        CashState,
    },
    *,
};
use crate::{
    balance_accounting::GenerationCell,
    balance_reconciliation::{read_pre, DetectorScope},
    process_epoch,
    spot_order_set_store::{ExpectedGeneration, JournalGeneration},
};

/// The observation's interval: [cut, cut + 500 ms], u = 150 ms.
fn interval(cut: DateTime<Utc>) -> ObservationInterval {
    let start = i128::from(cut.timestamp_micros()) * 1_000;
    ObservationInterval {
        start_ns: start,
        end_ns: start + 500_000_000,
        cut_ns: start,
    }
}

/// One balance-account daemon's cash writer on a fixture venue: the book, an
/// observed anchor of 1000 at `at(0)` on [-10 s, +500 ms], its accounting
/// context (with an installed order-set generation when opted), its gate
/// and its process epoch.
struct Rig {
    book: Book,
    accounting: BalanceAccounting,
    gate: Arc<OperationGate>,
    epoch: Uuid,
}

impl Rig {
    async fn new(pool: &PgPool, opted: bool) -> Result<Self> {
        let rig = Self::bare(pool, opted).await?;
        rig.book.anchor(pool, at(0), at(-10_000), dec!(1000)).await?;
        Ok(rig)
    }

    /// The same without the anchor.
    async fn bare(pool: &PgPool, opted: bool) -> Result<Self> {
        let book = Book::new(pool).await?;
        let venue = VenueId::new(VenueId::BINANCE_SPOT);
        // The context of a balance daemon (as `for_daemon` builds it) on the
        // book's own account scope: `venue_accounts` admits one tenant per
        // scope, and every rig of a test shares one database.
        let cell = GenerationCell::default();
        let accounting = BalanceAccounting {
            tenant_id: book.tenant,
            venue: venue.clone(),
            account_scope: book.scope.clone(),
            generation: cell.clone(),
        };
        if opted {
            let generation = JournalGeneration {
                tenant_id: book.tenant,
                account_scope: book.scope.clone(),
                generation_id: Uuid::now_v7(),
                process_epoch: book.epoch,
                source_audit_id: Uuid::now_v7(),
            };
            sqlx::query("INSERT INTO spot_order_set_generation (tenant_id,venue_id,account_scope,generation_id,process_epoch,source_audit_id,accounting_version,position_hints_complete) VALUES ($1,'binance:spot',$2,$3,$4,$5,0,true)")
                .bind(book.tenant).bind(&generation.account_scope).bind(generation.generation_id)
                .bind(generation.process_epoch).bind(generation.source_audit_id)
                .execute(pool).await?;
            cell.publish(generation);
        }
        let gate = OrderJournal::new(Arc::new(StubJournalBackend::default()), &venue)
            .gate()
            .clone();
        let epoch = book.epoch;
        Ok(Self { book, accounting, gate, epoch })
    }

    fn writer<'a>(&'a self, pool: &'a PgPool) -> CashWriter<'a> {
        CashWriter {
            pool,
            accounting: &self.accounting,
            gate: &self.gate,
            epoch: self.epoch,
            assets: ASSETS,
            read_only: false,
        }
    }

    async fn version(&self, pool: &PgPool) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT version FROM accounting_state WHERE tenant_id=$1")
            .bind(self.book.tenant)
            .fetch_one(pool)
            .await?)
    }

    /// A Clean attempt observing `total` at `cut`, with `v₀` and the marks
    /// read now (8k-3's pre).
    async fn attempt(
        &self,
        pool: &PgPool,
        cut: DateTime<Utc>,
        total: Decimal,
    ) -> Result<CashAttempt> {
        Ok(CashAttempt {
            verdict: BaseVerdict::Clean { interval: interval(cut) },
            version: self.version(pool).await?,
            marks: self.gate.invalidity_marks(),
            quote_total: total,
            u_ns: 150_000_000,
            sample_id: self.book.sample,
            epoch_id: self.epoch,
            received: Instant::now(),
        })
    }

    async fn conclude(
        &self,
        pool: &PgPool,
        cut: DateTime<Utc>,
        total: Decimal,
    ) -> Result<Concluded, CashRefusal> {
        let attempt = self.attempt(pool, cut, total).await.expect("pre");
        self.writer(pool).conclude(&attempt).await
    }

    async fn checkpoint(&self, pool: &PgPool) -> Result<Option<DateTime<Utc>>> {
        Ok(sqlx::query_scalar("SELECT cut FROM cash_checkpoints WHERE tenant_id=$1")
            .bind(self.book.tenant)
            .fetch_optional(pool)
            .await?)
    }

    /// Every discrepancy: direction, state, void reason, first delta, running
    /// minimum, checkpoint copy and sighting count, in opening order.
    async fn discrepancies(&self, pool: &PgPool) -> Result<Vec<Row8>> {
        Ok(sqlx::query_as("SELECT d.direction, d.state, d.void_reason, d.first_delta, d.min_delta, d.checkpoint_cut,
                (SELECT count(*) FROM cash_sightings s WHERE s.discrepancy_id = d.discrepancy_id), d.last_seen_at
            FROM cash_discrepancies d WHERE d.tenant_id=$1 ORDER BY d.first_interval_start, d.opened_version")
            .bind(self.book.tenant)
            .fetch_all(pool)
            .await?)
    }

    async fn state(
        &self,
        pool: &PgPool,
        t: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<CashState> {
        self.book.state(pool, t, now).await
    }

    /// Restart: a new process epoch on record and this rig's writer on it.
    async fn restart(&mut self, pool: &PgPool) -> Result<()> {
        let start = process_epoch::start_epoch(pool, self.book.tenant, "8l-3b-restart").await?;
        self.epoch = start.epoch.epoch_id;
        let venue = VenueId::new(VenueId::BINANCE_SPOT);
        self.gate = OrderJournal::new(Arc::new(StubJournalBackend::default()), &venue)
            .gate()
            .clone();
        Ok(())
    }

    /// 8k-2's pre statement qualifies the installed generation (8l-4a's
    /// `qualifies`): it is current at the accounting version in the live epoch.
    async fn qualifies(&self, pool: &PgPool) -> Result<bool> {
        const UID: &str = "8l-3b-scan";
        let installed = self.accounting.generation.current().expect("installed");
        sqlx::query("INSERT INTO venue_accounts (venue_id, account_scope, tenant_id, uid, bound_at, cutover_id) VALUES ('binance:spot', $1, $2, $3, now(), $4) ON CONFLICT DO NOTHING")
            .bind(&installed.account_scope).bind(installed.tenant_id).bind(UID).bind(Uuid::now_v7())
            .execute(pool).await?;
        let venue = VenueId::new(VenueId::BINANCE_SPOT);
        let scope = DetectorScope {
            expected: ExpectedGeneration {
                tenant_id: installed.tenant_id,
                venue: &venue,
                account_scope: &installed.account_scope,
                generation_id: installed.generation_id,
                process_epoch: installed.process_epoch,
                source_audit_id: installed.source_audit_id,
            },
            uid: UID,
            operated: SYMBOL,
            base_asset: "BTC",
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        Ok(read_pre(&pool.connect_options(), scope, deadline).await.is_ok())
    }
}

const SYMBOL: &str = "BTCUSDT"; // A labelled fixture pair, never a policy restriction.

type Row8 = (
    String,
    String,
    Option<String>,
    Decimal,
    Decimal,
    DateTime<Utc>,
    i64,
    DateTime<Utc>,
);


/// R11-4 (v16:1980-1981): only opening and voiding move the version;
/// checkpoints and sightings never do, and the generation follows each bump.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn r11_4_only_opening_and_voiding_bump_the_version(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, true).await?;
    // (total, bumps): clean, extra opens, extra sighting, extra cleared by a
    // clean observation, missing opens, missing sightings (missing, clean,
    // extra), extra superseded would need an open extra: below.
    let steps = [
        (dec!(1000.01), 0),
        (dec!(1050), 1),
        (dec!(1020), 0),
        (dec!(999.99), 1),
        (dec!(900), 1),
        (dec!(880), 0),
        (dec!(1000), 0),
        (dec!(1100), 0),
    ];
    for (n, (total, bumps)) in (1..).zip(steps) {
        let before = rig.version(&pool).await?;
        let done = rig.conclude(&pool, at(n * 60_000), total).await?;
        let after = rig.version(&pool).await?;
        ensure!(
            after - before == bumps && done.bumped.is_some() == (bumps == 1),
            "{n}: {done:?}"
        );
        ensure!(rig.qualifies(&pool).await?, "{n}: the generation follows");
    }
    // Extra superseded by missing: one bump for the void and the insert.
    let rig = Rig::new(&pool, true).await?;
    rig.conclude(&pool, at(60_000), dec!(1050)).await?;
    let before = rig.version(&pool).await?;
    let done = rig.conclude(&pool, at(120_000), dec!(950)).await?;
    ensure!(rig.version(&pool).await? == before + 1 && done.writes.len() == 2, "{done:?}");
    Ok(())
}

/// v16:494-495: a clean observation voids extra cash, never missing cash.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn clean_observation_never_voids_missing_cash(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, false).await?;
    rig.conclude(&pool, at(60_000), dec!(900)).await?;
    for (n, total) in [(2, dec!(1000)), (3, dec!(999.995)), (4, dec!(1000.01))] {
        let done = rig.conclude(&pool, at(n * 60_000), total).await?;
        ensure!(done.bumped.is_none(), "{done:?}");
        let rows = rig.discrepancies(&pool).await?;
        ensure!(rows.len() == 1 && rows[0].0 == "missing" && rows[0].1 == "open", "{rows:?}");
        ensure!(rig.state(&pool, at(n * 60_000), at(n * 60_000)).await?.open_missing);
        ensure!(rig.checkpoint(&pool).await? == Some(at(n * 60_000)), "a clean cut checkpoints");
    }
    Ok(())
}

/// v16:2033: missing cash never writes a checkpoint, whatever follows.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn missing_cash_never_writes_a_checkpoint(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, false).await?;
    rig.conclude(&pool, at(60_000), dec!(1000)).await?;
    for (n, total) in [
        (2, dec!(940)),
        (3, dec!(880)),
        (4, dec!(1100)),
        (5, dec!(800)),
    ] {
        rig.conclude(&pool, at(n * 60_000), total).await?;
        ensure!(rig.checkpoint(&pool).await? == Some(at(60_000)), "{n}");
    }
    // Late evidence voids the discrepancy; the re-detection still writes none.
    for seq in [0, 1, 2, 3] {
        rig.book.fill(&pool, &Fill::foreign(at((seq + 2) * 60_000 + 100))).await?;
    }
    let done = rig.conclude(&pool, at(360_000), dec!(800)).await?;
    ensure!(
        matches!(done.writes[..], [Write::Void(_, "late_evidence"), Write::Open(true)]),
        "{done:?}"
    );
    ensure!(rig.checkpoint(&pool).await? == Some(at(60_000)));
    let rows = rig.discrepancies(&pool).await?;
    ensure!(
        rows[1].5 == at(60_000),
        "the re-detection keeps the clean checkpoint copy: {rows:?}"
    );
    Ok(())
}

/// Positive, then negative, then clean: the extra row is superseded by a
/// missing one, which fences and which the clean observation keeps.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn extra_then_missing_fences_and_clean_keeps_it(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, true).await?;
    rig.conclude(&pool, at(30_000), dec!(1000)).await?;
    rig.conclude(&pool, at(60_000), dec!(1050)).await?;
    ensure!(
        !rig.state(&pool, at(60_000), at(90_000)).await?.open_missing,
        "extra only alarms"
    );
    let done = rig.conclude(&pool, at(120_000), dec!(970)).await?;
    ensure!(matches!(done.writes[..], [
        Write::Void(_, "superseded_by_missing"),
        Write::Open(true)
    ]));
    let rows = rig.discrepancies(&pool).await?;
    ensure!(
        rows.len() == 2 && rows[0].2.as_deref() == Some("superseded_by_missing"),
        "{rows:?}"
    );
    ensure!(
        rows[1].0 == "missing" && rows[1].1 == "open" && rows[1].5 == rows[0].5,
        "{rows:?}"
    );
    ensure!(rig.state(&pool, at(120_000), at(150_000)).await?.open_missing);
    rig.conclude(&pool, at(180_000), dec!(1000)).await?;
    let state = rig.state(&pool, at(180_000), at(200_000)).await?;
    ensure!(state.open_missing && state.effective_minimum == Some(dec!(-30)), "{state:?}");
    ensure!(rig.checkpoint(&pool).await? == Some(at(180_000)));
    ensure!(rig.qualifies(&pool).await?);
    Ok(())
}

/// Round 12's two partial withdrawals (v16:2010-2011): -60 then -120, the
/// minimum Δ is -120 and the first sighting and its copy stay.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn two_partial_withdrawals_keep_the_largest_shortfall(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, false).await?;
    rig.conclude(&pool, at(30_000), dec!(1000)).await?;
    rig.conclude(&pool, at(60_000), dec!(940)).await?;
    rig.conclude(&pool, at(120_000), dec!(880)).await?;
    rig.conclude(&pool, at(180_000), dec!(900)).await?;
    let rows = rig.discrepancies(&pool).await?;
    ensure!(rows.len() == 1, "{rows:?}");
    let (_, _, _, first, min, copy, sightings, last_seen) = &rows[0];
    ensure!(
        (*first, *min, *copy, *sightings) == (dec!(-60), dec!(-120), at(30_000), 3),
        "{rows:?}"
    );
    ensure!(*last_seen == at(180_000) + TimeDelta::milliseconds(350), "{rows:?}");
    let state = rig.state(&pool, at(180_000), at(200_000)).await?;
    ensure!(state.effective_minimum == Some(dec!(-120)) && state.open_missing, "{state:?}");
    Ok(())
}

/// A discrepancy's first sighting, checkpoint copy, sightings and minimum
/// survive restarts (v16:2012-2013); a capture of the old epoch is refused.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn restart_keeps_the_first_sighting_the_sightings_and_the_minimum(
    pool: PgPool,
) -> Result<()> {
    let mut rig = Rig::new(&pool, false).await?;
    rig.conclude(&pool, at(30_000), dec!(1000)).await?;
    rig.conclude(&pool, at(60_000), dec!(900)).await?;
    let old = rig.attempt(&pool, at(90_000), dec!(850)).await?;
    let before = rig.discrepancies(&pool).await?;
    rig.restart(&pool).await?;
    let refused = rig.writer(&pool).conclude(&old).await;
    ensure!(matches!(refused, Err(CashRefusal::Epoch)), "{refused:?}");
    ensure!(rig.discrepancies(&pool).await? == before, "a refusal writes nothing");
    rig.conclude(&pool, at(120_000), dec!(850)).await?;
    rig.restart(&pool).await?;
    rig.conclude(&pool, at(180_000), dec!(1000)).await?;
    let rows = rig.discrepancies(&pool).await?;
    ensure!(rows.len() == 1, "{rows:?}");
    let (direction, state, _, first, min, copy, sightings, _) = &rows[0];
    ensure!(
        (direction.as_str(), state.as_str(), *first, *min, *copy, *sightings)
            == ("missing", "open", dec!(-100), dec!(-150), at(30_000), 3),
        "{rows:?}"
    );
    Ok(())
}

/// The write-time guard: a booked fill at either end of the observation's
/// interval refuses the conclusion; one just outside does not.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn booked_fill_inside_the_interval_refuses_the_conclusion(pool: PgPool) -> Result<()> {
    let cut = at(60_000);
    let end = cut + TimeDelta::milliseconds(500);
    for (time, inside) in [
        (cut, true),
        (end, true),
        (cut - micro(), false),
        (end + micro(), false),
    ] {
        let rig = Rig::new(&pool, false).await?;
        rig.book.fill(&pool, &Fill::foreign(time)).await?;
        let version = rig.version(&pool).await?;
        let result = rig.conclude(&pool, cut, dec!(900)).await;
        if inside {
            ensure!(matches!(result, Err(CashRefusal::DirtyObservation)), "{time}: {result:?}");
            ensure!(
                rig.discrepancies(&pool).await?.is_empty() && rig.version(&pool).await? == version
            );
        } else {
            ensure!(result.is_ok_and(|done| done.writes == [Write::Open(true)]), "{time}");
        }
    }
    Ok(())
}

/// PLAN-r4 §6.4 step 4: a memory invalidator or pending evidence after the
/// attempt's post refuses publication, with nothing written; a fresh capture
/// publishes. Registered pending evidence (crash-points builds, where the
/// receiver exists) refuses too.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn pending_or_generation_change_after_post_refuses_publication(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, true).await?;
    let attempt = rig.attempt(&pool, at(60_000), dec!(900)).await?;
    let _ = rig.gate.record(IncidentReason::StreamSuspended);
    let refused = rig.writer(&pool).conclude(&attempt).await;
    ensure!(matches!(refused, Err(CashRefusal::Marks)), "{refused:?}");
    ensure!(rig.discrepancies(&pool).await?.is_empty() && rig.version(&pool).await? == 0);
    let mut pending = rig.attempt(&pool, at(60_000), dec!(900)).await?;
    pending.marks.pending = PendingEvidence::Pending;
    ensure!(matches!(rig.writer(&pool).conclude(&pending).await, Err(CashRefusal::Marks)));
    ensure!(rig.discrepancies(&pool).await?.is_empty() && rig.version(&pool).await? == 0);
    let fresh = rig.attempt(&pool, at(60_000), dec!(900)).await?;
    ensure!(rig.writer(&pool).conclude(&fresh).await?.writes == [Write::Open(true)]);
    if cfg!(feature = "crash-points") {
        let attempt = rig.attempt(&pool, at(90_000), dec!(880)).await?;
        let before = (rig.discrepancies(&pool).await?, rig.version(&pool).await?);
        register_pending(&rig.gate, rig.epoch)?;
        let refused = rig.writer(&pool).conclude(&attempt).await;
        ensure!(matches!(refused, Err(CashRefusal::Marks)), "{refused:?}");
        ensure!((rig.discrepancies(&pool).await?, rig.version(&pool).await?) == before);
    }
    Ok(())
}

/// A discrepancy left with no clean sighting of its direction is voided by
/// the next conclusion (`late_evidence`) and judged as none open: a real
/// shortfall is re-detected at once with the unchanged checkpoint copy.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn late_evidence_voids_durably_and_redetects(pool: PgPool) -> Result<()> {
    for (opened, total, writes_after) in [
        (dec!(900), dec!(900), "redetect"),
        (dec!(1100), dec!(1000), "clean"),
    ] {
        let rig = Rig::new(&pool, true).await?;
        rig.conclude(&pool, at(30_000), dec!(1000)).await?;
        rig.conclude(&pool, at(60_000), opened).await?;
        rig.book.fill(&pool, &Fill::foreign(at(60_100))).await?;
        let before = rig.version(&pool).await?;
        let done = rig.conclude(&pool, at(120_000), total).await?;
        ensure!(
            rig.version(&pool).await? == before + 1 && rig.qualifies(&pool).await?,
            "{done:?}"
        );
        let rows = rig.discrepancies(&pool).await?;
        ensure!(rows[0].1 == "void" && rows[0].2.as_deref() == Some("late_evidence"), "{rows:?}");
        match writes_after {
            "redetect" => {
                ensure!(matches!(done.writes[..], [
                    Write::Void(_, "late_evidence"),
                    Write::Open(true)
                ]));
                ensure!(
                    rows.len() == 2 && rows[1].1 == "open" && rows[1].5 == at(30_000),
                    "{rows:?}"
                );
            },
            _ => {
                ensure!(matches!(done.writes[..], [
                    Write::Void(_, "late_evidence"),
                    Write::Checkpoint
                ]));
                ensure!(rows.len() == 1 && rig.checkpoint(&pool).await? == Some(at(120_000)));
            },
        }
    }
    Ok(())
}

/// PLAN-r4 §8's regression: a clean -100 sighting before B, a deposit, a
/// clean ~0 sighting, then a fill booked later inside a third sighting. After
/// a restart and across B the missing fence and V6 hold and the effective
/// minimum is -100; the re-anchor 8l-5 would book (written here as fixture
/// rows from those inputs) dates W = 100 at the stored checkpoint before B,
/// so quote_B falls, and C = 100 after it, with E continuous.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn dirty_later_sighting_keeps_clean_missing_evidence_across_restart_and_boundary(
    pool: PgPool,
) -> Result<()> {
    let mut rig = Rig::new(&pool, false).await?;
    let b = utc("2026-12-01T00:00:00Z");
    let (t1, s1, s2, s3) = (
        b - TimeDelta::hours(1),
        b - TimeDelta::minutes(30),
        b + TimeDelta::minutes(30),
        b + TimeDelta::hours(1),
    );
    rig.conclude(&pool, t1, dec!(1000)).await?;
    rig.conclude(&pool, s1, dec!(900)).await?;
    // The operator deposits 100 back: a clean observation keeps missing cash.
    rig.conclude(&pool, s2, dec!(1000)).await?;
    rig.conclude(&pool, s3, dec!(850)).await?;
    // A buy of 150 at a venue time inside s3's interval, booked only now.
    let bought = Fill {
        leg: true,
        qty: dec!(1.5),
        ..Fill::foreign(s3 + TimeDelta::milliseconds(200))
    };
    rig.book.fill(&pool, &bought).await?;
    rig.restart(&pool).await?;
    let now = b + TimeDelta::hours(2);
    let state = rig.state(&pool, b, now).await?;
    ensure!(state.open_missing && state.missing_before_t, "the fence and V6 hold: {state:?}");
    ensure!(state.effective_minimum == Some(dec!(-100)), "{state:?}");
    ensure!(state.expected.value == Some(dec!(1000)) && !state.impaired_now, "{state:?}");
    let rows = rig.discrepancies(&pool).await?;
    ensure!(rows.len() == 1 && rows[0].4 == dec!(-150) && rows[0].5 == t1, "{rows:?}");
    // The wallet (850) now matches E: the next scan is clean, keeps it open.
    let done = rig.conclude(&pool, now, dec!(850)).await?;
    ensure!(done.delta == dec!(0) && done.bumped.is_none(), "{done:?}");
    ensure!(rig.discrepancies(&pool).await?[0].1 == "open");
    // 8l-5's inputs and outputs, as fixture rows: min Δ = min(-100, 0, 0, 0),
    // W = 100 at the stored copy t1 (after the latest observed anchor), C =
    // Δ(O2) + W = 100 at O1's receipt plus u, the new anchor at O2's cut.
    let anchor0: Uuid = sqlx::query_scalar("SELECT anchor_id FROM cash_anchors WHERE tenant_id=$1")
        .bind(rig.book.tenant)
        .fetch_one(&pool)
        .await?;
    let o2 = now + TimeDelta::minutes(20);
    let anchor1 = rig.book.anchor(&pool, o2, o2 - TimeDelta::seconds(10), dec!(850)).await?;
    let resolved: Uuid = sqlx::query_scalar("UPDATE cash_discrepancies SET state='resolved', closed_at=$2 WHERE tenant_id=$1 AND state='open' RETURNING discrepancy_id")
        .bind(rig.book.tenant).bind(o2).fetch_one(&pool).await?;
    let r = rig.book.reanchor(&pool, anchor0, anchor1, o2, t1, Some(resolved), &[3]).await?;
    rig.book.flow(&pool, r, true, dec!(100), t1).await?;
    rig.book.flow(&pool, r, false, dec!(100), o2 - TimeDelta::seconds(9)).await?;
    let quote_b = rig.state(&pool, b, o2 + TimeDelta::hours(1)).await?;
    ensure!(
        quote_b.expected.value == Some(dec!(900)) && !quote_b.expected.impaired,
        "{quote_b:?}"
    );
    // Dated before B, the withdrawal lowers quote_B and fences no December.
    ensure!(!quote_b.withdrawal_fence && !quote_b.open_missing, "{quote_b:?}");
    for t in [o2 - TimeDelta::seconds(1), o2 + micro()] {
        ensure!(
            rig.state(&pool, t, t).await?.expected.value == Some(dec!(850)),
            "continuous at {t}"
        );
    }
    Ok(())
}

/// Round 12's sequences under decision A (v16:2000-2011).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn round_12_sequences_under_decision_a(pool: PgPool) -> Result<()> {
    let b = utc("2026-12-01T00:00:00Z");
    // Cancel before B with no observation in between, and a contribution
    // masking a withdrawal: nothing is ever visible (the operator's
    // commitment covers it), and nothing moved gives quote_B = E(B).
    let rig = Rig::new(&pool, false).await?;
    rig.conclude(&pool, b - TimeDelta::seconds(30), dec!(1000)).await?;
    rig.conclude(&pool, b + TimeDelta::seconds(20), dec!(1000)).await?;
    ensure!(rig.discrepancies(&pool).await?.is_empty());
    let state = rig.state(&pool, b, b + TimeDelta::minutes(1)).await?;
    ensure!(state.expected.value == Some(dec!(1000)) && !state.missing_before_t, "{state:?}");
    // With an observation in between: missing at B - 10 s, the snapshot
    // waits (V6), and the later clean observation does not void it.
    let rig = Rig::new(&pool, false).await?;
    rig.conclude(&pool, b - TimeDelta::seconds(30), dec!(1000)).await?;
    rig.conclude(&pool, b - TimeDelta::seconds(10), dec!(900)).await?;
    rig.conclude(&pool, b + TimeDelta::seconds(20), dec!(1000)).await?;
    let state = rig.state(&pool, b, b + TimeDelta::minutes(1)).await?;
    ensure!(state.open_missing && state.missing_before_t, "{state:?}");
    ensure!(state.effective_minimum == Some(dec!(-100)), "{state:?}");
    let rows = rig.discrepancies(&pool).await?;
    ensure!(rows.len() == 1 && rows[0].5 == b - TimeDelta::seconds(30), "{rows:?}");
    Ok(())
}

/// Progress (PLAN-r4 §6.5): repeated scans conclude, each with the version
/// captured at its own pre; bumping conclusions advance the order-set
/// generation by its CAS so 8k-2's pre still qualifies; clean scans move the
/// checkpoint forward; a stale capture is refused and its retry concludes.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn repeated_scans_advance_checkpoint_and_qualify(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, true).await?;
    let totals = [
        dec!(1000),
        dec!(1000),
        dec!(900),
        dec!(900),
        dec!(1000),
        dec!(1100),
        dec!(1000),
    ];
    let mut bumps = 0;
    for (n, total) in (1..).zip(totals) {
        let done = rig.conclude(&pool, at(n * 60_000), total).await?;
        bumps += i64::from(done.bumped.is_some());
        ensure!(rig.qualifies(&pool).await?, "scan {n}: {done:?}");
    }
    ensure!(rig.version(&pool).await? == bumps && bumps >= 1, "{bumps}");
    ensure!(rig.checkpoint(&pool).await? == Some(at(7 * 60_000)));
    let stale = rig.attempt(&pool, at(8 * 60_000), dec!(1200)).await?;
    sqlx::query("UPDATE accounting_state SET version = version + 1 WHERE tenant_id=$1")
        .bind(rig.book.tenant)
        .execute(&pool)
        .await?;
    ensure!(matches!(
        rig.writer(&pool).conclude(&stale).await,
        Err(CashRefusal::VersionMoved)
    ));
    ensure!(
        !rig.qualifies(&pool).await?,
        "an outside bump without the CAS stales the generation"
    );
    // A stale generation refuses every bumping conclusion, writing nothing,
    // until the owner's rebuild re-installs it; then the retry lands.
    let rig = Rig::new(&pool, true).await?;
    rig.conclude(&pool, at(60_000), dec!(1000)).await?;
    sqlx::query("UPDATE accounting_state SET version = version + 1 WHERE tenant_id=$1")
        .bind(rig.book.tenant)
        .execute(&pool)
        .await?;
    let refused = rig.conclude(&pool, at(120_000), dec!(900)).await;
    ensure!(matches!(refused, Err(CashRefusal::Accounting(_))), "{refused:?}");
    ensure!(rig.discrepancies(&pool).await?.is_empty() && rig.version(&pool).await? == 1);
    sqlx::query("UPDATE spot_order_set_generation SET accounting_version = 1 WHERE tenant_id=$1")
        .bind(rig.book.tenant)
        .execute(&pool)
        .await?;
    ensure!(rig.conclude(&pool, at(180_000), dec!(900)).await?.bumped == Some(2));
    ensure!(rig.qualifies(&pool).await?);
    Ok(())
}

// ---- generated schedules of writers and invalidators ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invalidator {
    /// A memory invalidator: an occurrence moves the invalidity generation.
    Occurrence,
    /// Evidence registered and unacknowledged (crash-points builds).
    Pending,
    /// A bumping writer (a projector apply, a journal write): bump and CAS.
    Bump,
    /// A fill booked with its venue time inside the observation's interval.
    FillInside,
    /// A fill booked with its venue time outside it.
    FillOutside,
    /// Another cash conclusion, opening a discrepancy at an earlier cut.
    Concurrent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Point {
    BeforePre,
    PreToRequest,
    RequestToPost,
    PostToPublication,
    /// While a DB invalidator holds the accounting rows.
    Holding,
    AfterPublication,
}

impl Rig {
    /// A bumping writer's own transaction: the rows, the bump and the CAS.
    async fn bump(&self, pool: &PgPool) -> Result<()> {
        let mut tx = pool.begin().await?;
        self.accounting.advance(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn invalidate(&self, pool: &PgPool, what: Invalidator, cut: DateTime<Utc>) -> Result<()> {
        match what {
            Invalidator::Occurrence => drop(self.gate.record(IncidentReason::StreamSuspended)),
            Invalidator::Pending => register_pending(&self.gate, self.epoch)?,
            Invalidator::Bump => self.bump(pool).await?,
            Invalidator::FillInside | Invalidator::FillOutside => {
                let time = match what {
                    Invalidator::FillInside => cut + TimeDelta::milliseconds(250),
                    _ => cut - TimeDelta::seconds(1),
                };
                self.book.fill(pool, &Fill::foreign(time)).await?;
                self.bump(pool).await?;
            },
            Invalidator::Concurrent => {
                self.conclude(pool, cut - TimeDelta::seconds(5), dec!(950)).await?;
            },
        }
        Ok(())
    }
}

/// Pending evidence registered on the gate's slot and never acknowledged.
#[cfg(feature = "crash-points")]
fn register_pending(gate: &Arc<OperationGate>, epoch: Uuid) -> Result<()> {
    use robson_exec::{
        clock_records::RawClockCapture,
        gate::{PendingExecutionEvidence, PendingExecutionRegistration},
        venue::{OrderPhase, VenueOrderRecord},
        venue_clock::ClockDomain,
    };
    if gate.epoch().is_none() {
        let view = robson_exec::gate::EpochView { epoch_seq: 1, unclean_reason: None };
        gate.begin_epoch(view, None, Vec::new());
    }
    let owner = match gate.bind_pending_execution(epoch) {
        Some(owner) => owner,
        None => anyhow::bail!("the slot binds once per gate"),
    };
    let domain = ClockDomain::new(Uuid::from_u128(0xb0), 0, 0).expect("domain");
    let capture = |second: i64| RawClockCapture {
        epoch,
        wall_seconds: second,
        wall_nanoseconds: 0,
        domain,
        before_ns: i128::from(second) * 1_000_000_000,
        after_ns: i128::from(second) * 1_000_000_000 + 1,
    };
    let evidence = PendingExecutionEvidence {
        sample: Uuid::from_u128(0x5a),
        send: capture(100),
        receive: capture(101),
        executed: dec!(0.25),
    };
    let record = VenueOrderRecord {
        exchange_order_id: "7".into(),
        client_order_id: "pending-fixture".into(),
        orig_client_order_id: None,
        symbol: SYMBOL.into(),
        side: robson_domain::OrderSide::Sell,
        order_type: "MARKET".into(),
        protection_kind: None,
        status: "PARTIALLY_FILLED".into(),
        phase: OrderPhase::Live,
        orig_qty: Decimal::ONE,
        executed_qty: dec!(0.25),
        cumulative_quote_qty: None,
        stop_price: None,
        expiry_reason: None,
        transact_time_ms: None,
        time_ms: None,
        update_time_ms: None,
        working_time_ms: None,
    };
    let registered = owner.register(evidence, Arc::new(record));
    // The ticket is dropped unacknowledged: the evidence stays pending.
    ensure!(matches!(registered, PendingExecutionRegistration::Issued(_)), "issued");
    Ok(())
}

#[cfg(not(feature = "crash-points"))]
fn register_pending(_: &Arc<OperationGate>, _: Uuid) -> Result<()> {
    anyhow::bail!("pending evidence registers in crash-points builds only")
}

/// At least 24 schedules (PLAN-r4 §8): each invalidator at every point of a
/// scan's pre, request, post and publication, and after it. The attempt's
/// 8k-2 post re-read (version and marks) is emulated; the writer is called
/// even when post would have discarded the attempt, and must refuse alone.
/// After every schedule: a refusal wrote nothing, at most one discrepancy is
/// open, and the order-set generation is still current.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn generated_schedules_never_publish_an_invalidated_observation(pool: PgPool) -> Result<()> {
    use Invalidator as I;
    let mut kinds = vec![
        I::Occurrence,
        I::Bump,
        I::FillInside,
        I::FillOutside,
        I::Concurrent,
    ];
    if cfg!(feature = "crash-points") {
        kinds.push(I::Pending);
    }
    let points = [
        Point::BeforePre,
        Point::PreToRequest,
        Point::RequestToPost,
        Point::PostToPublication,
        Point::Holding,
        Point::AfterPublication,
    ];
    let (mut schedules, mut refused, mut published) = (0, 0, 0);
    for what in kinds {
        for point in points {
            let memory = matches!(what, I::Occurrence | I::Pending);
            // The holder is a bumping writer: one schedule.
            if point == Point::Holding && what != I::Bump {
                continue;
            }
            let rig = Rig::new(&pool, true).await?;
            rig.conclude(&pool, at(30_000), dec!(1000)).await?;
            let cut = at(60_000);
            if point == Point::BeforePre {
                rig.invalidate(&pool, what, cut).await?;
            }
            let attempt = rig.attempt(&pool, cut, dec!(900)).await?;
            if matches!(point, Point::PreToRequest | Point::RequestToPost) {
                rig.invalidate(&pool, what, cut).await?;
            }
            // 8k-2's post: version and marks re-read after the request.
            let post = rig.version(&pool).await? == attempt.version
                && rig.gate.invalidity_marks() == attempt.marks;
            if point == Point::PostToPublication {
                rig.invalidate(&pool, what, cut).await?;
            }
            let opened_before: i64 = count_open(&pool, &rig).await?;
            let result = match point {
                Point::Holding => holding(&pool, &rig, &attempt).await?,
                _ => rig.writer(&pool).conclude(&attempt).await,
            };
            if point == Point::AfterPublication {
                rig.invalidate(&pool, what, cut).await?;
            }
            let ours: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM cash_sightings WHERE tenant_id=$1 AND interval_start=$2",
            )
            .bind(rig.book.tenant)
            .bind(cut)
            .fetch_one(&pool)
            .await?;
            let label = format!("{what:?} at {point:?}: {result:?}");
            match point {
                Point::BeforePre if what == I::Pending => {
                    ensure!(matches!(result, Err(CashRefusal::Marks)) && ours == 0, "{label}");
                },
                Point::BeforePre if what == I::FillInside => {
                    ensure!(
                        matches!(result, Err(CashRefusal::DirtyObservation)) && ours == 0,
                        "{label}"
                    );
                },
                Point::BeforePre | Point::AfterPublication => {
                    ensure!(result.is_ok() && ours == 1, "{label}");
                },
                Point::PreToRequest | Point::RequestToPost => {
                    ensure!(!post, "8k-2's post discards it: {label}");
                    ensure!(result.is_err() && ours == 0, "{label}");
                },
                Point::PostToPublication | Point::Holding => {
                    ensure!(point == Point::Holding || post, "{label}");
                    let expected = match memory {
                        true => matches!(result, Err(CashRefusal::Marks)),
                        false => matches!(result, Err(CashRefusal::VersionMoved)),
                    };
                    ensure!(expected && ours == 0, "{label}");
                },
            }
            if result.is_err() {
                ensure!(count_open(&pool, &rig).await? == opened_before, "{label}");
                refused += 1;
            } else {
                published += 1;
            }
            ensure!(count_open(&pool, &rig).await? <= 1, "{label}");
            ensure!(rig.qualifies(&pool).await?, "{label}");
            if point == Point::AfterPublication && what == I::FillInside {
                // Late evidence in the published interval: the first
                // sighting is dirty, so nothing fences and the next scan
                // voids it and re-detects with the same checkpoint copy.
                let state = rig.state(&pool, at(90_000), at(90_000)).await?;
                ensure!(!state.open_missing, "{label}: {state:?}");
                let next = rig.conclude(&pool, at(120_000), dec!(900)).await?;
                ensure!(matches!(next.writes[..], [
                    Write::Void(_, "late_evidence"),
                    Write::Open(true)
                ]));
                ensure!(rig.discrepancies(&pool).await?[1].5 == at(30_000));
            }
            schedules += 1;
        }
    }
    ensure!(schedules >= 24, "{schedules} schedules");
    eprintln!("CASH_SCHEDULES schedules={schedules} refused={refused} published={published}");
    Ok(())
}

async fn count_open(pool: &PgPool, rig: &Rig) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM cash_discrepancies WHERE tenant_id=$1 AND state='open'",
    )
    .bind(rig.book.tenant)
    .fetch_one(pool)
    .await?)
}

/// The writer runs while another transaction holds the accounting rows and
/// bumps them: it waits on the rows, then sees the version moved.
async fn holding(
    pool: &PgPool,
    rig: &Rig,
    attempt: &CashAttempt,
) -> Result<Result<Concluded, CashRefusal>> {
    let mut tx = pool.begin().await?;
    rig.accounting.advance(&mut tx).await?;
    let writer = rig.writer(pool);
    let release = async move {
        tokio::time::sleep(Duration::from_millis(60)).await;
        tx.commit().await
    };
    let (result, committed) = tokio::join!(writer.conclude(attempt), release);
    committed?;
    Ok(result)
}

// ---- statements and the row-holding bound ----

/// Every statement a conclusion sends, by what it is.
fn shape(trace: &[String]) -> Vec<&'static str> {
    trace
        .iter()
        .map(|sql| match sql.trim() {
            _ if sql.as_str() == CONTEXT.as_str() => "context",
            "BEGIN" => "begin",
            "COMMIT" => "commit",
            sql if sql == BOUNDS => "bounds",
            sql if sql == ACCOUNTING_ROWS => "lock",
            sql if sql.starts_with("WITH locked AS (") => "bump",
            sql if sql.contains("UPDATE public.spot_order_set_generation") => "cas",
            sql if sql == CHECKPOINT => "checkpoint",
            sql if sql == OPEN => "open",
            sql if sql == SIGHT => "sight",
            sql if sql == VOID => "void",
            _ => "other",
        })
        .collect()
}

async fn proxied(pool: &PgPool) -> Result<(DbProxy, PgPool)> {
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
    Ok((proxy, proxied))
}

/// PLAN-r4 §6.4 / §7.2: each branch's exact statements, at most nine, by
/// proxy trace; then, at §7.2's cardinality with the barrier embedded, every
/// conclusion holds the rows well inside 400 ms.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cash_conclusion_is_at_most_nine_statements_within_its_bound(pool: PgPool) -> Result<()> {
    let (proxy, through) = proxied(&pool).await?;
    let rig = Rig::new(&pool, true).await?;
    let head = ["begin", "bounds", "lock", "context"];
    let bumped = ["begin", "bounds", "lock", "context", "bump", "cas"];
    let cases: [(i64, Decimal, Vec<&str>); 8] = [
        (1, dec!(1000), [&head[..], &["checkpoint", "commit"]].concat()),
        (2, dec!(1050), [&bumped[..], &["open", "commit"]].concat()),
        (3, dec!(1040), [&head[..], &["sight", "commit"]].concat()),
        (4, dec!(1000), [&bumped[..], &["void", "checkpoint", "commit"]].concat()),
        (5, dec!(1050), [&bumped[..], &["open", "commit"]].concat()),
        (6, dec!(960), [&bumped[..], &["void", "open", "commit"]].concat()),
        (7, dec!(1000), [&head[..], &["checkpoint", "sight", "commit"]].concat()),
        (8, dec!(900), [&head[..], &["sight", "commit"]].concat()),
    ];
    for (n, total, expected) in cases {
        let attempt = rig.attempt(&pool, at(n * 60_000), total).await?;
        proxy.enable_exact_query_trace(16);
        rig.writer(&through).conclude(&attempt).await?;
        let trace = proxy.query_trace();
        ensure!(shape(&trace) == expected && trace.len() <= 9, "{n}: {:?}", shape(&trace));
    }
    // Late evidence in both missing sightings, then re-detection: nine; and
    // late evidence voiding the new one, then a clean observation: nine.
    for n in [6, 8] {
        rig.book.fill(&pool, &Fill::foreign(at(n * 60_000 + 100))).await?;
    }
    let late = [
        (9, dec!(900), ["void", "open", "commit"]),
        (10, dec!(1000), ["void", "checkpoint", "commit"]),
    ];
    for (n, total, tail) in late {
        if n == 10 {
            rig.book.fill(&pool, &Fill::foreign(at(9 * 60_000 + 100))).await?;
        }
        let attempt = rig.attempt(&pool, at(n * 60_000), total).await?;
        proxy.enable_exact_query_trace(16);
        let done = rig.writer(&through).conclude(&attempt).await?;
        ensure!(matches!(done.writes[0], Write::Void(_, "late_evidence")), "{n}: {done:?}");
        let trace = proxy.query_trace();
        ensure!(shape(&trace) == [&bumped[..], &tail[..]].concat(), "{n}: {:?}", shape(&trace));
    }

    // PLAN-r4 §7.2: the cardinality, a settled barrier, an open missing
    // discrepancy of 500 sightings, E(cut) over 10,000 fills.
    let big = Rig::bare(&pool, false).await?;
    let counts = bound_fixture(&pool, &big.book).await?;
    let (cut, quote) = (utc("2026-11-20T00:00:00Z"), dec!(69899.2));
    let mut times = Vec::new();
    for n in 0..20 {
        let attempt = big.attempt(&pool, cut + TimeDelta::seconds(n), quote).await?;
        let started = Instant::now();
        let done = big.writer(&through).conclude(&attempt).await?;
        times.push(started.elapsed());
        ensure!(done.delta == dec!(-100) && done.bumped.is_none(), "{done:?}");
    }
    times.sort();
    let slowest = times[times.len() - 1];
    ensure!(slowest < Duration::from_millis(400), "slowest conclusion {slowest:?}");
    let month = NaiveDate::from_ymd_opt(2026, 11, 1).expect("month");
    let plan: Vec<String> = sqlx::query_scalar(&format!("EXPLAIN (ANALYZE, BUFFERS) {}", *CONTEXT))
        .bind(big.book.tenant)
        .bind(VenueId::BINANCE_SPOT)
        .bind(month)
        .bind(Utc::now())
        .bind(vec![SYMBOL])
        .bind(vec!["BTC"])
        .bind(vec!["USDT"])
        .bind(cut + TimeDelta::seconds(30))
        .bind(cut + TimeDelta::seconds(30) + TimeDelta::milliseconds(500))
        .bind(big.epoch)
        .fetch_all(&pool)
        .await?;
    eprintln!(
        "CASH_CONCLUSION_BOUND median_us={} max_us={} counts={counts:?}\n{}",
        times[times.len() / 2].as_micros(),
        slowest.as_micros(),
        plan.join("\n")
    );
    Ok(())
}

// ---- crash points (crash-points builds) ----

#[cfg(feature = "crash-points")]
const CRASH_CHILD: &str = "venue_ledger_store::cash::writer::pg_tests::cash_crash_child";
#[cfg(feature = "crash-points")]
const CRASH_APP: &str = "robson-8l3b-crash";

/// The writer killed at `CASH_AFTER_CONTEXT` and at `CASH_BEFORE_COMMIT`
/// (PLAN-r4 §8 rehearsal points, at the writer level; the loopback rig
/// rehearsal is 8k-3's): no partial write, the rows released by the server,
/// and after a restart the next conclusions land, one open row, progress.
#[cfg(feature = "crash-points")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cash_crash_points_leave_no_partial_write_and_progress(pool: PgPool) -> Result<()> {
    use std::os::unix::process::ExitStatusExt;
    for (n, point) in [(1, "cash_after_context"), (2, "cash_before_commit")] {
        let mut rig = Rig::new(&pool, false).await?;
        rig.conclude(&pool, at(30_000), dec!(1000)).await?;
        let options = pool.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await?
            .next()
            .expect("host");
        let url = database_url_through(
            &std::env::var("DATABASE_URL")?,
            upstream,
            options.get_database().expect("db"),
        )?;
        let marker =
            std::env::temp_dir().join(format!("robson-8l3b-crash-{}-{n}", rig.book.tenant));
        let status = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                CRASH_CHILD,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(robson_exec::crash_points::POINT_ENV, point)
            .env(robson_exec::crash_points::MARKER_FILE_ENV, &marker)
            .env(robson_exec::crash_points::WATCHDOG_ENV, "30")
            .env("ROBSON_TEST_CASH_CRASH_URL", url)
            .env(
                "ROBSON_TEST_CASH_CRASH_IDS",
                format!("{} {} {}", rig.book.tenant, rig.book.sample, rig.epoch),
            )
            .status()?;
        ensure!(status.signal() == Some(9), "{point}: {status:?}");
        ensure!(std::fs::read_to_string(&marker)? == point, "{point}");
        std::fs::remove_file(&marker)?;
        // The server notices the dead client and rolls its transaction back.
        let deadline = Instant::now() + Duration::from_secs(10);
        while sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pg_stat_activity WHERE application_name=$1",
        )
        .bind(CRASH_APP)
        .fetch_one(&pool)
        .await?
            > 0
        {
            ensure!(Instant::now() < deadline, "{point}: the child's session lingers");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        ensure!(
            rig.discrepancies(&pool).await?.is_empty() && rig.version(&pool).await? == 0,
            "{point}"
        );
        ensure!(rig.checkpoint(&pool).await? == Some(at(30_000)), "{point}");
        rig.restart(&pool).await?;
        ensure!(rig.conclude(&pool, at(60_000), dec!(900)).await?.writes == [Write::Open(true)]);
        let done = rig.conclude(&pool, at(90_000), dec!(900)).await?;
        ensure!(matches!(done.writes[..], [Write::Sight(_, 2)]), "{point}: {done:?}");
        ensure!(count_open(&pool, &rig).await? == 1 && rig.version(&pool).await? == 1);
    }
    Ok(())
}

/// The child of the test above: one missing-cash conclusion of the parent's
/// venue, armed to die at the parent's crash point.
#[cfg(feature = "crash-points")]
#[test]
#[ignore = "child process of cash_crash_points_leave_no_partial_write_and_progress"]
fn cash_crash_child() -> Result<()> {
    use std::str::FromStr;

    use sqlx::postgres::PgConnectOptions;
    // Run alone (a suite with `--include-ignored`), it has nothing to do.
    let Ok(url) = std::env::var("ROBSON_TEST_CASH_CRASH_URL") else {
        return Ok(());
    };
    robson_exec::crash_points::arm_from_env();
    let ids = std::env::var("ROBSON_TEST_CASH_CRASH_IDS")?;
    let ids: Vec<Uuid> = ids.split(' ').map(Uuid::parse_str).collect::<Result<_, _>>()?;
    let [tenant, sample, epoch] = ids[..] else {
        anyhow::bail!("three ids")
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let options = PgConnectOptions::from_str(&url)?.application_name(CRASH_APP);
            let pool = PgPoolOptions::new().max_connections(1).connect_with(options).await?;
            let venue = VenueId::new(VenueId::BINANCE_SPOT);
            let cell = GenerationCell::default();
            let accounting =
                BalanceAccounting::for_daemon(AccountKind::BalanceAccount, tenant, &venue, &cell)
                    .expect("a balance account");
            let gate = OrderJournal::new(Arc::new(StubJournalBackend::default()), &venue)
                .gate()
                .clone();
            let version: i64 =
                sqlx::query_scalar("SELECT version FROM accounting_state WHERE tenant_id=$1")
                    .bind(tenant)
                    .fetch_one(&pool)
                    .await?;
            let attempt = CashAttempt {
                verdict: BaseVerdict::Clean { interval: interval(at(60_000)) },
                version,
                marks: gate.invalidity_marks(),
                quote_total: dec!(900),
                u_ns: 150_000_000,
                sample_id: sample,
                epoch_id: epoch,
                received: Instant::now(),
            };
            let writer = CashWriter {
                pool: &pool,
                accounting: &accounting,
                gate: &gate,
                epoch,
                assets: ASSETS,
                read_only: false,
            };
            let result = writer.conclude(&attempt).await;
            anyhow::bail!("the crash point was not reached: {result:?}")
        })
}

/// Every other refusal writes nothing: read-only mode, a verdict other than
/// Clean, a malformed interval, an unsettled leg, an open transition, a
/// stale receipt, an attempt of another epoch, and an impaired E(cut).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn every_refusal_writes_nothing(pool: PgPool) -> Result<()> {
    use crate::balance_reconciliation::UnqualifiedReason;
    let rig = Rig::new(&pool, false).await?;
    let base = rig.attempt(&pool, at(60_000), dec!(900)).await?;
    let read_only = CashWriter { read_only: true, ..rig.writer(&pool) };
    ensure!(matches!(read_only.conclude(&base).await, Err(CashRefusal::ReadOnly)));
    let unqualified = CashAttempt {
        verdict: BaseVerdict::Unqualified(UnqualifiedReason::VersionChanged),
        ..base.clone()
    };
    ensure!(matches!(
        rig.writer(&pool).conclude(&unqualified).await,
        Err(CashRefusal::NotClean)
    ));
    let malformed = CashAttempt { u_ns: -1, ..base.clone() };
    ensure!(matches!(
        rig.writer(&pool).conclude(&malformed).await,
        Err(CashRefusal::Malformed)
    ));
    let stale = CashAttempt {
        received: Instant::now().checked_sub(Duration::from_secs(61)).expect("uptime"),
        ..base.clone()
    };
    ensure!(matches!(rig.writer(&pool).conclude(&stale).await, Err(CashRefusal::Stale)));
    let foreign = CashAttempt { epoch_id: Uuid::now_v7(), ..base.clone() };
    ensure!(matches!(rig.writer(&pool).conclude(&foreign).await, Err(CashRefusal::Epoch)));
    // An unsettled leg (its entry unknown), and an open transition.
    let unsettled = Rig::new(&pool, false).await?;
    sqlx::query("UPDATE balance_legs SET recovery_state='entry_unknown' WHERE position_id=$1")
        .bind(unsettled.book.leg)
        .execute(&pool)
        .await?;
    let refused = unsettled.conclude(&pool, at(60_000), dec!(900)).await;
    ensure!(matches!(refused, Err(CashRefusal::Unsettled)), "{refused:?}");
    let moving = Rig::new(&pool, false).await?;
    sqlx::query("INSERT INTO balance_transitions (transition_id,position_id,tenant_id,venue_id,account_scope,symbol,kind,target_trigger,reason,started_at) VALUES ($1,$2,$3,'binance:spot',$4,$5,'replace',90,'fixture',now())")
        .bind(Uuid::now_v7()).bind(moving.book.leg).bind(moving.book.tenant).bind(&moving.book.scope).bind(SYMBOL)
        .execute(&pool).await?;
    let refused = moving.conclude(&pool, at(60_000), dec!(900)).await;
    ensure!(matches!(refused, Err(CashRefusal::Transition)), "{refused:?}");
    for other in [&unsettled, &moving] {
        ensure!(other.discrepancies(&pool).await?.is_empty() && other.version(&pool).await? == 0);
    }
    // E(cut) impaired: a fill inside the anchor's hull.
    let impaired = Rig::new(&pool, false).await?;
    impaired.book.fill(&pool, &Fill::foreign(at(-5_000))).await?;
    let refused = impaired.conclude(&pool, at(60_000), dec!(900)).await;
    ensure!(matches!(refused, Err(CashRefusal::Undefined)), "{refused:?}");
    for other in [&impaired, &rig] {
        ensure!(other.discrepancies(&pool).await?.is_empty());
        ensure!(other.checkpoint(&pool).await?.is_none() && other.version(&pool).await? == 0);
    }
    ensure!(
        rig.writer(&pool).conclude(&base).await?.writes == [Write::Open(true)],
        "then it lands"
    );
    Ok(())
}

/// PLAN-r4 §6.4 step 3 / Root's decision 4: when the current checkpoint's
/// interval is dirty, a new discrepancy copies the latest clean-hull
/// anchor's as_of and hull instead (earlier: conservative); a checkpoint
/// later than the observation's cut is never copied either.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn dirty_checkpoint_copies_the_latest_clean_hull_anchor(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, false).await?;
    rig.conclude(&pool, at(30_000), dec!(1000)).await?;
    rig.book.fill(&pool, &Fill::foreign(at(30_200))).await?;
    rig.conclude(&pool, at(60_000), dec!(900)).await?;
    let rows = rig.discrepancies(&pool).await?;
    ensure!(rows.len() == 1 && rows[0].5 == at(0), "{rows:?}");
    let (lo, hi): (DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "SELECT checkpoint_interval_start, checkpoint_interval_end FROM cash_discrepancies WHERE tenant_id=$1",
    )
    .bind(rig.book.tenant)
    .fetch_one(&pool)
    .await?;
    ensure!((lo, hi) == (at(-10_000), at(500)), "the anchor's hull: {lo} {hi}");
    // A later scan wrote the checkpoint first: an earlier cut never copies it.
    let late = Rig::new(&pool, false).await?;
    let early = late.attempt(&pool, at(60_000), dec!(900)).await?;
    late.conclude(&pool, at(90_000), dec!(1000)).await?;
    late.writer(&pool).conclude(&early).await?;
    let rows = late.discrepancies(&pool).await?;
    ensure!(rows.len() == 1 && rows[0].5 == at(0), "{rows:?}");
    ensure!(late.checkpoint(&pool).await? == Some(at(90_000)), "never moves back");
    // An earlier clean observation published after a later one leaves the
    // later checkpoint in place.
    let clean = Rig::new(&pool, false).await?;
    let earlier = clean.attempt(&pool, at(60_000), dec!(1000)).await?;
    clean.conclude(&pool, at(90_000), dec!(1000)).await?;
    let done = clean.writer(&pool).conclude(&earlier).await?;
    ensure!(done.writes == [Write::Checkpoint], "{done:?}");
    ensure!(clean.checkpoint(&pool).await? == Some(at(90_000)), "never moves back");
    Ok(())
}

// ---- a successor epoch against a conclusion (Root's decision (c)) ----

/// Another transaction's table lock that blocks a conclusion's write, so the
/// writer holds the accounting rows while a successor's epoch insert starts.
const HOLD_CHECKPOINTS: &str = "LOCK TABLE cash_checkpoints IN EXCLUSIVE MODE";
const HOLD_DISCREPANCIES: &str = "LOCK TABLE cash_discrepancies IN EXCLUSIVE MODE";
/// Blocks an unclean start after its epoch insert, holding its rows.
const HOLD_INCIDENTS: &str = "LOCK TABLE recovery_incidents IN EXCLUSIVE MODE";

async fn epochs(pool: &PgPool, tenant: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM process_epochs WHERE tenant_id=$1")
        .bind(tenant)
        .fetch_one(pool)
        .await?)
}

/// Until a session of this test's database waits on a lock in a statement
/// that starts as `sql` does.
async fn lock_waiter(pool: &PgPool, sql: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database()
                AND wait_event_type = 'Lock' AND starts_with(query, left($1, 60)))",
        )
        .bind(sql)
        .fetch_one(pool)
        .await?;
        if waiting {
            return Ok(());
        }
        ensure!(Instant::now() < deadline, "no session waits in {sql}");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn successor(pool: &PgPool, tenant: Uuid) -> tokio::task::JoinHandle<Result<Uuid>> {
    let pool = pool.clone();
    tokio::spawn(async move {
        let start = process_epoch::start_balance_epoch(&pool, tenant, "successor").await?;
        Ok(start.epoch.epoch_id)
    })
}

/// Proof 1 (Root's decision (c)): a successor's balance epoch insert that
/// starts while a conclusion holds the accounting rows waits until that
/// conclusion commits (a non-bumping checkpoint, a bumping opening) or rolls
/// back (its blocked write past the writer's 300 ms lock bound), and only
/// then lands; the old epoch's next conclusion is refused.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_successor_epoch_waits_for_a_conclusion_holding_the_rows(pool: PgPool) -> Result<()> {
    let cases = [
        ("checkpoint", CHECKPOINT, HOLD_CHECKPOINTS, dec!(1000), true),
        ("opening", OPEN, HOLD_DISCREPANCIES, dec!(900), true),
        ("rolled back", OPEN, HOLD_DISCREPANCIES, dec!(900), false),
    ];
    for (label, write, hold, total, commits) in cases {
        let rig = Rig::new(&pool, true).await?;
        let tenant = rig.book.tenant;
        rig.conclude(&pool, at(30_000), dec!(1000)).await?;
        let attempt = rig.attempt(&pool, at(60_000), total).await?;
        let mut external = pool.begin().await?;
        sqlx::query(hold).execute(&mut *external).await?;
        let writer = rig.writer(&pool);
        let observer = async {
            // The writer holds the rows, blocked in its write; the successor
            // starts and waits for them, with no epoch of its own yet.
            lock_waiter(&pool, write).await?;
            let successor = successor(&pool, tenant);
            lock_waiter(&pool, ACCOUNTING_ROWS).await?;
            ensure!(epochs(&pool, tenant).await? == 1 && !successor.is_finished(), "{label}");
            if commits {
                external.commit().await?;
                return successor.await?;
            }
            // The writer's write gives up at its lock bound and rolls back;
            // the successor lands while the table is still held.
            let landed = successor.await??;
            external.commit().await?;
            Ok::<_, anyhow::Error>(landed)
        };
        let (concluded, landed) = tokio::join!(writer.conclude(&attempt), observer);
        let landed = landed?;
        ensure!(epochs(&pool, tenant).await? == 2 && landed != rig.epoch, "{label}");
        let (open, version) = (rig.discrepancies(&pool).await?.len(), rig.version(&pool).await?);
        match (label, &concluded) {
            ("checkpoint", Ok(_)) => ensure!(rig.checkpoint(&pool).await? == Some(at(60_000))),
            ("opening", Ok(_)) => ensure!(open == 1 && version == 1, "{label}"),
            ("rolled back", Err(CashRefusal::Ledger(_))) => ensure!(open == 0 && version == 0),
            _ => anyhow::bail!("{label}: {concluded:?}"),
        }
        // Whatever it was, the old epoch's next conclusion is refused.
        let refused = rig.conclude(&pool, at(90_000), total).await;
        ensure!(matches!(refused, Err(CashRefusal::Epoch)), "{label}: {refused:?}");
    }
    Ok(())
}

/// Proof 1: a successor whose epoch commits first (before the writer runs,
/// or while the writer waits for the rows it holds) makes the writer refuse
/// its predecessor's attempt with nothing written, bumping or not.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_successor_epoch_committed_first_refuses_the_conclusion(pool: PgPool) -> Result<()> {
    for (total, concurrent) in [
        (dec!(1000), false),
        (dec!(900), false),
        (dec!(1000), true),
        (dec!(900), true),
    ] {
        let rig = Rig::new(&pool, true).await?;
        let tenant = rig.book.tenant;
        rig.conclude(&pool, at(30_000), dec!(1000)).await?;
        let attempt = rig.attempt(&pool, at(60_000), total).await?;
        let refused = match concurrent {
            false => {
                process_epoch::start_balance_epoch(&pool, tenant, "successor").await?;
                rig.writer(&pool).conclude(&attempt).await
            },
            true => {
                // The successor's unclean start holds the rows and its new
                // epoch, blocked before its incident; the writer waits for
                // the rows; the successor commits.
                let mut external = pool.begin().await?;
                sqlx::query(HOLD_INCIDENTS).execute(&mut *external).await?;
                let successor = successor(&pool, tenant);
                lock_waiter(&pool, "INSERT INTO recovery_incidents").await?;
                let writer = rig.writer(&pool);
                let release = async {
                    lock_waiter(&pool, ACCOUNTING_ROWS).await?;
                    external.commit().await?;
                    successor.await?
                };
                let (refused, landed) = tokio::join!(writer.conclude(&attempt), release);
                landed?;
                refused
            },
        };
        ensure!(matches!(refused, Err(CashRefusal::Epoch)), "{total} {concurrent}: {refused:?}");
        ensure!(epochs(&pool, tenant).await? == 2);
        ensure!(rig.discrepancies(&pool).await?.is_empty());
        ensure!(rig.checkpoint(&pool).await? == Some(at(30_000)) && rig.version(&pool).await? == 0);
    }
    Ok(())
}

/// Proof 3: USD-M's epoch start issues exactly the statements it always did
/// (an unclean start, then a clean one); a balance daemon's issues the same
/// texts with one more, the tenant's accounting rows right after `BEGIN`.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_balance_epoch_start_adds_only_the_row_lock(pool: PgPool) -> Result<()> {
    let (proxy, through) = proxied(&pool).await?;
    let words = |trace: &[String]| -> Vec<String> {
        let head = |sql: &String| sql.split_whitespace().take(4).collect::<Vec<_>>().join(" ");
        trace.iter().map(head).collect()
    };
    let unclean = [
        "BEGIN",
        "SELECT epoch_id, epoch_seq, marker_version",
        "INSERT INTO process_epochs (epoch_id,",
        "UPDATE order_intents SET uncertain_generation",
        "INSERT INTO recovery_incidents (incident_id,",
        "SELECT reason, first_at, last_at,",
        "COMMIT",
    ];
    let clean = [unclean[0], unclean[1], unclean[2], unclean[5], unclean[6]];
    let (usdm, balance) = (Uuid::now_v7(), Book::new(&pool).await?.tenant);
    for (round, expected) in [&unclean[..], &clean[..]].into_iter().enumerate() {
        if round == 1 {
            sqlx::query("UPDATE process_epochs SET clean_stop_at = now(), marker_version = $2 WHERE tenant_id = ANY($1)")
                .bind(vec![usdm, balance])
                .bind(process_epoch::MARKER_VERSION)
                .execute(&pool)
                .await?;
        }
        proxy.enable_exact_query_trace(16);
        process_epoch::start_epoch(&through, usdm, "usdm").await?;
        let usdm_trace = proxy.query_trace();
        ensure!(words(&usdm_trace) == expected, "USD-M {round}: {usdm_trace:?}");
        proxy.enable_exact_query_trace(16);
        let started = process_epoch::start_balance_epoch(&through, balance, "spot").await?;
        ensure!(started.predecessor.unclean_reason().is_none() == (round == 1));
        let mut balance_trace = proxy.query_trace();
        ensure!(
            balance_trace.len() == usdm_trace.len() + 1,
            "balance {round}: {balance_trace:?}"
        );
        ensure!(balance_trace.remove(1) == ACCOUNTING_ROWS && balance_trace == usdm_trace);
    }
    Ok(())
}

/// Proof 4: the balance epoch insert waits for the rows no longer than the
/// balance pool's session lock bound (2 s): past it the start is refused, no
/// epoch is written, nothing hangs, and a later start lands.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_balance_epoch_wait_is_bounded_and_fails_closed(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, false).await?;
    let tenant = rig.book.tenant;
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .expect("host");
    let url = database_url_through(
        &std::env::var("DATABASE_URL")?,
        upstream,
        options.get_database().expect("db"),
    )?;
    let bounds = crate::db_session::SessionBounds::BALANCE;
    let daemon =
        crate::db_session::connect_daemon_pool_with(&url, AccountKind::BalanceAccount, bounds)
            .await?;
    let mut holder = pool.begin().await?;
    sqlx::query(ACCOUNTING_ROWS).bind(tenant).fetch_all(&mut *holder).await?;
    let started = Instant::now();
    let refused = process_epoch::start_balance_epoch(&daemon, tenant, "blocked").await;
    let waited = started.elapsed();
    ensure!(
        refused.as_ref().is_err_and(|e| e.to_string().contains("lock timeout")),
        "{refused:?}"
    );
    let bound = Duration::from_millis(bounds.lock_timeout_ms);
    ensure!(waited >= bound && waited < bound + Duration::from_secs(2), "{waited:?}");
    ensure!(epochs(&pool, tenant).await? == 1, "no epoch written");
    holder.rollback().await?;
    process_epoch::start_balance_epoch(&daemon, tenant, "after").await?;
    ensure!(epochs(&pool, tenant).await? == 2);
    Ok(())
}

/// Proof 2: the writer reads `process_epochs`, and everything else in its
/// context, without a row lock; the only lock it shares with the epoch
/// insert is the accounting rows, which both take first by this statement
/// (the traces above), in (tenant, venue) order.
#[test]
fn the_context_read_locks_no_row() {
    for clause in [
        "FOR UPDATE",
        "FOR NO KEY UPDATE",
        "FOR SHARE",
        "FOR KEY SHARE",
    ] {
        assert!(!CONTEXT.to_uppercase().contains(clause), "{clause}");
    }
    assert!(CONTEXT.contains("process_epochs"));
    assert!(ACCOUNTING_ROWS.ends_with("WHERE tenant_id = $1 ORDER BY venue_id FOR UPDATE"));
}

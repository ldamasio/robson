//! 8l-5b proofs on PostgreSQL (PLAN-r4 §8 row 8l-5): the one transaction's
//! statements by proxy; W, C and their dates (the anchor floor, the effective
//! dating copy, O1 and O2 in the minimum, C before the new anchor); `E(t)`
//! continuous and equal to the wallet after every re-anchor, over generated
//! sequences of movements, scans, failed reads, re-anchors and a boundary
//! (R14); no flow for Δ within 0.01; the excluded dirty sightings; two
//! re-anchors then late evidence on the first, at the exact cuts; every
//! refusal writing nothing; a lost COMMIT answer and its retry; the recovery
//! of an impaired anchor (Q2 = R) with its invalid months; and the writer-level
//! rehearsal and crash across B (R13-1).
use std::sync::Arc;

use anyhow::{ensure, Context as _, Result};
use chrono::TimeDelta;
use robson_exec::{
    gate::{IncidentReason, PendingEvidence},
    order_journal::{OrderJournal, StubJournalBackend},
    venue::VenueId,
};
use robson_testkit::{
    daemon_process::database_url_through,
    db_proxy::{DbProxy, ProxyMode},
};
use rust_decimal_macros::dec;
use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::time::Instant;

use super::{
    super::{AccountReceipt, CashReanchor, Evidence, Observation, Observed, ReanchorVenue},
    *,
};
use crate::{
    balance_accounting::{BalanceAccounting, GenerationCell},
    balance_reconciliation::{AccountReading, BaseVerdict, ObservationInterval},
    process_epoch,
    spot_order_set_store::JournalGeneration,
    venue_ledger_store::{
        cash::{
            pg_tests::{at, bound_fixture, micro, utc, Book, Fill, ASSETS},
            writer::{CashAttempt, CashWriter},
            ExpectedQuote,
        },
        settlement::{read_settlement, SettlementQuery},
    },
};

/// The transaction never asks the venue.
struct Unused;

#[async_trait::async_trait]
impl ReanchorVenue for Unused {
    async fn account(&self) -> Option<AccountReceipt> {
        None
    }

    fn revalidate(&self, _: &AccountReceipt) -> bool {
        false
    }
}

/// One balance-account daemon's re-anchor on 8l-3a's fixture venue: an
/// observed anchor of 1000 at `at(0)` on [-10 s, +500 ms] (unless bare), an
/// installed order-set generation when opted.
struct Rig {
    book: Book,
    reanchor: CashReanchor,
}

impl Rig {
    async fn new(pool: &PgPool, opted: bool) -> Result<Self> {
        let rig = Self::bare(pool, opted).await?;
        rig.book.anchor(pool, at(0), at(-10_000), dec!(1000)).await?;
        Ok(rig)
    }

    async fn bare(pool: &PgPool, opted: bool) -> Result<Self> {
        let book = Book::new(pool).await?;
        let venue = VenueId::new(VenueId::BINANCE_SPOT);
        let cell = GenerationCell::default();
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
        let accounting = BalanceAccounting {
            tenant_id: book.tenant,
            venue: venue.clone(),
            account_scope: book.scope.clone(),
            generation: cell,
        };
        let gate = OrderJournal::new(Arc::new(StubJournalBackend::default()), &venue)
            .gate()
            .clone();
        let reanchor = CashReanchor {
            pool: pool.clone(),
            accounting,
            gate,
            epoch: book.epoch,
            uid: "4242".into(),
            pair: ASSETS[0],
            venue: Arc::new(Unused),
            read_only: false,
            flight: Default::default(),
        };
        Ok(Self { book, reanchor })
    }

    /// An observation of `total` on [cut, cut + 500 ms], u = 150 ms.
    fn observation(&self, cut: DateTime<Utc>, total: Decimal) -> Observation {
        let start = i128::from(cut.timestamp_micros()) * 1_000;
        let interval = ObservationInterval {
            start_ns: start,
            end_ns: start + 500_000_000,
            cut_ns: start,
        };
        let reading = AccountReading {
            interval,
            u_ns: 150_000_000,
            sample_id: self.book.sample,
            epoch_id: self.book.epoch,
            quote_total: Some(total),
        };
        let observed = Observed::new(interval, 150_000_000).expect("observation");
        Observation { reading, observed, total }
    }

    /// O1 at `cut` and O2 6 s later, `v₀` and the marks read now.
    async fn evidence(
        &self,
        pool: &PgPool,
        cut: DateTime<Utc>,
        totals: [Decimal; 2],
    ) -> Result<Evidence> {
        Ok(Evidence {
            o1: self.observation(cut, totals[0]),
            o2: self.observation(cut + TimeDelta::seconds(6), totals[1]),
            version: self.version(pool).await?,
            marks: self.reanchor.gate.invalidity_marks(),
            requested: Instant::now(),
        })
    }

    async fn commit(&self, evidence: &Evidence) -> Result<Reanchored, ReanchorRefusal> {
        self.reanchor
            .commit("operator@example.test", "fixture movement", evidence)
            .await
    }

    /// A re-anchor whose O1 is at `cut` and O2 6 s later, both reading
    /// `total`.
    async fn reanchor(
        &self,
        pool: &PgPool,
        cut: DateTime<Utc>,
        total: Decimal,
    ) -> Result<Reanchored> {
        let evidence = self.evidence(pool, cut, [total, total]).await?;
        self.commit(&evidence).await.map_err(|refusal| anyhow::anyhow!("{refusal:?}"))
    }

    /// A clean scan's conclusion through 8l-3b's writer (8k-3's role).
    async fn scan(&self, pool: &PgPool, cut: DateTime<Utc>, total: Decimal) -> Result<()> {
        let start = i128::from(cut.timestamp_micros()) * 1_000;
        let interval = ObservationInterval {
            start_ns: start,
            end_ns: start + 500_000_000,
            cut_ns: start,
        };
        let attempt = CashAttempt {
            verdict: BaseVerdict::Clean { interval },
            version: self.version(pool).await?,
            marks: self.reanchor.gate.invalidity_marks(),
            quote_total: total,
            u_ns: 150_000_000,
            sample_id: self.book.sample,
            epoch_id: self.book.epoch,
            received: std::time::Instant::now(),
        };
        let writer = CashWriter {
            pool,
            accounting: &self.reanchor.accounting,
            gate: &self.reanchor.gate,
            epoch: self.reanchor.epoch,
            assets: ASSETS,
            read_only: false,
        };
        writer
            .conclude(&attempt)
            .await
            .map_err(|refusal| anyhow::anyhow!("{refusal:?}"))?;
        Ok(())
    }

    async fn version(&self, pool: &PgPool) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT version FROM accounting_state WHERE tenant_id=$1")
            .bind(self.book.tenant)
            .fetch_one(pool)
            .await?)
    }

    /// Every attested flow: direction, amount and date, by date.
    async fn flows(&self, pool: &PgPool) -> Result<Vec<(String, Decimal, DateTime<Utc>)>> {
        Ok(sqlx::query_as("SELECT direction, amount, economic_date FROM cash_attested_flows WHERE tenant_id=$1 ORDER BY economic_date, direction")
            .bind(self.book.tenant)
            .fetch_all(pool)
            .await?)
    }

    /// Anchors, re-anchors, flows, open discrepancies and the version: what
    /// a refusal must leave unchanged.
    async fn written(&self, pool: &PgPool) -> Result<(i64, i64, i64, i64, i64)> {
        Ok(sqlx::query_as("SELECT (SELECT count(*) FROM cash_anchors WHERE tenant_id=$1), (SELECT count(*) FROM cash_reanchors WHERE tenant_id=$1),
                (SELECT count(*) FROM cash_attested_flows WHERE tenant_id=$1), (SELECT count(*) FROM cash_discrepancies WHERE tenant_id=$1 AND state='open'),
                (SELECT version FROM accounting_state WHERE tenant_id=$1)")
            .bind(self.book.tenant)
            .fetch_one(pool)
            .await?)
    }

    /// The last re-anchor's mode, old anchor, dating copy and excluded seqs.
    async fn last(
        &self,
        pool: &PgPool,
    ) -> Result<(String, Uuid, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Vec<i32>)> {
        Ok(sqlx::query_as("SELECT mode, old_anchor_id, dating_cut, dating_interval_start, excluded_sighting_seqs FROM cash_reanchors WHERE tenant_id=$1 ORDER BY o2_interval_start DESC LIMIT 1")
            .bind(self.book.tenant)
            .fetch_one(pool)
            .await?)
    }

    async fn discrepancy_state(&self, pool: &PgPool, id: Uuid) -> Result<(String, Option<String>)> {
        Ok(sqlx::query_as(
            "SELECT state, void_reason FROM cash_discrepancies WHERE discrepancy_id=$1",
        )
        .bind(id)
        .fetch_one(pool)
        .await?)
    }

    async fn expected(&self, pool: &PgPool, t: DateTime<Utc>) -> Result<ExpectedQuote> {
        self.book.expected(pool, t).await
    }
}

fn value(quote: ExpectedQuote) -> Option<Decimal> {
    quote.value
}

/// A pool through a counting proxy.
async fn proxied(pool: &PgPool) -> Result<(DbProxy, PgPool)> {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .context("host")?;
    let proxy = DbProxy::start(upstream).await?;
    let database = options.get_database().context("database")?;
    let url = database_url_through(&std::env::var("DATABASE_URL")?, proxy.addr(), database)?;
    Ok((proxy, PgPoolOptions::new().max_connections(1).connect_lazy(&url)?))
}

/// A withdrawal of 100 seen by O1 and O2: one transaction of seven
/// statements (six without an installed generation: no CAS) on one
/// connection; W = 100 dated at the old anchor (no checkpoint: the clean-hull
/// anchor's copy), C = 0 and no contribution row; the new anchor and
/// checkpoint at O2's cut; `E` continuous at O2's cut and equal to the
/// wallet after; the version and the installed generation advanced once.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn reanchor_books_a_withdrawal_and_keeps_e_continuous(pool: PgPool) -> Result<()> {
    for (opted, statements) in [(true, 7), (false, 6)] {
        let mut rig = Rig::new(&pool, opted).await?;
        let (proxy, through) = proxied(&pool).await?;
        rig.reanchor.pool = through;
        let evidence = rig.evidence(&pool, at(60_000), [dec!(900), dec!(900)]).await?;
        let start = proxy.stats();
        let done = rig.commit(&evidence).await.map_err(|r| anyhow::anyhow!("{r:?}"))?;
        let end = proxy.stats();
        let counted = (end.statements - start.statements, end.connections - start.connections);
        ensure!(counted == (statements, 1), "opted={opted}: {counted:?}");
        ensure!(
            (
                done.mode,
                done.version,
                done.delta,
                done.withdrawal,
                done.contribution,
                done.flows
            ) == ("attested", 1, Some(dec!(-100)), Some(dec!(100)), Some(dec!(0)), 1),
            "{done:?}"
        );
        ensure!(rig.flows(&pool).await? == [("withdrawal".to_string(), dec!(100), at(0))]);
        let cut = at(66_000);
        ensure!(
            value(rig.expected(&pool, cut).await?) == Some(dec!(900)),
            "continuous at O2's cut"
        );
        let after = rig.expected(&pool, cut + micro()).await?;
        ensure!(after.value == Some(dec!(900)) && after.anchor.map(|a| a.1) == Some(cut));
        ensure!(value(rig.expected(&pool, at(600_000)).await?) == Some(dec!(900)));
        let checkpoint: (DateTime<Utc>, String) =
            sqlx::query_as("SELECT cut, source FROM cash_checkpoints WHERE tenant_id=$1")
                .bind(rig.book.tenant)
                .fetch_one(&pool)
                .await?;
        ensure!(checkpoint == (cut, "reanchor".to_string()));
        ensure!(rig.version(&pool).await? == 1);
        if opted {
            let generation: i64 = sqlx::query_scalar(
                "SELECT accounting_version FROM spot_order_set_generation WHERE tenant_id=$1",
            )
            .bind(rig.book.tenant)
            .fetch_one(&pool)
            .await?;
            ensure!(generation == 1, "the generation advanced with the bump");
        }
        println!("REANCHOR_TX_COUNTS opted={opted} statements={} connections=1", counted.0);
    }
    Ok(())
}

/// R13-2: O1's and O2's Δ join the minimum with the open discrepancy's
/// clean one (-60): W = 120 whichever of O1 and O2 saw the lower total, C =
/// Δ(O2) + W, W dated at the discrepancy's checkpoint copy, C at O1's end,
/// the discrepancy resolved.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn reanchor_minimum_includes_o1_and_o2(pool: PgPool) -> Result<()> {
    for (totals, contribution) in [
        ([dec!(880), dec!(880.005)], Some(dec!(0.005))),
        ([dec!(880.005), dec!(880)], None),
    ] {
        let rig = Rig::new(&pool, false).await?;
        let open = rig
            .book
            .discrepancy(&pool, "open", at(10_000), &[(at(20_000), dec!(-60))])
            .await?;
        let evidence = rig.evidence(&pool, at(60_000), totals).await?;
        let done = rig.commit(&evidence).await.map_err(|r| anyhow::anyhow!("{r:?}"))?;
        ensure!(done.withdrawal == Some(dec!(120)), "{totals:?}: {done:?}");
        let mut expected = vec![("withdrawal".to_string(), dec!(120), at(10_000))];
        if let Some(c) = contribution {
            expected.push(("contribution".to_string(), c, at(60_500)));
        }
        ensure!(rig.flows(&pool).await? == expected, "{totals:?}");
        ensure!(rig.discrepancy_state(&pool, open).await?.0 == "resolved");
        ensure!(value(rig.expected(&pool, at(66_000)).await?) == Some(totals[1]));
    }
    Ok(())
}

/// v16:551-552: with no open discrepancy and |Δ(O1)|, |Δ(O2)| ≤ 0.01 the
/// re-anchor refreshes the anchor and the checkpoint and writes no flow; one
/// cent more writes it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn no_movement_reanchor_writes_no_flow(pool: PgPool) -> Result<()> {
    for (totals, flows) in [
        ([dec!(1000.005), dec!(999.995)], 0),
        ([dec!(1000), dec!(1000)], 0),
        ([dec!(1000.01), dec!(1000.01)], 0),
        ([dec!(999.99), dec!(999.99)], 0),
        ([dec!(1000.011), dec!(1000.011)], 1),
        ([dec!(999.995), dec!(999.989)], 1),
    ] {
        let rig = Rig::new(&pool, false).await?;
        let evidence = rig.evidence(&pool, at(60_000), totals).await?;
        let done = rig.commit(&evidence).await.map_err(|r| anyhow::anyhow!("{r:?}"))?;
        ensure!(
            done.flows == flows && rig.flows(&pool).await?.len() == flows as usize,
            "{totals:?}"
        );
        let after = rig.expected(&pool, at(66_000) + micro()).await?;
        ensure!(after.value == Some(totals[1]) && rig.version(&pool).await? == 1, "{totals:?}");
    }
    Ok(())
}

/// R13-1: W is dated no earlier than the latest observed anchor, even when
/// the discrepancy's stored checkpoint copy is older; the row keeps that
/// copy; `E` is continuous at O2's cut.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn reanchor_dates_w_no_earlier_than_the_latest_observed_anchor(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, false).await?;
    rig.book
        .discrepancy(&pool, "open", at(-20_000), &[(at(20_000), dec!(-100))])
        .await?;
    rig.reanchor(&pool, at(60_000), dec!(900)).await?;
    ensure!(rig.flows(&pool).await? == [("withdrawal".to_string(), dec!(100), at(0))]);
    let (_, _, dating, _, _) = rig.last(&pool).await?;
    ensure!(dating == Some(at(-20_000)), "{dating:?}");
    ensure!(value(rig.expected(&pool, at(66_000)).await?) == Some(dec!(900)));
    Ok(())
}

/// C is dated at O1's end, before the new anchor, so the anchor never counts
/// it twice: `E` after O2 is the wallet.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn reanchor_contribution_precedes_the_new_anchor(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, false).await?;
    let done = rig.reanchor(&pool, at(60_000), dec!(1100)).await?;
    ensure!((done.withdrawal, done.contribution) == (Some(dec!(0)), Some(dec!(100))));
    ensure!(rig.flows(&pool).await? == [("contribution".to_string(), dec!(100), at(60_500))]);
    ensure!(value(rig.expected(&pool, at(66_000)).await?) == Some(dec!(1100)));
    ensure!(value(rig.expected(&pool, at(600_000)).await?) == Some(dec!(1100)));
    Ok(())
}

/// A discrepancy's checkpoint copy made dirty by a later-booked fill is
/// replaced by the latest clean-hull anchor's `as_of` and hull, and the row
/// stores that effective copy.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn reanchor_stores_the_effective_clean_dating_copy(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, false).await?;
    rig.book
        .discrepancy(&pool, "open", at(10_000), &[(at(20_000), dec!(-100))])
        .await?;
    rig.book.fill(&pool, &Fill::foreign(at(10_200))).await?;
    rig.reanchor(&pool, at(60_000), dec!(900)).await?;
    let (_, _, dating, start, _) = rig.last(&pool).await?;
    ensure!((dating, start) == (Some(at(0)), Some(at(-10_000))), "{dating:?} {start:?}");
    ensure!(rig.flows(&pool).await? == [("withdrawal".to_string(), dec!(100), at(0))]);
    Ok(())
}

/// A sighting already dirty at the re-anchor's commit is excluded from its
/// minimum and recorded, so the (R) rule never reads it as evidence booked
/// after the re-anchor: `E` stays defined.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn sighting_dirty_before_the_reanchor_does_not_impair_it(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, false).await?;
    let sightings = [(at(20_000), dec!(-100)), (at(30_000), dec!(-150))];
    rig.book.discrepancy(&pool, "open", at(10_000), &sightings).await?;
    rig.book.fill(&pool, &Fill::foreign(at(30_100))).await?;
    let done = rig.reanchor(&pool, at(60_000), dec!(900)).await?;
    ensure!(done.withdrawal == Some(dec!(100)), "the dirty -150 is excluded: {done:?}");
    ensure!(rig.last(&pool).await?.4 == [2]);
    // W is dated at the copy, at(10_000).
    for (t, e) in [
        (at(0) + micro(), dec!(1000)),
        (at(30_000), dec!(900)),
        (at(66_000), dec!(900)),
    ] {
        let quote = rig.expected(&pool, t).await?;
        ensure!(!quote.impaired && quote.value == Some(e), "{t}: {quote:?}");
    }
    Ok(())
}

/// R1 resolves D1, R2 follows. A fill booked later inside a sighting R1
/// used impairs exactly (A0.as_of, A1.as_of] (the (R) rule; A0.as_of itself
/// defined), never R2's span; in A1's own hull it impairs (A1.as_of,
/// A2.as_of] (the (A) rule; A2.as_of included, just after it defined) and,
/// R1's anchor hull being dirty, (A0.as_of, A1.as_of]. Read on a fresh
/// connection (restart); R2 and its flows untouched; nothing written.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn two_reanchors_then_late_evidence_on_the_first_after_restart(pool: PgPool) -> Result<()> {
    let (a0, a1, a2) = (at(0), at(66_000), at(126_000));
    for late in [at(20_100), at(63_000)] {
        let rig = Rig::new(&pool, false).await?;
        rig.book
            .discrepancy(&pool, "open", at(10_000), &[(at(20_000), dec!(-100))])
            .await?;
        rig.reanchor(&pool, at(60_000), dec!(900)).await?;
        rig.reanchor(&pool, at(120_000), dec!(950)).await?;
        let before = rig.written(&pool).await?;
        rig.book.fill(&pool, &Fill::foreign(late)).await?;
        // Restart: every read below is a new process's, on a new pool.
        let restarted = PgPoolOptions::new()
            .max_connections(1)
            .connect_with((*pool.connect_options()).clone())
            .await?;
        let (r, p) = (&rig, &restarted);
        let impaired = |t| async move { Ok::<_, anyhow::Error>(r.expected(p, t).await?.impaired) };
        ensure!(!impaired(a0).await?, "{late}: the replaced anchor's cut is outside the span");
        ensure!(impaired(a0 + micro()).await? && impaired(a1).await?, "{late}: (A0, A1]");
        let hull = late == at(63_000);
        ensure!(impaired(a1 + micro()).await? == hull && impaired(a2).await? == hull, "{late}");
        let after = rig.expected(&restarted, a2 + micro()).await?;
        ensure!(!after.impaired && after.value == Some(dec!(950)), "{late}: {after:?}");
        ensure!(rig.written(&restarted).await? == before, "reads write nothing");
        ensure!(rig.flows(&restarted).await?.len() == 2, "R1's W and R2's C stand");
        restarted.close().await;
    }
    Ok(())
}

/// Every refusal of the transaction writes nothing: not at rest (an open
/// transition, a live order that is not a stop, an uncertain intent), the
/// version moved, a dirty hull, no anchor, an anchor between the cuts, an
/// impairment that is not the anchor's own hull, a stale or other epoch, the
/// marks moved or pending, publication after 60 s, a stale generation.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn reanchor_refusals_write_nothing(pool: PgPool) -> Result<()> {
    use ReanchorRefusal as R;
    let cases: [(&str, R); 14] = [
        ("transition", R::NotAtRest),
        ("live order", R::NotAtRest),
        ("uncertain intent", R::NotAtRest),
        ("version", R::VersionMoved),
        ("dirty hull", R::DirtyHull),
        ("no anchor", R::NoAnchor),
        ("overlap", R::AnchorOverlap),
        ("impaired chain", R::ImpairedChain),
        ("successor epoch", R::Epoch),
        ("other process", R::Epoch),
        ("occurrence", R::Marks),
        ("pending", R::Marks),
        ("stale", R::Stale),
        ("generation", R::StaleGeneration),
    ];
    for (case, expected) in cases {
        let rig = match case {
            "no anchor" => Rig::bare(&pool, false).await?,
            "generation" => Rig::new(&pool, true).await?,
            _ => Rig::new(&pool, false).await?,
        };
        let mut rig = rig;
        let (tenant, scope) = (rig.book.tenant, rig.book.scope.clone());
        match case {
            "transition" => {
                sqlx::query("INSERT INTO balance_transitions (transition_id,position_id,tenant_id,venue_id,account_scope,symbol,kind,target_trigger,reason,started_at) VALUES ($1,$2,$3,'binance:spot',$4,'BTCUSDT','reprotect',90,'fixture',now())")
                    .bind(Uuid::now_v7()).bind(rig.book.leg).bind(tenant).bind(&scope).execute(&pool).await?;
            },
            "live order" => {
                sqlx::query("INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,side,order_type,orig_qty,status,phase,last_checked_at,live_seen_at) VALUES ('binance:spot',$1,'BTCUSDT','8801',$2,'web-1','web-1','discovered','BUY','LIMIT',1,'NEW','live',now(),now())")
                    .bind(&scope).bind(tenant).execute(&pool).await?;
            },
            "uncertain intent" => {
                sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at,uncertain_at,uncertain_generation) VALUES ($1,$2,'binance:spot',$3,$4,'cancel','{}',now(),now(),1)")
                    .bind(Uuid::now_v7()).bind(tenant).bind(&scope).bind(Uuid::now_v7().simple().to_string())
                    .execute(&pool).await?;
            },
            "dirty hull" => rig.book.fill(&pool, &Fill::foreign(at(63_000))).await?,
            "overlap" => {
                rig.book.anchor(&pool, at(63_000), at(62_000), dec!(1000)).await?;
            },
            "impaired chain" => {
                // A later re-anchor whose span covers both cuts and whose
                // dating copy holds a late fill; E's own anchor stays clean.
                let future = rig.book.anchor(&pool, at(200_000), at(190_000), dec!(1000)).await?;
                let old = rig.expected(&pool, at(1)).await?.anchor.context("A0")?.0;
                rig.book
                    .reanchor(&pool, old, future, at(200_000), at(40_000), None, &[])
                    .await?;
                rig.book.fill(&pool, &Fill::foreign(at(40_100))).await?;
            },
            "successor epoch" => {
                process_epoch::start_epoch(&pool, tenant, "8l-5b-successor").await?;
            },
            "other process" => rig.reanchor.epoch = Uuid::now_v7(),
            _ => {},
        }
        let mut evidence = rig.evidence(&pool, at(60_000), [dec!(900), dec!(900)]).await?;
        match case {
            "version" => evidence.version -= 1,
            "occurrence" => {
                rig.reanchor.gate.record(IncidentReason::VenueDiscrepancy);
            },
            "pending" => evidence.marks.pending = PendingEvidence::Pending,
            "stale" => {
                evidence.requested = Instant::now()
                    .checked_sub(std::time::Duration::from_secs(61))
                    .context("instant")?;
            },
            "generation" => {
                sqlx::query("UPDATE spot_order_set_generation SET accounting_version = 7 WHERE tenant_id=$1")
                    .bind(tenant)
                    .execute(&pool)
                    .await?;
            },
            _ => {},
        }
        let before = rig.written(&pool).await?;
        let refused = rig.commit(&evidence).await;
        ensure!(refused == Err(expected), "{case}: {refused:?}");
        ensure!(rig.written(&pool).await? == before, "{case}: a refusal writes nothing");
    }
    Ok(())
}

/// A discrepancy with no surviving clean sighting of its direction is voided
/// by late evidence, never resolved, and counts as none open: its dirty
/// minimum is not W's; the copy is the clean-hull anchor's.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_discrepancy_without_surviving_evidence_is_voided(pool: PgPool) -> Result<()> {
    let rig = Rig::new(&pool, false).await?;
    let open = rig
        .book
        .discrepancy(&pool, "open", at(10_000), &[(at(20_000), dec!(-300))])
        .await?;
    rig.book.fill(&pool, &Fill::foreign(at(20_100))).await?;
    let done = rig.reanchor(&pool, at(60_000), dec!(900)).await?;
    ensure!(done.withdrawal == Some(dec!(100)), "{done:?}");
    ensure!(
        rig.discrepancy_state(&pool, open).await? == ("void".into(), Some("late_evidence".into()))
    );
    let voided: (Option<Uuid>, Option<Uuid>, Vec<i32>) = sqlx::query_as("SELECT resolved_discrepancy_id, voided_discrepancy_id, excluded_sighting_seqs FROM cash_reanchors WHERE tenant_id=$1")
        .bind(rig.book.tenant).fetch_one(&pool).await?;
    ensure!(voided == (None, Some(open), vec![]), "{voided:?}");
    ensure!(rig.flows(&pool).await? == [("withdrawal".to_string(), dec!(100), at(0))]);
    Ok(())
}

/// A lost COMMIT answer is `uncertain`; the transaction did commit, and the
/// retry (a new O1/O2 at the new version) sees Δ = 0 and writes no flow.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_lost_commit_answer_is_uncertain_and_the_retry_writes_no_flow(
    pool: PgPool,
) -> Result<()> {
    let mut rig = Rig::new(&pool, false).await?;
    let (proxy, through) = proxied(&pool).await?;
    rig.reanchor.pool = through;
    let evidence = rig.evidence(&pool, at(60_000), [dec!(900), dec!(900)]).await?;
    proxy.set_mode(ProxyMode::DropCommitAck);
    ensure!(rig.commit(&evidence).await == Err(ReanchorRefusal::Uncertain));
    ensure!(rig.version(&pool).await? == 1, "the first did commit");
    let retry = rig.reanchor(&pool, at(120_000), dec!(900)).await?;
    ensure!((retry.delta, retry.flows) == (Some(dec!(0)), 0), "{retry:?}");
    ensure!(rig.flows(&pool).await? == [("withdrawal".to_string(), dec!(100), at(0))]);
    ensure!(rig.version(&pool).await? == 2);
    Ok(())
}

/// Operator Q2 = R: E's own anchor (A1) has a hull dirty by late evidence,
/// so `E` is impaired at O1 and O2 and the re-anchor recovers: O1/O2 as
/// usual, a new anchor and checkpoint, the open discrepancy resolved, no
/// flow, no dating copy, no amounts, the old anchor the latest clean-hull one
/// (A0). The months from A0's through the recovery's are invalid (October
/// and November), September and December are not; `E` is valid again after
/// the recovery's anchor. With no clean-hull anchor at all, the old anchor is
/// the first and the span starts at its month.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn recovery_reanchor_writes_no_flow_and_invalidates_the_uncertain_months(
    pool: PgPool,
) -> Result<()> {
    let (a0, a1) = (utc("2026-10-20T12:00:00Z"), utc("2026-11-05T12:00:00Z"));
    let o1 = utc("2026-11-10T12:00:00Z");
    for with_clean in [true, false] {
        let rig = Rig::bare(&pool, false).await?;
        let first = match with_clean {
            true => {
                Some(rig.book.anchor(&pool, a0, a0 - TimeDelta::seconds(10), dec!(1000)).await?)
            },
            false => None,
        };
        let dirty = rig.book.anchor(&pool, a1, a1 - TimeDelta::seconds(10), dec!(1000)).await?;
        let open = rig
            .book
            .discrepancy(&pool, "open", a1 + TimeDelta::days(1), &[(
                a1 + TimeDelta::days(2),
                dec!(-100),
            )])
            .await?;
        rig.book.fill(&pool, &Fill::foreign(a1 - TimeDelta::seconds(5))).await?;
        ensure!(rig.expected(&pool, o1).await?.impaired, "E_old is impaired by (A)");
        let done = rig.reanchor(&pool, o1, dec!(700)).await?;
        ensure!(
            (done.mode, done.delta, done.withdrawal, done.contribution, done.flows)
                == ("recovery", None, None, None, 0),
            "{done:?}"
        );
        let (mode, old, dating, _, _) = rig.last(&pool).await?;
        ensure!((mode.as_str(), old, dating) == ("recovery", first.unwrap_or(dirty), None));
        ensure!(rig.flows(&pool).await?.is_empty());
        ensure!(rig.discrepancy_state(&pool, open).await?.0 == "resolved");
        let cut = o1 + TimeDelta::seconds(6);
        let after = rig.expected(&pool, cut + micro()).await?;
        ensure!(!after.impaired && after.value == Some(dec!(700)), "{after:?}");
        ensure!(rig.expected(&pool, a1 + TimeDelta::days(1)).await?.impaired);
        let venue = VenueId::new(VenueId::BINANCE_SPOT);
        let mut conn = pool.acquire().await?;
        let mut invalid = Vec::new();
        for month in ["2026-09-01", "2026-10-01", "2026-11-01", "2026-12-01"] {
            let query = SettlementQuery {
                tenant_id: rig.book.tenant,
                venue: &venue,
                month: month.parse()?,
                now: utc("2027-01-15T00:00:00Z"),
                assets: ASSETS,
            };
            invalid.push(read_settlement(&mut conn, &query).await?.invalidations > 0);
        }
        ensure!(
            invalid == [false, with_clean, true, false],
            "with_clean={with_clean}: {invalid:?}"
        );
    }
    Ok(())
}

/// v16's conservatively dated reference ledger (R14), restated apart from
/// the SQL: the anchor, the clean checkpoint, the open discrepancy (missing,
/// its first sighting's checkpoint copy, its running minimum) and every
/// attested flow, driven by the same steps. Amounts are whole units, so the
/// 0.01 tolerance never decides.
#[derive(Debug)]
struct Reference {
    quote: Decimal,
    as_of: DateTime<Utc>,
    checkpoint: Option<DateTime<Utc>>,
    open: Option<(bool, DateTime<Utc>, Decimal)>,
    flows: Vec<(String, Decimal, DateTime<Utc>)>,
}

impl Reference {
    /// A new discrepancy's copy: the clean checkpoint, else the anchor's.
    fn copy(&self) -> DateTime<Utc> {
        self.checkpoint.unwrap_or(self.as_of)
    }

    /// 8l-3b's branch table, PLAN-r4 §6.4 step 5.
    fn scan(&mut self, cut: DateTime<Utc>, wallet: Decimal) {
        let delta = wallet - self.quote;
        let clean = delta.is_zero();
        self.open = match self.open {
            None | Some((false, ..)) if clean => None,
            None => Some((delta < Decimal::ZERO, self.copy(), delta)),
            Some((false, copy, min)) if delta > Decimal::ZERO => {
                Some((false, copy, min.min(delta)))
            },
            Some((false, ..)) => Some((true, self.copy(), delta)),
            Some((true, copy, min)) => Some((true, copy, min.min(delta))),
        };
        if clean {
            self.checkpoint = Some(cut);
        }
    }

    /// PLAN-r4 §6.12: O1 at `cut`, O2 6 s later, both reading `wallet`.
    fn reanchor(&mut self, cut: DateTime<Utc>, wallet: Decimal) {
        let delta = wallet - self.quote;
        let min = self.open.map_or(Decimal::ZERO, |open| open.2).min(delta).min(Decimal::ZERO);
        let dating = self.open.map_or(self.copy(), |open| open.1).max(self.as_of);
        if self.open.is_some() || !delta.is_zero() {
            if min < Decimal::ZERO {
                self.flows.push(("withdrawal".into(), -min, dating));
            }
            if delta - min > Decimal::ZERO {
                self.flows.push((
                    "contribution".into(),
                    delta - min,
                    cut + TimeDelta::milliseconds(500),
                ));
            }
        }
        (self.quote, self.as_of) = (wallet, cut + TimeDelta::seconds(6));
        (self.checkpoint, self.open) = (Some(self.as_of), None);
    }
}

/// A re-anchor's W date, C date, old anchor's and own anchor's `as_of`.
type Dated = (Option<DateTime<Utc>>, Option<DateTime<Utc>>, DateTime<Utc>, DateTime<Utc>);

/// Review r1: Δ and C are exact. With `E_old` = 10⁻²⁸ and O1 = O2 =
/// `Decimal::MAX`, a checked subtraction would round Δ to MAX and book an
/// anchor `E` cannot reach; the exact one refuses `undefined`, nothing
/// written.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_inexact_difference_refuses_and_writes_nothing(pool: PgPool) -> Result<()> {
    let rig = Rig::bare(&pool, false).await?;
    rig.book.anchor(&pool, at(0), at(-10_000), Decimal::new(1, 28)).await?;
    let evidence = rig.evidence(&pool, at(60_000), [Decimal::MAX, Decimal::MAX]).await?;
    let before = rig.written(&pool).await?;
    ensure!(rig.commit(&evidence).await == Err(ReanchorRefusal::Undefined));
    ensure!(rig.written(&pool).await? == before, "nothing written");
    Ok(())
}

/// Review r1, PLAN-r4 §7.2: at the cardinality of 8l-3a's `bound_fixture`
/// (10,000 fills, 200 legs, 2,000 intents, 50 re-anchors, an open
/// discrepancy of 500 sightings), twenty re-anchors each hold the accounting
/// rows well within 400 ms; the context's plan is printed.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn reanchor_holds_the_rows_within_its_bound(pool: PgPool) -> Result<()> {
    let rig = Rig::bare(&pool, false).await?;
    let counts = bound_fixture(&pool, &rig.book).await?;
    let start = utc("2026-11-20T00:00:00Z");
    let mut times = Vec::new();
    for n in 0..20 {
        let cut = start + TimeDelta::seconds(20 * n);
        let evidence = rig.evidence(&pool, cut, [dec!(69899.2), dec!(69899.2)]).await?;
        let started = std::time::Instant::now();
        let done = rig.commit(&evidence).await.map_err(|r| anyhow::anyhow!("{n}: {r:?}"))?;
        times.push(started.elapsed());
        let delta = if n == 0 { dec!(-100) } else { dec!(0) };
        ensure!(done.delta == Some(delta), "{n}: {done:?}");
    }
    times.sort();
    let slowest = times[times.len() - 1];
    ensure!(slowest < std::time::Duration::from_millis(400), "slowest re-anchor {slowest:?}");
    let cut = start + TimeDelta::seconds(400);
    let month = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).context("month")?;
    let plan: Vec<String> = sqlx::query_scalar(&format!("EXPLAIN (ANALYZE, BUFFERS) {}", *CONTEXT))
        .bind(rig.book.tenant)
        .bind(VenueId::BINANCE_SPOT)
        .bind(month)
        .bind(Utc::now())
        .bind(vec!["BTCUSDT"])
        .bind(vec!["BTC"])
        .bind(vec!["USDT"])
        .bind(cut)
        .bind(cut + TimeDelta::seconds(6))
        .bind(cut + TimeDelta::milliseconds(6_500))
        .bind(rig.book.epoch)
        .fetch_all(&pool)
        .await?;
    eprintln!(
        "REANCHOR_BOUND median_us={} max_us={} counts={counts:?}\n{}",
        times[times.len() / 2].as_micros(),
        slowest.as_micros(),
        plan.join("\n")
    );
    println!(
        "REANCHOR_BOUND median_us={} max_us={}",
        times[10].as_micros(),
        slowest.as_micros()
    );
    Ok(())
}

/// One step of a generated sequence (R14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Move(i64),
    Scan,
    Fail,
    Reanchor,
    Idle,
}

/// 36 ten-minute slots from B − 3 h; B at slot 18. No re-anchor within 1 h
/// of B; an observation between any two movements (else a withdrawal and a
/// deposit could cancel unseen across B: v16's commitment case); the last
/// slot a re-anchor. The wallet never goes below zero.
fn sequence(seed: u64) -> Vec<Step> {
    let mut state = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    let mut next = |n: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % n
    };
    let (mut steps, mut wallet, mut unseen) = (Vec::new(), 2_000_i64, false);
    for slot in 0..36_u64 {
        let near = (12..=24).contains(&slot);
        let step = match next(20) {
            0..=4 if !unseen => {
                let amount = i64::try_from(next(300) + 1).unwrap_or(1);
                let amount = if next(2) == 0 && wallet > amount {
                    -amount
                } else {
                    amount
                };
                wallet += amount;
                Step::Move(amount)
            },
            0..=10 => Step::Scan,
            11..=12 => Step::Fail,
            13..=16 if !near => Step::Reanchor,
            _ => Step::Idle,
        };
        unseen = matches!(step, Step::Move(_))
            || (unseen && !matches!(step, Step::Scan | Step::Reanchor));
        steps.push(step);
    }
    steps.push(Step::Reanchor);
    steps
}

/// R14 and v16 §C8's property over generated sequences of movements, scans,
/// failed reads and re-anchors around a boundary B: after every re-anchor
/// `E` equals the wallet at and after O2's cut; every withdrawal is dated at
/// or after the anchor its re-anchor replaced and every contribution before
/// the anchor it writes; `quote_B = E(B)` never exceeds the cash at B, and
/// equals it when every movement's dated flow lies on the movement's side of
/// B.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn repeated_reanchors_keep_e_continuous(pool: PgPool) -> Result<()> {
    let b = utc("2026-12-01T00:00:00Z");
    let slot =
        |k: usize| b - TimeDelta::hours(3) + TimeDelta::minutes(10 * i64::try_from(k).unwrap_or(0));
    let (mut equal, mut conservative) = (0, 0);
    for seed in 1..=24_u64 {
        let steps = sequence(seed);
        let rig = Rig::bare(&pool, false).await?;
        rig.book
            .anchor(
                &pool,
                slot(0) - TimeDelta::minutes(10),
                slot(0) - TimeDelta::minutes(11),
                dec!(2000),
            )
            .await?;
        let mut wallet = dec!(2000);
        let mut reference = Reference {
            quote: wallet,
            as_of: slot(0) - TimeDelta::minutes(10),
            checkpoint: None,
            open: None,
            flows: Vec::new(),
        };
        // (time, amount, the re-anchor that first saw it)
        let mut moves: Vec<(DateTime<Utc>, Decimal, Option<usize>)> = Vec::new();
        let mut reanchors = 0;
        for (k, step) in steps.iter().enumerate() {
            let t = slot(k);
            match *step {
                Step::Move(amount) => {
                    wallet += Decimal::from(amount);
                    moves.push((t, Decimal::from(amount), None));
                },
                Step::Scan => {
                    reference.scan(t, wallet);
                    rig.scan(&pool, t, wallet)
                        .await
                        .with_context(|| format!("seed {seed} slot {k}"))?;
                },
                Step::Reanchor => {
                    reference.reanchor(t, wallet);
                    rig.reanchor(&pool, t, wallet)
                        .await
                        .with_context(|| format!("seed {seed} slot {k}"))?;
                    let cut = t + TimeDelta::seconds(6);
                    ensure!(
                        value(rig.expected(&pool, cut).await?) == Some(wallet),
                        "seed {seed} slot {k}: at O2's cut"
                    );
                    ensure!(
                        value(rig.expected(&pool, cut + micro()).await?) == Some(wallet),
                        "seed {seed} slot {k}"
                    );
                    for entry in moves.iter_mut().filter(|m| m.2.is_none()) {
                        entry.2 = Some(reanchors);
                    }
                    reanchors += 1;
                },
                Step::Fail | Step::Idle => {},
            }
        }
        // Each re-anchor's flows, in order: (W date, C date).
        let dated: Vec<Dated> = sqlx::query_as(
            "SELECT (SELECT f.economic_date FROM cash_attested_flows f WHERE f.reanchor_id = r.reanchor_id AND f.direction = 'withdrawal'),
                    (SELECT f.economic_date FROM cash_attested_flows f WHERE f.reanchor_id = r.reanchor_id AND f.direction = 'contribution'),
                    o.as_of, n.as_of
             FROM cash_reanchors r JOIN cash_anchors o ON o.anchor_id = r.old_anchor_id JOIN cash_anchors n ON n.anchor_id = r.new_anchor_id
             WHERE r.tenant_id = $1 ORDER BY n.as_of")
            .bind(rig.book.tenant)
            .fetch_all(&pool)
            .await?;
        for (w, c, old, new) in &dated {
            ensure!(
                w.is_none_or(|w| w >= *old && w < *new),
                "seed {seed}: W {w:?} vs [{old}, {new})"
            );
            ensure!(
                c.is_none_or(|c| c > *old && c < *new),
                "seed {seed}: C {c:?} vs ({old}, {new})"
            );
        }
        // Every flow is the reference ledger's, so quote_B is its E(B).
        reference.flows.sort_by(|a, b| (a.2, &a.0).cmp(&(b.2, &b.0)));
        ensure!(rig.flows(&pool).await? == reference.flows, "seed {seed}: {steps:?}");
        let cash_b = moves.iter().filter(|m| m.0 < b).map(|m| m.1).sum::<Decimal>() + dec!(2000);
        let quote_b = value(rig.expected(&pool, b).await?).context("E(B)")?;
        let signed =
            |(direction, amount, _): &(String, Decimal, DateTime<Utc>)| match direction.as_str() {
                "withdrawal" => -*amount,
                _ => *amount,
            };
        let reference_b =
            dec!(2000) + reference.flows.iter().filter(|f| f.2 < b).map(signed).sum::<Decimal>();
        ensure!(
            quote_b == reference_b,
            "seed {seed}: quote_B {quote_b} != reference {reference_b}"
        );
        ensure!(quote_b <= cash_b, "seed {seed}: quote_B {quote_b} > cash {cash_b}: {steps:?}");
        let sided = moves.iter().all(|(t, amount, seen)| {
            let flow = seen.and_then(|k| dated.get(k)).and_then(|(w, c, _, _)| {
                match amount.is_sign_negative() {
                    true => *w,
                    false => *c,
                }
            });
            flow.is_some_and(|date| (date < b) == (*t < b))
        });
        if sided {
            ensure!(
                quote_b == cash_b,
                "seed {seed}: quote_B {quote_b} != cash {cash_b}: {steps:?}"
            );
            equal += 1;
        } else {
            conservative += 1;
        }
    }
    println!("REANCHOR_PROPERTY cases=24 equal={equal} conservative={conservative}");
    ensure!(equal > 0 && conservative > 0, "both branches exercised");
    Ok(())
}

/// The writer-level rehearsal (PLAN-r4 §8): a cutover anchor, a clean scan,
/// a withdrawal, missing cash fenced (V5), the re-anchor, `E` = wallet, the
/// discrepancy resolved, and the withdrawal fence for the rest of the month,
/// gone at the next.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn rehearsal_withdrawal_is_fenced_until_the_next_boundary(pool: PgPool) -> Result<()> {
    let rig = Rig::bare(&pool, false).await?;
    let t0 = utc("2026-11-10T12:00:00Z");
    // The anchor's cutover run (M56's foreign key).
    let cutover = Uuid::now_v7();
    sqlx::query(include_str!("../../../../../tests/fixtures/cutover_run.sql"))
        .bind("binance:spot")
        .bind(rig.book.tenant)
        .bind(cutover)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO cash_anchors (anchor_id,tenant_id,venue_id,kind,cutover_id,quote,as_of,send_at,receive_at,u_ns,sample_id,epoch_id,interval_start,interval_end,depends_start,depends_end)
        VALUES ($1,$2,'binance:spot','cutover',$3,2000,$4,$4+interval '150 ms',$4+interval '350 ms',150000000,$5,$6,$4,$4+interval '500 ms',$4-interval '10 s',$4+interval '500 ms')")
        .bind(Uuid::now_v7()).bind(rig.book.tenant).bind(cutover).bind(t0).bind(rig.book.sample).bind(rig.book.epoch)
        .execute(&pool).await?;
    rig.scan(&pool, t0 + TimeDelta::minutes(1), dec!(2000)).await?;
    rig.scan(&pool, t0 + TimeDelta::minutes(5), dec!(1900)).await?;
    let fenced = rig
        .book
        .state(&pool, t0 + TimeDelta::minutes(6), t0 + TimeDelta::minutes(6))
        .await?;
    ensure!(fenced.open_missing && !fenced.withdrawal_fence, "{fenced:?}");
    rig.reanchor(&pool, t0 + TimeDelta::minutes(10), dec!(1900)).await?;
    ensure!(
        rig.flows(&pool).await?
            == [("withdrawal".to_string(), dec!(100), t0 + TimeDelta::minutes(1))]
    );
    let later = t0 + TimeDelta::minutes(20);
    let state = rig.book.state(&pool, later, later).await?;
    ensure!(
        state.expected.value == Some(dec!(1900)) && !state.open_missing && state.withdrawal_fence,
        "{state:?}"
    );
    let next = utc("2026-12-01T00:00:00Z");
    ensure!(
        !rig.book.state(&pool, next, next).await?.withdrawal_fence,
        "the fence ends at the boundary"
    );
    Ok(())
}

// ---- crash across B (crash-points builds) ----

#[cfg(feature = "crash-points")]
const CRASH_CHILD: &str =
    "venue_ledger_store::cash::reanchor::commit::pg_tests::reanchor_crash_child";
#[cfg(feature = "crash-points")]
const CRASH_APP: &str = "robson-8l5b-crash";

/// R13-1 (v16:2028-2030): a no-movement re-anchor on November 30, a
/// withdrawal of 100, the next re-anchor killed at `REANCHOR_BEFORE_COMMIT`
/// (nothing written, the rows released), a scan after B sees missing cash
/// whose stored checkpoint is the first re-anchor's (V6 holds December), and
/// the retry on December 1 at 02:00 dates W at that checkpoint in November:
/// `quote_B` = 1,900, no December fence.
#[cfg(feature = "crash-points")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn crash_across_b_dates_the_withdrawal_before_b(pool: PgPool) -> Result<()> {
    use std::os::unix::process::ExitStatusExt;
    let rig = Rig::bare(&pool, false).await?;
    let b = utc("2026-12-01T00:00:00Z");
    let start = utc("2026-11-30T20:00:00Z");
    rig.book
        .anchor(&pool, start, start - TimeDelta::seconds(10), dec!(2000))
        .await?;
    rig.scan(&pool, utc("2026-11-30T20:59:50Z"), dec!(2000)).await?;
    let first = utc("2026-11-30T21:00:03Z");
    rig.reanchor(&pool, first, dec!(2000)).await?;
    let checkpoint = first + TimeDelta::seconds(6);
    // The withdrawal; the next re-anchor dies before COMMIT.
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .context("host")?;
    let url = database_url_through(
        &std::env::var("DATABASE_URL")?,
        upstream,
        options.get_database().context("db")?,
    )?;
    let marker = std::env::temp_dir().join(format!("robson-8l5b-crash-{}", rig.book.tenant));
    let before = rig.written(&pool).await?;
    let status = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            CRASH_CHILD,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(robson_exec::crash_points::POINT_ENV, "reanchor_before_commit")
        .env(robson_exec::crash_points::MARKER_FILE_ENV, &marker)
        .env(robson_exec::crash_points::WATCHDOG_ENV, "30")
        .env("ROBSON_TEST_REANCHOR_CRASH_URL", url)
        .env(
            "ROBSON_TEST_REANCHOR_CRASH_IDS",
            format!(
                "{} {} {} {}",
                rig.book.tenant, rig.book.scope, rig.book.sample, rig.book.epoch
            ),
        )
        .status()?;
    ensure!(status.signal() == Some(9), "{status:?}");
    ensure!(std::fs::read_to_string(&marker)? == "reanchor_before_commit");
    std::fs::remove_file(&marker)?;
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    while sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM pg_stat_activity WHERE application_name=$1",
    )
    .bind(CRASH_APP)
    .fetch_one(&pool)
    .await?
        > 0
    {
        ensure!(Instant::now() < deadline, "the child's session lingers");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    ensure!(rig.written(&pool).await? == before, "the killed re-anchor wrote nothing");
    // After B: a scan sees the missing 100 against the stored checkpoint.
    rig.scan(&pool, utc("2026-12-01T00:30:00Z"), dec!(1900)).await?;
    let pending = rig.book.state(&pool, b, utc("2026-12-01T00:31:00Z")).await?;
    ensure!(
        pending.missing_before_t && pending.open_missing,
        "V6 holds December: {pending:?}"
    );
    rig.reanchor(&pool, utc("2026-12-01T02:00:00Z"), dec!(1900)).await?;
    ensure!(rig.flows(&pool).await? == [("withdrawal".to_string(), dec!(100), checkpoint)]);
    let after = rig.book.state(&pool, b, utc("2026-12-01T02:01:00Z")).await?;
    ensure!(after.expected.value == Some(dec!(1900)) && !after.withdrawal_fence, "{after:?}");
    Ok(())
}

/// The child of the test above: the parent's withdrawal seen by O1 and O2,
/// committed by a re-anchor armed to die before COMMIT.
#[cfg(feature = "crash-points")]
#[test]
#[ignore = "child process of crash_across_b_dates_the_withdrawal_before_b"]
fn reanchor_crash_child() -> Result<()> {
    use std::str::FromStr;

    use sqlx::postgres::PgConnectOptions;
    let Ok(url) = std::env::var("ROBSON_TEST_REANCHOR_CRASH_URL") else {
        return Ok(());
    };
    robson_exec::crash_points::arm_from_env();
    let ids = std::env::var("ROBSON_TEST_REANCHOR_CRASH_IDS")?;
    let ids: Vec<&str> = ids.split(' ').collect();
    let [tenant, scope, sample, epoch] = ids[..] else {
        anyhow::bail!("four ids")
    };
    let (tenant, sample, epoch) =
        (Uuid::parse_str(tenant)?, Uuid::parse_str(sample)?, Uuid::parse_str(epoch)?);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let options = PgConnectOptions::from_str(&url)?.application_name(CRASH_APP);
            let pool = PgPoolOptions::new().max_connections(1).connect_with(options).await?;
            let venue = VenueId::new(VenueId::BINANCE_SPOT);
            let book = Book {
                tenant,
                scope: scope.to_string(),
                sample,
                epoch,
                leg: Uuid::nil(),
            };
            let rig = Rig {
                reanchor: CashReanchor {
                    pool: pool.clone(),
                    accounting: BalanceAccounting {
                        tenant_id: tenant,
                        venue: venue.clone(),
                        account_scope: scope.to_string(),
                        generation: GenerationCell::default(),
                    },
                    gate: OrderJournal::new(Arc::new(StubJournalBackend::default()), &venue)
                        .gate()
                        .clone(),
                    epoch,
                    uid: "4242".into(),
                    pair: ASSETS[0],
                    venue: Arc::new(Unused),
                    read_only: false,
                    flight: Default::default(),
                },
                book,
            };
            let refused = rig.reanchor(&pool, utc("2026-11-30T21:10:00Z"), dec!(1900)).await;
            anyhow::bail!("the child must die before COMMIT: {refused:?}")
        })
}

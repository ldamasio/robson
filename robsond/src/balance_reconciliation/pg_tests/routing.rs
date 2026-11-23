//! 8k-3c proofs (PLAN-r2 §8): every scan's result takes exactly one route,
//! through the production attempt, scan and cash writer on PostgreSQL; the
//! page and fence sinks; withheld recovery; no retry after a writer refusal;
//! generated schedules of invalidators at every point of an attempt and of
//! its publication; the module's source guards; the metrics' zero export.
use std::{future::Future, sync::Mutex};

use chrono::{DateTime, Utc};
use rust_decimal_macros::dec;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgConnection};

use super::*;
use crate::{
    balance_accounting::{BalanceAccounting, GenerationCell},
    balance_reconciliation::route::{
        route, scan_once, ConflictSink, Routed, ScanMetrics, SCAN_METRICS,
    },
    order_journal_store::PgJournalBackend,
    process_epoch,
    venue_ledger_store::{
        cash::{
            read_cash_state,
            writer::{CashAttempt, CashRefusal, CashWriter},
            CashQuery, CashState,
        },
        settlement::CommissionAssets,
    },
};

const ASSETS: &[CommissionAssets<'static>] = &[CommissionAssets {
    symbol: "BTCUSDT",
    base: "BTC",
    quote: "USDT",
}];

const OUTCOMES: [&str; 8] = [
    "concluded",
    "writer_refused",
    "uncertain",
    "conflict",
    "recovery_withheld",
    "held",
    "incomplete",
    "unqualified",
];

/// The scripted receipts' wall clock: receipt `n` is sent at `T + 10n` s and
/// received a second later, with u = 1 ns.
fn wall(seconds: i64, millis: i64) -> DateTime<Utc> {
    let at = DateTime::from_timestamp(1_794_614_400 + seconds, 0).expect("wall");
    at + chrono::TimeDelta::milliseconds(millis)
}

/// A balance-account desk of the non-trading scope: 8k-3a's fixture, the
/// clock sample its receipts are judged on, an observed anchor of 1000 USDT
/// three days before them, and its cash writer's accounting context (no
/// order-set generation: no CAS).
struct Desk {
    f: Fixture,
    sample: Uuid,
    accounting: BalanceAccounting,
}

impl Desk {
    async fn new(pool: &PgPool) -> Result<Self> {
        let f = Fixture::new(pool).await?;
        let sample = Uuid::now_v7();
        sqlx::query("INSERT INTO clock_samples (sample_id,tenant_id,venue_id,process_epoch,clock_domain,send_wall_seconds,send_wall_nanoseconds,send_tick_before,send_tick_after,receive_wall_seconds,receive_wall_nanoseconds,receive_tick_before,receive_tick_after,server_ms,theta2_ns,r_low_ns,r_high_ns,u2_ns,numeric_verdict,numeric_failures,algorithm_version,recorded_at) VALUES ($1,$2,'binance:spot',$3,ROW('boottime',$4::uuid,0,0)::budget_clock_domain_v38,1794614400,0,0,0,1794614400,0,0,0,0,-3589228800000000000,0,0,3589228800200000000,'invalid',1,1,now())")
            .bind(sample).bind(f.tenant).bind(f.epoch).bind(Uuid::now_v7())
            .execute(pool).await?;
        let as_of: DateTime<Utc> = "2026-11-10T12:00:00Z".parse()?;
        sqlx::query("INSERT INTO cash_anchors (anchor_id,tenant_id,venue_id,kind,quote,as_of,send_at,receive_at,u_ns,sample_id,epoch_id,interval_start,interval_end,depends_start,depends_end)
            VALUES ($1,$2,'binance:spot','reanchor',1000,$3,$3+interval '150 ms',$3+interval '350 ms',150000000,$4,$5,$3,$3+interval '500 ms',$3-interval '10 s',$3+interval '500 ms')")
            .bind(Uuid::now_v7()).bind(f.tenant).bind(as_of).bind(sample).bind(f.epoch)
            .execute(pool).await?;
        let accounting = BalanceAccounting {
            tenant_id: f.tenant,
            venue: f.venue.clone(),
            account_scope: f.scope.clone(),
            generation: GenerationCell::default(),
        };
        Ok(Self { f, sample, accounting })
    }

    fn writer<'a>(&'a self, pool: &'a PgPool, gate: &'a OperationGate) -> CashWriter<'a> {
        CashWriter {
            pool,
            accounting: &self.accounting,
            gate,
            epoch: self.f.epoch,
            assets: ASSETS,
            read_only: false,
        }
    }

    /// The scripted port on this desk's sample, its receipts holding `usdt`.
    fn reads<'a>(
        &'a self,
        pool: &'a PgPool,
        usdt: &'static str,
        script: Vec<Step>,
    ) -> Scripted<'a> {
        let mut reads = Scripted::new(pool, &self.f, script);
        reads.sample = self.sample;
        reads.usdt = usdt;
        reads
    }

    /// A discovered BTC buy filled at `time` in another scope of the
    /// tenant's venue, its rows then the bump, as one booking transaction.
    async fn fill_in(&self, tx: &mut PgConnection, time: DateTime<Utc>) -> Result<()> {
        let id = format!("{}", Uuid::now_v7().as_u128() % 1_000_000_000_000);
        let scope = format!("other-{}", self.f.tenant);
        sqlx::query("INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,side,order_type,orig_qty,status,phase,executed_cum,fills_qty,last_checked_at,live_seen_at,terminal_seen_at)
            VALUES ('binance:spot',$1,'BTCUSDT',$2,$3,$2,$2,'discovered','BUY','MARKET',0.1,'FILLED','terminal',0.1,0.1,$4,$4,$4)")
            .bind(&scope).bind(&id).bind(self.f.tenant).bind(time)
            .execute(&mut *tx).await?;
        sqlx::query("INSERT INTO order_fills VALUES ('binance:spot',$1,'BTCUSDT',$2,$2,$3,100,0.1,10,0,'BTC',$4,$4)")
            .bind(&scope).bind(&id).bind(self.f.tenant).bind(time)
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE accounting_state SET version = version + 1 WHERE tenant_id=$1")
            .bind(self.f.tenant)
            .execute(&mut *tx)
            .await?;
        Ok(())
    }

    async fn fill(&self, pool: &PgPool, time: DateTime<Utc>) -> Result<()> {
        let mut tx = pool.begin().await?;
        self.fill_in(&mut tx, time).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn state(&self, pool: &PgPool) -> Result<CashState> {
        let t = wall(1_000, 0);
        let query = CashQuery {
            tenant_id: self.f.tenant,
            venue: &self.f.venue,
            t,
            now: t,
            assets: ASSETS,
        };
        Ok(read_cash_state(&mut *pool.acquire().await?, &query).await?)
    }

    async fn count(&self, pool: &PgPool, table: &str) -> Result<i64> {
        let sql = format!("SELECT count(*) FROM {table} WHERE tenant_id=$1");
        Ok(sqlx::query_scalar(&sql).bind(self.f.tenant).fetch_one(pool).await?)
    }
}

/// Scan options and a writer pool through one counting proxy.
async fn proxied_pool(pool: &PgPool) -> Result<(DbProxy, PgConnectOptions, PgPool)> {
    let (proxy, options) = proxied(pool).await?;
    let writers = PgPoolOptions::new().max_connections(1).connect_lazy_with(options.clone());
    Ok((proxy, options, writers))
}

/// A journal persisted to this database for the desk's tenant.
fn durable(pool: &PgPool, desk: &Desk) -> OrderJournal {
    let backend = Arc::new(PgJournalBackend::new(pool, desk.f.tenant));
    OrderJournal::new(backend, &desk.f.venue).with_balance_account(true)
}

fn scans(metrics: &ScanMetrics, outcome: &str) -> u64 {
    metrics.scans.with_label_values(&[outcome]).get()
}

fn one_attempt() -> ScanOutcome {
    ScanOutcome {
        attempts: 1,
        cost: ScanCost { sql: 2, http: 3, weight: 32 },
        outcome: AttemptOutcome::Qualified(interval()),
    }
}

/// The gate's incident state: generation, fence and open incidents.
fn incidents(gate: &OperationGate) -> (i64, bool, usize) {
    (gate.generation(), gate.fenced(), gate.open_incidents().len())
}

type Events = Vec<(tracing::Level, Option<String>)>;

/// Every event `work` logs, by level and `kind` field.
async fn logged<T>(work: impl Future<Output = T>) -> (T, Events) {
    use tracing::{field::Field, instrument::WithSubscriber, Event, Subscriber};
    use tracing_subscriber::layer::{Context as LayerContext, Layer, SubscriberExt};

    struct Capture(Arc<Mutex<Events>>);
    struct KindField(Option<String>);

    impl tracing::field::Visit for KindField {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "kind" {
                self.0 = Some(format!("{value:?}"));
            }
        }
    }

    impl<S: Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &Event<'_>, _: LayerContext<'_, S>) {
            let mut kind = KindField(None);
            event.record(&mut kind);
            if let Ok(mut seen) = self.0.lock() {
                seen.push((*event.metadata().level(), kind.0));
            }
        }
    }

    let seen = Arc::new(Mutex::new(Events::new()));
    let subscriber = tracing_subscriber::registry().with(Capture(seen.clone()));
    tracing::callsite::rebuild_interest_cache();
    let output = work.with_subscriber(subscriber).await;
    let events = seen.lock().map(|seen| seen.clone()).unwrap_or_default();
    (output, events)
}

fn order(id: u64) -> serde_json::Value {
    json!({"symbol": "BTCUSDT", "orderId": id, "orderListId": -1, "clientOrderId": "web-1",
        "price": "100", "origQty": "0.1", "executedQty": "0", "cummulativeQuoteQty": "0",
        "status": "NEW", "timeInForce": "GTC", "type": "LIMIT", "side": "BUY", "time": 1,
        "updateTime": 1, "selfTradePreventionMode": "NONE"})
}

/// Every verdict and consequence takes exactly one route, counted once
/// under its outcome, its refusal under its stage; the writer's statements
/// appear only for the Clean of a real scan.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn routing_matrix_takes_exactly_one_route(pool: PgPool) -> Result<()> {
    use Finding as F;
    use IncompleteReason as I;
    use UnqualifiedReason as U;
    let desk = Desk::new(&pool).await?;
    let gate = gate(&desk.f.venue);
    let (proxy, options, writers) = proxied_pool(&pool).await?;
    let writer = desk.writer(&writers, &gate);
    let metrics = ScanMetrics::new(None);
    let found = |findings: Vec<F>| BaseVerdict::Discrepancy { findings, interval: interval() };
    let (one, zero) = (dec!(1), dec!(0));
    let excess = F::BaseExcess { holdings: one, upper: zero };
    let within = F::WithinClaims {
        holdings: one,
        lower: zero,
        upper: dec!(2),
    };
    let mut cases = vec![
        (found(vec![excess.clone()]), "conflict"),
        (found(vec![F::BaseDeficit { holdings: zero, lower: one }]), "conflict"),
        (found(vec![F::LockedForeign { locked: one, robson: zero }]), "conflict"),
        (found(vec![F::OperatedOrderList { order_list_id: 7 }]), "conflict"),
        (
            found(vec![F::LockedShort {
                locked: zero,
                robson: one,
                stops: vec!["9".into()],
            }]),
            "recovery_withheld",
        ),
        (
            found(vec![F::NeedsExactEvidence { exchange_order_id: "9".into() }]),
            "recovery_withheld",
        ),
        (found(vec![F::UnlinkedOperatedOrder { order_id: 5 }]), "recovery_withheld"),
        (found(vec![within.clone()]), "held"),
        (
            found(vec![within, F::UnlinkedOperatedOrder { order_id: 6 }]),
            "recovery_withheld",
        ),
        (found(vec![F::UnlinkedOperatedOrder { order_id: 8 }, excess]), "conflict"),
        // A Clean without the writer's input concludes nothing.
        (BaseVerdict::Clean { interval: interval() }, "unqualified"),
    ];
    let incomplete = [
        I::MalformedAccount,
        I::ClosedLegWithoutWriteOff,
        I::InFlightOrder,
        I::AttributionContradiction,
        I::UndefinedPartition,
        I::Deficit,
        I::RecoveryState,
        I::UncertainIntent,
        I::UncertainCancel,
        I::Unrepresentable,
        I::Malformed,
    ];
    for reason in incomplete {
        cases.push((BaseVerdict::Incomplete { reason, interval: interval() }, "incomplete"));
    }
    let unqualified = [
        U::VersionChanged,
        U::GenerationChanged,
        U::PendingEvidence,
        U::Clock,
        U::Venue,
        U::Sql,
        U::Timeout,
        U::Scope,
        U::Cap,
        U::Malformed,
        U::LedgerNotEmpty,
        U::Deferred,
    ];
    for reason in unqualified {
        cases.push((BaseVerdict::Unqualified(reason), "unqualified"));
    }
    let statements = proxy.stats().statements;
    let total = cases.len() as u64;
    for (n, (verdict, outcome)) in cases.into_iter().enumerate() {
        let before = scans(&metrics, outcome);
        let scanned = Scanned {
            outcome: one_attempt(),
            verdict: verdict.clone(),
            cash: None,
        };
        let routed = route(scanned, &writer, ConflictSink::Page, &metrics).await;
        ensure!(scans(&metrics, outcome) == before + 1, "{verdict:?} took {routed:?}");
        let counted: u64 = OUTCOMES.iter().map(|o| scans(&metrics, o)).sum();
        ensure!(counted == n as u64 + 1, "{verdict:?}: one route each");
    }
    ensure!(
        proxy.stats().statements == statements,
        "no route but a Clean's reaches the writer"
    );
    let refused = |stage: &str, reason: &str| metrics.refusals.with_label_values(&[stage, reason]);
    ensure!(refused("classify", "closed_leg_without_write_off").get() == 1);
    ensure!(refused("qualify", "ledger_not_empty").get() == 1);
    ensure!(refused("qualify", "malformed").get() == 2, "the Clean without input too");
    ensure!(metrics.withheld.with_label_values(&["unlinked_operated_order"]).get() == 3);
    ensure!(metrics.conflicts.with_label_values(&["base_excess"]).get() == 2);
    ensure!((metrics.attempts.get(), metrics.weight.get()) == (total, 32 * total));
    // The Clean of a real scan reaches the writer: its statements follow
    // the scan's two.
    let mut reads = desk.reads(&pool, "1000", vec![Step::Answer { btc: "0", bump: false }]);
    let routed = scan_once(&mut reads, &options, &desk.f.scope(), &writer, &metrics).await;
    ensure!(matches!(routed, Some(Routed::Concluded(_))), "{routed:?}");
    ensure!(proxy.stats().statements > statements + 2 && scans(&metrics, "concluded") == 1);
    ensure!(desk.count(&pool, "cash_checkpoints").await? == 1);
    ensure!(metrics.last_concluded.get() > 0, "the scan-health clock moved");
    Ok(())
}

/// C1: the verdict decides. Every other verdict carrying a qualified
/// Clean's own writer input, and a Clean of another interval with it, never
/// reach the writer; the Clean it was drawn on concludes.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn only_a_clean_verdict_reaches_the_writer(pool: PgPool) -> Result<()> {
    let desk = Desk::new(&pool).await?;
    let gate = gate(&desk.f.venue);
    let (proxy, options, writers) = proxied_pool(&pool).await?;
    let writer = desk.writer(&writers, &gate);
    let metrics = ScanMetrics::new(None);
    let mut reads = desk.reads(&pool, "1000", vec![Step::Answer { btc: "0", bump: false }]);
    let scanned = scan_non_trading(&mut reads, &gate, &options, &desk.f.scope()).await;
    let clean = scanned.context("one attempt")?;
    let cash = clean.cash.clone().context("the writer's input")?;
    let BaseVerdict::Clean { interval } = &clean.verdict else {
        anyhow::bail!("{clean:?}");
    };
    let interval = *interval;
    let other = ObservationInterval {
        start_ns: interval.start_ns - 1,
        ..interval
    };
    let excess = Finding::BaseExcess { holdings: dec!(1), upper: dec!(0) };
    let within = Finding::WithinClaims {
        holdings: dec!(1),
        lower: dec!(0),
        upper: dec!(2),
    };
    let hostile = [
        BaseVerdict::Discrepancy { findings: vec![excess], interval },
        BaseVerdict::Discrepancy { findings: vec![within], interval },
        BaseVerdict::Incomplete {
            reason: IncompleteReason::MalformedAccount,
            interval,
        },
        BaseVerdict::Unqualified(UnqualifiedReason::VersionChanged),
        BaseVerdict::Clean { interval: other },
    ];
    let statements = proxy.stats().statements;
    for verdict in hostile {
        let scanned = Scanned {
            outcome: clean.outcome,
            verdict: verdict.clone(),
            cash: Some(cash.clone()),
        };
        let routed = route(scanned, &writer, ConflictSink::Page, &metrics).await;
        let published =
            matches!(routed, Routed::Concluded(_) | Routed::WriterRefused(_) | Routed::Uncertain);
        ensure!(!published, "{verdict:?} reached the writer: {routed:?}");
        ensure!(proxy.stats().statements == statements, "{verdict:?}: writer statements");
    }
    // The first hostile verdict paged; a Clean without its own input (here
    // none at all) is refused and leaves the page standing.
    let bare = Scanned {
        outcome: clean.outcome,
        verdict: clean.verdict.clone(),
        cash: None,
    };
    let routed = route(bare, &writer, ConflictSink::Page, &metrics).await;
    ensure!(matches!(routed, Routed::Unqualified(UnqualifiedReason::Malformed)));
    ensure!(metrics.conflict.get() == 1, "a malformed Clean never clears the page");
    ensure!(desk.count(&pool, "cash_checkpoints").await? == 0, "nothing concluded");
    let routed = route(clean, &writer, ConflictSink::Page, &metrics).await;
    ensure!(matches!(routed, Routed::Concluded(_)), "{routed:?}");
    ensure!(desk.count(&pool, "cash_checkpoints").await? == 1);
    ensure!(proxy.stats().statements > statements, "the Clean's own publication");
    ensure!(metrics.conflict.get() == 0, "the Clean that reached the writer clears it");
    Ok(())
}

/// C2, C6, C8: a non-trading process pages each fence-and-page finding and
/// records nothing: the gauge at 1, one ERROR and one count per finding;
/// the gate's generation, fence and incidents unchanged, nothing for the
/// journal's retry pass, no `recovery_incidents` row. A scan that concludes
/// nothing leaves the page standing; a later qualified Clean clears it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_non_trading_conflict_pages_without_an_incident(pool: PgPool) -> Result<()> {
    use tracing::Level;
    let desk = Desk::new(&pool).await?;
    let journal = durable(&pool, &desk);
    let gate = journal.gate().clone();
    let writer = desk.writer(&pool, &gate);
    let options = pool.connect_options();
    let scope = desk.f.scope();
    let metrics = ScanMetrics::new(None);
    let before = incidents(&gate);
    let list = json!([{"orderListId": 77, "symbol": "BTCUSDT", "listClientOrderId": "oco-1",
        "orders": [{"symbol": "BTCUSDT", "orderId": 1, "clientOrderId": "a"},
                   {"symbol": "BTCUSDT", "orderId": 2, "clientOrderId": "b"}]}]);
    // BTC free, BTC locked, open lists: the findings each scan pages.
    let cases: [(&str, &str, serde_json::Value, &[&str]); 3] = [
        ("0.5", "0", json!([]), &["base_excess"]),
        ("0", "0.5", json!([]), &["base_excess", "locked_foreign"]),
        ("0", "0", list, &["operated_order_list"]),
    ];
    for (n, (free, locked, lists, kinds)) in cases.into_iter().enumerate() {
        let mut reads = desk.reads(&pool, "1000", vec![Step::Answer { btc: free, bump: false }]);
        (reads.locked, reads.lists, reads.calls) = (locked, lists, 10 * n as u32);
        let (routed, events) =
            logged(scan_once(&mut reads, &options, &scope, &writer, &metrics)).await;
        ensure!(matches!(routed, Some(Routed::Conflict)), "{routed:?}");
        let errors: Vec<_> = events.iter().filter(|(level, _)| *level == Level::ERROR).collect();
        let paged: Vec<_> = errors.iter().filter_map(|(_, kind)| kind.as_deref()).collect();
        ensure!(paged == kinds && errors.len() == kinds.len(), "{kinds:?}: {events:?}");
        ensure!(metrics.conflict.get() == 1, "{kinds:?}: the page stands");
        ensure!(incidents(&gate) == before, "{kinds:?}: nothing recorded on the gate");
    }
    // A base deficit cannot arise on an empty ledger (its lower bound is 0).
    let deficit = Finding::BaseDeficit { holdings: dec!(0), lower: dec!(1) };
    let verdict = BaseVerdict::Discrepancy {
        findings: vec![deficit],
        interval: interval(),
    };
    let scanned = Scanned {
        outcome: one_attempt(),
        verdict,
        cash: None,
    };
    let (routed, events) = logged(route(scanned, &writer, ConflictSink::Page, &metrics)).await;
    ensure!(matches!(routed, Routed::Conflict), "{routed:?}");
    let errors: Vec<_> = events.into_iter().filter(|(level, _)| *level == Level::ERROR).collect();
    ensure!(errors == [(Level::ERROR, Some("base_deficit".to_string()))], "{errors:?}");
    let counted = [
        "base_excess",
        "locked_foreign",
        "operated_order_list",
        "base_deficit",
    ]
    .map(|kind| metrics.conflicts.with_label_values(&[kind]).get());
    ensure!(counted == [2, 1, 1, 1], "{counted:?}");
    // A scan that concludes nothing changes nothing.
    let mut reads = desk.reads(&pool, "1000", vec![Step::Fail(0, ReadFailure::Deferred)]);
    let routed = scan_once(&mut reads, &options, &scope, &writer, &metrics).await;
    ensure!(matches!(routed, Some(Routed::Unqualified(UnqualifiedReason::Deferred))));
    ensure!(metrics.conflict.get() == 1, "only a qualified Clean clears the page");
    // Nothing for the journal to persist, and nothing persisted.
    ensure!(incidents(&gate) == before && journal.retry_set_depth() == 0);
    ensure!(journal.flush_pending().await == 0);
    ensure!(desk.count(&pool, "recovery_incidents").await? == 0, "no incident row");
    let mut reads = desk.reads(&pool, "1000", vec![Step::Answer { btc: "0", bump: false }]);
    reads.calls = 50;
    let routed = scan_once(&mut reads, &options, &scope, &writer, &metrics).await;
    ensure!(matches!(routed, Some(Routed::Concluded(_))), "{routed:?}");
    ensure!(metrics.conflict.get() == 0, "a later qualified Clean clears the page");
    println!("SCAN_8K3C case=page conflicts={counted:?} incidents=0");
    Ok(())
}

/// C3, C9: a trading process fences through `apply`: with 8h's 2b cut in
/// this test database (as 8k-1 B's rehearsal applies it), the conflict
/// closes the fence and the journal's retry pass persists
/// `venue_discrepancy`; a later Clean concludes and clears nothing (R2): the
/// fence, the incident, its row and the gauge stand.
#[cfg(feature = "crash-points")]
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_trading_conflict_fences_and_persists(pool: PgPool) -> Result<()> {
    use robson_exec::gate::EpochView;
    let desk = Desk::new(&pool).await?;
    sqlx::query(
        "ALTER TABLE public.recovery_incidents VALIDATE CONSTRAINT recovery_incidents_reason_v38",
    )
    .execute(&pool)
    .await?;
    crate::venue_budget::n5_claim::rehearsal_c2::cut_legacy_reason(&pool).await?;
    let journal = durable(&pool, &desk);
    let gate = journal.gate().clone();
    gate.begin_epoch(EpochView { epoch_seq: 1, unclean_reason: None }, None, Vec::new());
    let writer = desk.writer(&pool, &gate);
    let options = pool.connect_options();
    let metrics = ScanMetrics::new(None);
    let open = |gate: &OperationGate| {
        let open = gate.open_incidents();
        open.iter().any(|incident| incident.reason == IncidentReason::VenueDiscrepancy)
    };
    ensure!(!gate.fenced() && !open(&gate), "open before the conflict");
    let rows = || {
        sqlx::query_as::<_, (String, i64)>(
            "SELECT reason, count FROM recovery_incidents WHERE tenant_id = $1 AND released_at IS NULL ORDER BY reason",
        )
        .bind(desk.f.tenant)
        .fetch_all(&pool)
    };
    let mut reads = desk.reads(&pool, "1000", vec![Step::Answer { btc: "0.5", bump: false }]);
    let scanned = scan_non_trading(&mut reads, &gate, &options, &desk.f.scope()).await;
    let routed =
        route(scanned.context("one attempt")?, &writer, ConflictSink::Fence, &metrics).await;
    ensure!(matches!(routed, Routed::Conflict), "{routed:?}");
    ensure!(gate.fenced() && open(&gate) && metrics.conflict.get() == 1, "fenced");
    ensure!(journal.flush_pending().await == 1, "the retry pass persists it");
    let persisted = rows().await?;
    ensure!(persisted == [("venue_discrepancy".to_string(), 1)], "{persisted:?}");
    let mut reads = desk.reads(&pool, "1000", vec![Step::Answer { btc: "0", bump: false }]);
    reads.calls = 10;
    let scanned = scan_non_trading(&mut reads, &gate, &options, &desk.f.scope()).await;
    let routed =
        route(scanned.context("one attempt")?, &writer, ConflictSink::Fence, &metrics).await;
    ensure!(matches!(routed, Routed::Concluded(_)), "{routed:?}");
    ensure!(
        gate.fenced() && open(&gate) && metrics.conflict.get() == 1,
        "R2: nothing cleared"
    );
    ensure!(rows().await? == persisted, "the row stands");
    println!("SCAN_8K3C case=fence rows={persisted:?}");
    Ok(())
}

/// C5: an unlinked BTCUSDT order is withheld: counted and warned with its
/// order id, nothing paged, nothing written and nothing sent (the port is
/// the scan's only venue handle and only reads; the proxy shows the scan's
/// two statements alone). Once the order is gone, the next scan concludes.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_non_trading_scan_sends_no_mutation(pool: PgPool) -> Result<()> {
    use tracing::Level;
    let desk = Desk::new(&pool).await?;
    let gate = gate(&desk.f.venue);
    let (proxy, options, writers) = proxied_pool(&pool).await?;
    let writer = desk.writer(&writers, &gate);
    let scope = desk.f.scope();
    let metrics = ScanMetrics::new(None);
    let before = incidents(&gate);
    let mut reads = desk.reads(&pool, "1000", vec![Step::Answer { btc: "0", bump: false }]);
    reads.orders = json!([order(7001)]);
    let statements = proxy.stats().statements;
    let (routed, events) = logged(scan_once(&mut reads, &options, &scope, &writer, &metrics)).await;
    ensure!(matches!(routed, Some(Routed::RecoveryWithheld)), "{routed:?}");
    let withheld = metrics.withheld.with_label_values(&["unlinked_operated_order"]).get();
    let flagged: Vec<_> = events.iter().filter(|(_, kind)| kind.is_some()).cloned().collect();
    let warned = Some("unlinked_operated_order".to_string());
    ensure!(flagged == [(Level::WARN, warned)] && withheld == 1, "{events:?}");
    ensure!(events.iter().all(|(level, _)| *level != Level::ERROR), "{events:?}");
    ensure!(metrics.conflict.get() == 0 && incidents(&gate) == before, "nothing paged");
    let sent = proxy.stats().statements - statements;
    ensure!((reads.calls, sent) == (1, 2), "one triple, the scan's two statements: {sent}");
    ensure!(desk.count(&pool, "cash_checkpoints").await? == 0, "no conclusion");
    reads.orders = json!([]);
    reads.script.push_back(Step::Answer { btc: "0", bump: false });
    let routed = scan_once(&mut reads, &options, &scope, &writer, &metrics).await;
    ensure!(matches!(routed, Some(Routed::Concluded(_))), "{routed:?}");
    Ok(())
}

/// C4, C10: a writer refusal ends the scan: one attempt and one refused
/// publication (at most four writer statements, the scan within its 16),
/// counted as a retry for the next scan except a dirty observation, which
/// the next scan observes anew; it then concludes. `Stale` comes from the
/// freshness floor: 60 s pass from the floor taken before the ACCOUNT
/// request, not from the response's arrival.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_writer_refusal_ends_the_scan(pool: PgPool) -> Result<()> {
    use CashRefusal as R;
    for case in ["version", "marks", "stale", "dirty"] {
        let desk = Desk::new(&pool).await?;
        let gate = gate(&desk.f.venue);
        let (proxy, options, writers) = proxied_pool(&pool).await?;
        let writer = desk.writer(&writers, &gate);
        let scope = desk.f.scope();
        let metrics = ScanMetrics::new(None);
        let mut reads = desk.reads(&pool, "1000", vec![Step::Answer { btc: "0", bump: false }]);
        // The stale case's margin on each side of the 60 s freshness is half
        // this hold: at least a second of scheduling between its measurement
        // and its checks.
        reads.hold = Duration::from_millis(2_000);
        if case == "dirty" {
            // Booked in another scope, inside the first receipt's interval.
            desk.fill(&pool, wall(10, 500)).await?;
        }
        let start = proxy.stats().statements;
        let scanned = scan_non_trading(&mut reads, &gate, &options, &scope).await;
        let mut scanned = scanned.context("one attempt")?;
        let scan = proxy.stats().statements - start;
        match case {
            "version" => desk.f.bump(&pool).await?,
            "marks" => {
                let _ = gate.record(IncidentReason::StreamSuspended);
            },
            "stale" => {
                // Measured now, the shift puts the arrival half the venue's
                // hold inside the writer's 60 s freshness and the floor (before
                // the request) at least half the hold past it, whatever the
                // scan took after the arrival.
                let fresh = Duration::from_secs(60);
                let shift = fresh
                    .checked_sub(reads.arrived[0].elapsed() + reads.hold / 2)
                    .context("an arrival inside the window")?;
                let cash = scanned.cash.as_mut().context("the writer's input")?;
                cash.received = cash.received.checked_sub(shift).context("monotonic")?;
                let arrived = reads.arrived[0].checked_sub(shift).context("monotonic")?;
                ensure!(arrived.elapsed() < fresh, "fresh from arrival");
                ensure!(cash.received.elapsed() > fresh, "stale from the floor");
            },
            _ => {},
        }
        let routed = route(scanned, &writer, ConflictSink::Page, &metrics).await;
        let published = proxy.stats().statements - start - scan;
        let refused = match case {
            "version" => matches!(routed, Routed::WriterRefused(R::VersionMoved)),
            "marks" => matches!(routed, Routed::WriterRefused(R::Marks)),
            "stale" => matches!(routed, Routed::WriterRefused(R::Stale)),
            _ => matches!(routed, Routed::WriterRefused(R::DirtyObservation)),
        };
        ensure!(refused, "{case}: {routed:?}");
        ensure!((reads.calls, scan) == (1, 2), "{case}: one attempt");
        ensure!((1..=4).contains(&published) && scan + published <= 16, "{case}: {published}");
        let retried = u64::from(case != "dirty");
        ensure!(metrics.retries.get() == retried, "{case}: {}", metrics.retries.get());
        let dirty = metrics.refusals.with_label_values(&["publish", "dirty_observation"]);
        ensure!(dirty.get() == u64::from(case == "dirty"), "{case}");
        reads.script.push_back(Step::Answer { btc: "0", bump: false });
        let routed = scan_once(&mut reads, &options, &scope, &writer, &metrics).await;
        ensure!(matches!(routed, Some(Routed::Concluded(_))), "{case}: next {routed:?}");
        println!("SCAN_8K3C case=refusal-{case} scan_sql={scan} writer_sql={published}");
    }
    Ok(())
}

// ---- generated schedules through the production attempt and writer ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Occurrence,
    /// A pending rise acknowledged at once: the generation moves, nothing
    /// stays pending.
    PendingAck,
    /// A pending rise acknowledged only before the next scan.
    Pending,
    Bump,
    /// A fill booked inside the first receipt's interval, with its bump.
    FillInside,
    /// A balance successor's epoch.
    Successor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Point {
    BeforeMarks,
    PreToTriple,
    TripleToPost,
    PostToWriter,
    /// The invalidator holds the accounting rows when the writer takes them.
    RowsHeld,
    /// The writer holds the rows, stopped at its context read.
    AfterContext,
    /// The writer is past its final check, stopped at its first write.
    BeforeCommit,
    AfterCommit,
}

const CONTEXT_HELD: &str = "LOCK TABLE public.cash_checkpoints IN ACCESS EXCLUSIVE MODE";
const WRITE_HELD: &str = "LOCK TABLE public.cash_discrepancies IN EXCLUSIVE MODE";

#[cfg(feature = "crash-points")]
type Owner = robson_exec::gate::PendingExecutionOwner;
#[cfg(feature = "crash-points")]
type Ticket = robson_exec::gate::PendingExecutionTicket;
#[cfg(not(feature = "crash-points"))]
type Owner = ();
#[cfg(not(feature = "crash-points"))]
type Ticket = ();

/// A gate whose pending slot is bound (crash-points builds).
#[cfg(feature = "crash-points")]
fn bound(venue: &VenueId) -> (Arc<OperationGate>, Option<Owner>) {
    let (gate, owner) = pending::bound(venue);
    (gate, Some(owner))
}

#[cfg(not(feature = "crash-points"))]
fn bound(venue: &VenueId) -> (Arc<OperationGate>, Option<Owner>) {
    (gate(venue), None)
}

#[cfg(feature = "crash-points")]
fn rise(owner: Option<&Owner>, tickets: &Mutex<Vec<Ticket>>, acknowledge: bool) {
    let owner = owner.expect("a bound slot");
    let ticket = pending::rise(owner).expect("issued");
    match acknowledge {
        true => assert!(owner.acknowledge(ticket), "acknowledged"),
        false => tickets.lock().expect("tickets").push(ticket),
    }
}

#[cfg(not(feature = "crash-points"))]
fn rise(_: Option<&Owner>, _: &Mutex<Vec<Ticket>>, _: bool) {
    unreachable!("pending evidence registers in crash-points builds only")
}

#[cfg(feature = "crash-points")]
fn acknowledge(owner: Option<&Owner>, tickets: &Mutex<Vec<Ticket>>) {
    let tickets: Vec<_> = tickets.lock().expect("tickets").drain(..).collect();
    for ticket in tickets {
        assert!(owner.expect("a bound slot").acknowledge(ticket), "acknowledged");
    }
}

#[cfg(not(feature = "crash-points"))]
fn acknowledge(_: Option<&Owner>, _: &Mutex<Vec<Ticket>>) {}

async fn invalidate(
    pool: &PgPool,
    desk: &Desk,
    gate: &OperationGate,
    owner: Option<&Owner>,
    tickets: &Mutex<Vec<Ticket>>,
    kind: Kind,
) {
    match kind {
        Kind::Occurrence => {
            let _ = gate.record(IncidentReason::StreamSuspended);
        },
        Kind::PendingAck => rise(owner, tickets, true),
        Kind::Pending => rise(owner, tickets, false),
        Kind::Bump => desk.f.bump(pool).await.expect("bump"),
        Kind::FillInside => desk.fill(pool, wall(10, 500)).await.expect("fill"),
        Kind::Successor => {
            let start = process_epoch::start_balance_epoch(pool, desk.f.tenant, "8k-3c-successor");
            let _successor = start.await.expect("successor");
        },
    }
}

/// Until `n` sessions of this test's database wait on a lock.
async fn waiting(pool: &PgPool, n: i64) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock'")
                .fetch_one(pool)
                .await?;
            if waiting >= n {
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await?
}

/// `work` (the route) while this test holds `lock`, which stops the writer;
/// once it waits, `invalidate` runs, and a database invalidator queues
/// behind the writer's accounting rows before the lock is released (inside
/// the writer's 300 ms lock bound).
async fn stalled<T>(
    pool: &PgPool,
    lock: &str,
    database: bool,
    work: impl Future<Output = T>,
    invalidate: impl Future<Output = ()>,
) -> Result<T> {
    let mut held = pool.begin().await?;
    sqlx::query(lock).execute(&mut *held).await?;
    let release = async move {
        waiting(pool, 1).await?;
        let commit = async move {
            if database {
                waiting(pool, 2).await?;
            }
            held.commit().await?;
            anyhow::Ok(())
        };
        let ((), committed) = tokio::join!(invalidate, commit);
        committed
    };
    let (output, released) = tokio::join!(work, release);
    released?;
    Ok(output)
}

/// A conclusion lands only when no invalidator is ordered before the
/// writer's final check, or the scan observed anew after it.
fn expected(kind: Kind, point: Point, routed: &Routed) -> bool {
    use CashRefusal as R;
    use Kind as K;
    use Point as P;
    use UnqualifiedReason as U;
    let memory = matches!(kind, K::Occurrence | K::PendingAck | K::Pending);
    match (point, kind) {
        (P::BeforeMarks | P::PreToTriple | P::TripleToPost, K::Pending) => {
            matches!(routed, Routed::Unqualified(U::PendingEvidence))
        },
        (P::BeforeMarks | P::PreToTriple | P::TripleToPost, K::Successor) => {
            matches!(routed, Routed::Unqualified(U::Scope))
        },
        (P::BeforeMarks, K::FillInside) => {
            matches!(routed, Routed::WriterRefused(R::DirtyObservation))
        },
        (P::PostToWriter | P::AfterContext, _) if memory => {
            matches!(routed, Routed::WriterRefused(R::Marks))
        },
        (P::PostToWriter | P::RowsHeld, K::Bump | K::FillInside) => {
            matches!(routed, Routed::WriterRefused(R::VersionMoved))
        },
        (P::PostToWriter, _) => matches!(routed, Routed::WriterRefused(R::VersionMoved | R::Epoch)),
        _ => matches!(routed, Routed::Concluded(_)),
    }
}

fn expected_attempts(kind: Kind, point: Point) -> u8 {
    match (point, kind) {
        (Point::BeforeMarks | Point::PreToTriple | Point::TripleToPost, Kind::Pending) => 3,
        (Point::PreToTriple | Point::TripleToPost, _) => 2,
        _ => 1,
    }
}

/// At least 24 schedules (PLAN-r2 §8): each invalidator (an occurrence; a
/// pending rise, acknowledged at once or later; a bumping ledger write; a
/// fill booked inside the observation's interval; a balance successor's
/// epoch) at every point of an attempt and of its publication, through the
/// production attempt, scan, route and cash writer, each scan concluding
/// missing cash (900 against 1000). After each: a refusal wrote nothing; a
/// fill booked after the COMMIT inside the concluded interval is reported
/// dirty by `read_cash_state`; the next scan concludes (an old process after
/// its successor never does). Deterministic boundary schedules, the writer
/// stopped on database locks at its points; the generated concurrent races
/// follow (`concurrent_invalidators_never_publish_an_invalidated_observation`).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn generated_interleavings_conclude_only_after_every_invalidator(pool: PgPool) -> Result<()> {
    use Kind as K;
    use Point as P;
    let mut kinds = vec![K::Occurrence, K::Bump, K::FillInside, K::Successor];
    if cfg!(feature = "crash-points") {
        kinds.extend([K::PendingAck, K::Pending]);
    }
    let points = [
        P::BeforeMarks,
        P::PreToTriple,
        P::TripleToPost,
        P::PostToWriter,
        P::RowsHeld,
        P::AfterContext,
        P::BeforeCommit,
        P::AfterCommit,
    ];
    let options = pool.connect_options();
    let metrics = ScanMetrics::new(None);
    let (mut schedules, mut concluded) = (0_u32, 0_u32);
    for kind in kinds {
        for point in points {
            if point == P::RowsHeld && !matches!(kind, K::Bump | K::FillInside) {
                continue;
            }
            let desk = Desk::new(&pool).await?;
            let (gate, owner) = bound(&desk.f.venue);
            let writer = desk.writer(&pool, &gate);
            let scope = desk.f.scope();
            let tickets = Mutex::new(Vec::new());
            let hit = || invalidate(&pool, &desk, &gate, owner.as_ref(), &tickets, kind);
            let script = (0..4).map(|_| Step::Answer { btc: "0", bump: false }).collect();
            let mut reads = desk.reads(&pool, "900", script);
            if point == P::BeforeMarks {
                hit().await;
            }
            if matches!(point, P::PreToTriple | P::TripleToPost) {
                let hook: Hook<'_> = Box::pin(hit());
                reads.hook = Some((point == P::TripleToPost, hook));
            }
            let scanned = scan_non_trading(&mut reads, &gate, &options, &scope).await;
            let scanned = scanned.context("one attempt")?;
            let attempts = scanned.outcome.attempts;
            if point == P::PostToWriter {
                hit().await;
            }
            let memory = matches!(kind, K::Occurrence | K::PendingAck | K::Pending);
            let routing = route(scanned, &writer, ConflictSink::Page, &metrics);
            let routed = match point {
                P::RowsHeld => {
                    let mut tx = pool.begin().await?;
                    match kind {
                        K::FillInside => desk.fill_in(&mut tx, wall(10, 500)).await?,
                        _ => {
                            sqlx::query("UPDATE accounting_state SET version = version + 1 WHERE tenant_id=$1")
                                .bind(desk.f.tenant)
                                .execute(&mut *tx)
                                .await?;
                        },
                    }
                    // Released once the writer waits on the rows, inside its
                    // 300 ms lock bound, as `stalled` releases its locks.
                    let pool = &pool;
                    let release = async move {
                        waiting(pool, 1).await?;
                        tx.commit().await?;
                        anyhow::Ok(())
                    };
                    let (routed, committed) = tokio::join!(routing, release);
                    committed?;
                    routed
                },
                P::AfterContext => stalled(&pool, CONTEXT_HELD, !memory, routing, hit()).await?,
                P::BeforeCommit => stalled(&pool, WRITE_HELD, !memory, routing, hit()).await?,
                _ => routing.await,
            };
            if point == P::AfterCommit {
                hit().await;
            }
            let label = format!("{kind:?} at {point:?} after {attempts} attempts: {routed:?}");
            ensure!(expected(kind, point, &routed), "{label}");
            ensure!(attempts == expected_attempts(kind, point), "{label}");
            let landed = matches!(routed, Routed::Concluded(_));
            ensure!(desk.count(&pool, "cash_discrepancies").await? == i64::from(landed), "{label}");
            let late = kind == K::FillInside
                && matches!(point, P::AfterContext | P::BeforeCommit | P::AfterCommit);
            let state = desk.state(&pool).await?;
            ensure!(state.open_missing == (landed && !late), "{label}: {state:?}");
            acknowledge(owner.as_ref(), &tickets);
            reads.calls = 10;
            let next = scan_once(&mut reads, &options, &scope, &writer, &metrics).await;
            let next_ok = match kind {
                K::Successor => matches!(next, Some(Routed::Unqualified(UnqualifiedReason::Scope))),
                _ => matches!(next, Some(Routed::Concluded(_))),
            };
            ensure!(next_ok, "{label}; next scan: {next:?}");
            schedules += 1;
            concluded += u32::from(landed);
        }
    }
    ensure!(schedules >= 24, "{schedules} schedules");
    println!("SCAN_8K3C case=interleavings schedules={schedules} concluded={concluded}");
    Ok(())
}

// ---- generated races on a multi-threaded runtime ----

/// One generated invalidator: its kind and its delay after the case starts.
#[derive(Debug, Clone, Copy)]
struct Racer {
    kind: Kind,
    delay_ms: u64,
}

/// Whether anything that refuses publication holds against `cash`: the
/// gate's invalidity marks, the accounting version or the tenant's live epoch
/// moved since it was captured (each only moves forward), or a fill booked
/// inside its interval (fills are immutable).
async fn invalidated(
    pool: &PgPool,
    desk: &Desk,
    gate: &OperationGate,
    cash: &CashAttempt,
) -> Result<bool> {
    let marks = gate.invalidity_marks();
    let version = desk.f.version(pool).await?;
    let epoch: Uuid = sqlx::query_scalar(
        "SELECT epoch_id FROM process_epochs WHERE tenant_id=$1 ORDER BY epoch_seq DESC LIMIT 1",
    )
    .bind(desk.f.tenant)
    .fetch_one(pool)
    .await?;
    let BaseVerdict::Clean { interval } = &cash.verdict else {
        anyhow::bail!("the writer's input is a Clean: {cash:?}");
    };
    let at = |ns: i128, up: i128| {
        DateTime::from_timestamp_micros(i64::try_from(ns.div_euclid(1_000) + up).ok()?)
    };
    let (start, end) = (at(interval.start_ns, 0), at(interval.end_ns, 1));
    let dirty: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM order_fills WHERE tenant_id=$1 AND trade_time BETWEEN $2 AND $3)",
    )
    .bind(desk.f.tenant)
    .bind(start.context("start")?)
    .bind(end.context("end")?)
    .fetch_one(pool)
    .await?;
    let marked = marks != cash.marks || marks.pending.blocks_publication();
    Ok(marked || version != cash.version || epoch != desk.f.epoch || dirty)
}

/// One generated case: a fresh desk; its racers spawned on the runtime's
/// workers, each after its delay; the scan (its triple held `hold` ms) and
/// its route on this task. True when the scan concluded.
async fn race(
    pool: &PgPool,
    options: &PgConnectOptions,
    racers: &[Racer],
    hold: u64,
) -> Result<bool> {
    let desk = Arc::new(Desk::new(pool).await?);
    let (gate, owner) = bound(&desk.f.venue);
    let owner = owner.map(Arc::new);
    let tickets = Arc::new(Mutex::new(Vec::new()));
    let spawned: Vec<_> = racers
        .iter()
        .map(|racer| {
            let (pool, desk, gate) = (pool.clone(), desk.clone(), gate.clone());
            let (owner, tickets, racer) = (owner.clone(), tickets.clone(), *racer);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(racer.delay_ms)).await;
                invalidate(&pool, &desk, &gate, owner.as_deref(), &tickets, racer.kind).await;
            })
        })
        .collect();
    let writer = desk.writer(pool, &gate);
    let scope = desk.f.scope();
    let metrics = ScanMetrics::new(None);
    let script = (0..4).map(|_| Step::Answer { btc: "0", bump: false }).collect();
    let mut reads = desk.reads(pool, "900", script);
    reads.hold = Duration::from_millis(hold);
    let scanned = scan_non_trading(&mut reads, &gate, options, &scope).await;
    let scanned = scanned.context("one attempt")?;
    let cash = scanned.cash.clone();
    // Whatever invalidated the attempt before the route is called refuses
    // publication; when nothing did by the time it returns, the conclusion
    // lands (a fill already booked inside the interval when the scan began
    // included: the writer refuses it as dirty, as the boundary schedule
    // `FillInside` at `BeforeMarks` shows).
    let before = match &cash {
        Some(cash) => invalidated(pool, &desk, &gate, cash).await?,
        None => true,
    };
    let routed = route(scanned, &writer, ConflictSink::Page, &metrics).await;
    let landed = matches!(routed, Routed::Concluded(_));
    let label = format!("{racers:?} held {hold} ms: {routed:?}");
    ensure!(!(before && landed), "published an invalidated observation: {label}");
    if let Some(cash) = &cash {
        let after = invalidated(pool, &desk, &gate, cash).await?;
        ensure!(after || landed, "nothing invalidated it, yet no conclusion: {label}");
    }
    for racer in spawned {
        racer.await?;
    }
    // A refusal wrote nothing; a fill booked inside the concluded interval
    // after its publication leaves no clean missing evidence.
    ensure!(desk.count(pool, "cash_discrepancies").await? == i64::from(landed), "{label}");
    let fill = i128::from(wall(10, 500).timestamp_micros()) * 1_000;
    let inside = match cash.as_ref().map(|cash| &cash.verdict) {
        Some(BaseVerdict::Clean { interval }) => {
            (interval.start_ns..=interval.end_ns).contains(&fill)
        },
        _ => false,
    };
    let late = landed && inside && racers.iter().any(|racer| racer.kind == Kind::FillInside);
    ensure!(desk.state(pool).await?.open_missing == (landed && !late), "{label}");
    // The next scheduled scan, the pending evidence acknowledged.
    acknowledge(owner.as_deref(), &tickets);
    reads.calls = 10;
    let next = scan_once(&mut reads, options, &scope, &writer, &metrics).await;
    let next_ok = match racers.iter().any(|racer| racer.kind == Kind::Successor) {
        true => matches!(next, Some(Routed::Unqualified(UnqualifiedReason::Scope))),
        false => matches!(next, Some(Routed::Concluded(_))),
    };
    ensure!(next_ok, "{label}; next scan: {next:?}");
    Ok(landed)
}

/// PLAN-r2 §8's generated interleavings, multithreaded: proptest draws one to
/// three invalidators of any kind (an occurrence; a pending rise acknowledged
/// at once or before the next scan; a bumping ledger write; a fill booked
/// inside the first receipt's interval; a balance successor's epoch), each
/// after its own delay of 0 to 400 ms, racing one scan whose triple is held 0
/// to 200 ms and its publication, on a four-worker runtime; 24 cases or more.
/// Sound under any interleaving because marks, versions and epochs only move
/// forward and fills are immutable: anything that invalidated the published
/// attempt (moved since its capture, or a fill inside its interval) before
/// the route's call refuses publication; when nothing did by the time the
/// route returned, the conclusion lands; a refusal writes nothing; at most
/// one pending invalidator per case (the fixture's slot takes one rise per
/// gate); a fill booked inside a concluded interval is reported dirty
/// afterwards; the next scan concludes (or refuses `Scope` after a
/// successor's epoch).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn concurrent_invalidators_never_publish_an_invalidated_observation(
    pool: PgPool,
) -> Result<()> {
    use proptest::{
        prelude::*,
        test_runner::{Config, TestRunner},
    };
    let options = (*pool.connect_options()).clone();
    let racing = std::thread::spawn(move || -> Result<(u32, u32)> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?;
        let connect = PgPoolOptions::new().max_connections(8).connect_with(options.clone());
        let pool = runtime.block_on(connect)?;
        let mut kinds = vec![
            Kind::Occurrence,
            Kind::Bump,
            Kind::FillInside,
            Kind::Successor,
        ];
        if cfg!(feature = "crash-points") {
            kinds.extend([Kind::PendingAck, Kind::Pending]);
        }
        let racer = (prop::sample::select(kinds), 0_u64..400)
            .prop_map(|(kind, delay_ms)| Racer { kind, delay_ms });
        // The fixture's pending slot takes one rise per gate (its evidence
        // never advances): at most one pending invalidator per case.
        let pending = |racers: &Vec<Racer>| {
            let slot = |racer: &&Racer| matches!(racer.kind, Kind::Pending | Kind::PendingAck);
            racers.iter().filter(slot).count() <= 1
        };
        let racers = prop::collection::vec(racer, 1..=3).prop_filter("one pending rise", pending);
        let strategy = (racers, 0_u64..200);
        let config = Config {
            cases: 24,
            failure_persistence: None,
            ..Config::default()
        };
        let (cases, concluded) = (std::cell::Cell::new(0_u32), std::cell::Cell::new(0_u32));
        let outcome = TestRunner::new(config).run(&strategy, |(racers, hold)| {
            let case = runtime.block_on(race(&pool, &options, &racers, hold));
            let landed = case.map_err(|e| TestCaseError::fail(format!("{e:#}")))?;
            cases.set(cases.get() + 1);
            concluded.set(concluded.get() + u32::from(landed));
            Ok(())
        });
        runtime.block_on(pool.close());
        outcome.map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok((cases.get(), concluded.get()))
    });
    let (cases, concluded) = racing.join().map_err(|_| anyhow::anyhow!("the race panicked"))??;
    ensure!(cases >= 24, "{cases} cases");
    println!("SCAN_8K3C case=concurrent cases={cases} concluded={concluded}");
    Ok(())
}

// ---- source guards and the metrics' export ----

/// Every production source of the module (`.rs` and `.sql`, in every
/// directory under it), its test files aside: `tests.rs`, `pg_tests.rs` and
/// the `pg_tests/` directory.
fn module_sources() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/balance_reconciliation");
    let (mut sources, mut dirs) = (Vec::new(), vec![root.clone()]);
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("a module directory") {
            let path = entry.expect("an entry").path();
            let name = path.strip_prefix(&root).expect("under the module");
            let name = name.to_string_lossy().to_string();
            match (path.is_dir(), name.as_str()) {
                (true, "pg_tests") => {},
                (true, _) => dirs.push(path),
                (false, "tests.rs" | "pg_tests.rs") => {},
                (false, _) if name.ends_with(".rs") || name.ends_with(".sql") => {
                    sources.push((name, std::fs::read_to_string(&path).expect("a source")));
                },
                (false, _) => {},
            }
        }
    }
    sources
}

/// C7 (PLAN-r2 §1 item 4): a BTC conflict is fenced or paged, never sold.
/// No production source of the module names a sell side, a sweep close, an
/// intent kind, a gate mutation, a placement, a cancel or a send.
#[test]
fn btc_excess_is_never_sold() {
    let sources = module_sources();
    let names: Vec<_> = sources.iter().map(|(name, _)| name.as_str()).collect();
    for file in [
        "mod.rs",
        "classify.rs",
        "attempt.rs",
        "route.rs",
        "non_trading.sql",
    ] {
        assert!(names.contains(&file), "{file} not scanned: {names:?}");
    }
    let forbidden = [
        "Sell",
        "SELL",
        "SweepClose",
        "IntentKind",
        "mutation(",
        "place_",
        "cancel_order",
        "cancel_algo",
        ".cancel(",
        "close_position",
        "begin_send",
        "submit(",
    ];
    for (name, text) in &sources {
        for needle in forbidden {
            assert!(!text.contains(needle), "{name} names {needle}");
        }
    }
}

/// What a conclusion must never carry: an anchor, flow, re-anchor or
/// invalidation table, a reopen, a bump of its own, an SQL write or a row
/// lock, matched in any case, across line breaks and inside Rust string
/// literals (words split at every character outside identifiers).
fn accounting_writes(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
    let words: Vec<&str> = lower.split(|c: char| !word(c)).filter(|w| !w.is_empty()).collect();
    let flat = format!(" {} ", words.join(" "));
    let names = [
        "cash_anchors",
        "cash_attested_flows",
        "cash_reanchors",
        "accounting_invalidations",
        "reopen",
        "truncate",
        "upsert",
        ".advance(",
    ];
    let phrases = [
        " insert into ",
        " delete from ",
        " for update ",
        " for no key update ",
        " for share ",
        " for key share ",
    ];
    let mut found: Vec<String> = names
        .iter()
        .filter(|name| lower.contains(*name))
        .chain(phrases.iter().filter(|phrase| flat.contains(*phrase)))
        .map(|needle| needle.trim().to_string())
        .collect();
    let set = |at: usize| words[at + 1..].iter().take(4).any(|word| *word == "set");
    if words.iter().enumerate().any(|(at, word)| *word == "update" && set(at)) {
        found.push("update … set".into());
    }
    found
}

/// A conclusion is evidence, never accounting (PLAN-r2 §6.6): the module
/// names no anchor, flow, re-anchor or invalidation table and no reopen, and
/// writes no row, takes no row lock and makes no bump of its own.
#[test]
fn cash_snapshot_is_not_reanchor() {
    for example in [
        "UPDATE public.accounting_state\n    SET version = version + 1",
        "UPDATE public.cash_discrepancies d\n SET state = 'void'",
        "INSERT\nINTO public.cash_checkpoints VALUES (1)",
        "SELECT version FROM accounting_state\n FOR\n UPDATE",
        r#"sqlx::query("UPDATE public.accounting_state SET version = version + 1")"#,
        r#"sqlx::query("INSERT INTO public.cash_checkpoints VALUES (1)").execute(c)"#,
        r#"const ROWS: &str = "SELECT version FROM accounting_state FOR UPDATE";"#,
    ] {
        assert!(!accounting_writes(example).is_empty(), "{example}");
    }
    for (name, text) in module_sources() {
        let found = accounting_writes(&text);
        assert!(found.is_empty(), "{name} carries {found:?}");
    }
}

/// The registered set exports every finite series at 0 (PLAN-r2 §6.10), so
/// an absent series means a missing daemon, not a quiet scan.
#[test]
fn the_scan_metrics_export_at_zero() {
    std::sync::LazyLock::force(&SCAN_METRICS);
    let rendered = crate::metrics::render();
    let labelled = |name: &str, label: &str, values: &[&str]| {
        values
            .iter()
            .map(|value| format!("{name}{{{label}=\"{value}\"}} 0"))
            .collect::<Vec<_>>()
    };
    let mut zero = labelled("robsond_balance_scans_total", "outcome", &OUTCOMES);
    zero.extend(labelled("robsond_spot_btc_conflicts_total", "finding", &[
        "base_deficit",
        "base_excess",
        "locked_foreign",
        "operated_order_list",
    ]));
    zero.extend(labelled("robsond_balance_recovery_withheld_total", "kind", &[
        "locked_short",
        "unlinked_operated_order",
        "needs_exact_evidence",
    ]));
    for plain in [
        "robsond_balance_scan_attempts_total",
        "robsond_balance_scan_weight_total",
        "robsond_balance_scan_last_concluded_timestamp_seconds",
        "robsond_cash_observation_retries_total",
        "robsond_spot_btc_conflict",
    ] {
        zero.push(format!("{plain} 0"));
    }
    for line in zero {
        assert!(rendered.lines().any(|l| l == line), "{line} not exported:\n{rendered}");
    }
}

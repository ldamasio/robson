//! 8k-3a proofs on PostgreSQL (PLAN-r2 §8): the non-trading scope's pre and
//! post statement through the one connection owner, the capture's
//! invalidity marks before pre and after post, the pre reader's bounded
//! materialization, statement counts by proxy and the statement's bound at
//! 8l's cardinality.
use std::{sync::Arc, time::Duration};

use anyhow::{ensure, Context, Result};
use robson_exec::{
    gate::{IncidentReason, OperationGate, PendingEvidence},
    order_journal::{OrderJournal, StubJournalBackend},
    venue::VenueId,
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use sqlx::{
    postgres::{PgArguments, PgConnectOptions},
    query::Query,
    PgPool, Postgres, Row,
};
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    snapshot::{read_non_trading, PRE_ROW_CAP},
    *,
};

const UID: &str = "4242";
const NON_TRADING: &str = include_str!("non_trading.sql");

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn interval() -> ObservationInterval {
    ObservationInterval { start_ns: 0, end_ns: 1, cut_ns: 0 }
}

fn valid() -> ClockOutcome {
    ClockOutcome::Valid(interval())
}

/// A gate with no epoch and no pending slot (its evidence is Unbound).
fn gate(venue: &VenueId) -> Arc<OperationGate> {
    OrderJournal::new(Arc::new(StubJournalBackend::default()), venue).gate().clone()
}

/// One balance-account tenant bound to its scope's UID, on its live epoch,
/// with an empty Spot ledger and no order-set generation.
struct Fixture {
    tenant: Uuid,
    scope: String,
    epoch: Uuid,
    venue: VenueId,
}

impl Fixture {
    async fn new(pool: &PgPool) -> Result<Self> {
        let tenant = Uuid::now_v7();
        Self::bound(pool, tenant, VenueId::BINANCE_SPOT, &format!("nt-{tenant}"), Uuid::now_v7())
            .await
    }

    /// The same with every identity chosen, a malformed one included.
    async fn bound(
        pool: &PgPool,
        tenant: Uuid,
        venue: &str,
        scope: &str,
        epoch: Uuid,
    ) -> Result<Self> {
        sqlx::query("INSERT INTO accounting_state (tenant_id,venue_id,budget_model) VALUES ($1,$2,'net_from_start_non_expanding_v1')")
            .bind(tenant).bind(venue).execute(pool).await?;
        sqlx::query("INSERT INTO venue_accounts (venue_id,account_scope,tenant_id,uid,bound_at,cutover_id) VALUES ($1,$2,$3,$4,now(),$5)")
            .bind(venue).bind(scope).bind(tenant).bind(UID).bind(Uuid::now_v7()).execute(pool).await?;
        Self::epoch_of(pool, tenant, epoch).await?;
        Ok(Self {
            tenant,
            scope: scope.to_string(),
            epoch,
            venue: VenueId::new(venue),
        })
    }

    async fn epoch_of(pool: &PgPool, tenant: Uuid, epoch: Uuid) -> Result<()> {
        sqlx::query("INSERT INTO process_epochs (epoch_id,tenant_id,binary_version) VALUES ($1,$2,'8k-3a-fixture')")
            .bind(epoch).bind(tenant).execute(pool).await?;
        Ok(())
    }

    fn scope_as<'a>(&'a self, account_scope: &'a str, uid: &'a str) -> ScanScope<'a> {
        ScanScope {
            detector: DetectorScope {
                expected: ExpectedGeneration {
                    tenant_id: self.tenant,
                    venue: &self.venue,
                    account_scope,
                    generation_id: Uuid::nil(),
                    process_epoch: self.epoch,
                    source_audit_id: Uuid::nil(),
                },
                uid,
                operated: "BTCUSDT",
                base_asset: "BTC",
            },
            quote_asset: "USDT",
        }
    }

    fn scope(&self) -> ScanScope<'_> {
        self.scope_as(&self.scope, UID)
    }

    async fn version(&self, pool: &PgPool) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT version FROM accounting_state WHERE tenant_id=$1")
            .bind(self.tenant)
            .fetch_one(pool)
            .await?)
    }

    /// A bump, as every ledger write makes in its own transaction.
    async fn bump(&self, pool: &PgPool) -> Result<()> {
        sqlx::query("UPDATE accounting_state SET version = version + 1 WHERE tenant_id=$1")
            .bind(self.tenant)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// An open leg of `scope` (its route and admission first), no bump.
    async fn leg(&self, pool: &PgPool, scope: &str) -> Result<()> {
        let (leg, admission) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query("INSERT INTO leg_routes VALUES ($1,$2,'binance:spot',$3,now())")
            .bind(leg)
            .bind(self.tenant)
            .bind(scope)
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO balance_leg_admissions (entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,reference_price,remainder_reserve,admission_charge,initial_trigger,created_at)
            VALUES ($1,$2,$3,'binance:spot',$4,'BTCUSDT','2026-11-01',10,10,2,5,0.00001,'base','fixture rules',now(),'fixture-hash',1,1,100,0.1,1,90,now())")
            .bind(admission).bind(leg).bind(self.tenant).bind(scope).execute(pool).await?;
        sqlx::query("INSERT INTO balance_legs (position_id,tenant_id,venue_id,account_scope,symbol,admission_intent_id,decided_month,target_trigger,created_at)
            VALUES ($1,$2,'binance:spot',$3,'BTCUSDT',$4,'2026-11-01',90,now())")
            .bind(leg).bind(self.tenant).bind(scope).bind(admission).execute(pool).await?;
        Ok(())
    }

    /// A discovered venue order of `scope`, no bump.
    async fn order(&self, pool: &PgPool, scope: &str) -> Result<()> {
        sqlx::query("INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,side,order_type,orig_qty,status,phase,last_checked_at,live_seen_at)
            VALUES ('binance:spot',$1,'BTCUSDT','7001',$2,'web-1','web-1','discovered','BUY','LIMIT',1,'NEW','live',now(),now())")
            .bind(scope).bind(self.tenant).execute(pool).await?;
        Ok(())
    }

    /// An intent of `venue` and `scope`, no bump.
    async fn intent(&self, pool: &PgPool, venue: &str, scope: &str) -> Result<()> {
        sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at) VALUES ($1,$2,$3,$4,$5,'entry','{}',now())")
            .bind(Uuid::now_v7()).bind(self.tenant).bind(venue).bind(scope).bind(Uuid::now_v7().simple().to_string())
            .execute(pool).await?;
        Ok(())
    }
}

/// The non-trading statement's binds (pre: no expected version).
fn non_trading<'q>(
    sql: &'q str,
    f: &'q Fixture,
    account_scope: &'q str,
) -> Query<'q, Postgres, PgArguments> {
    sqlx::query(sql)
        .bind(f.tenant)
        .bind(f.venue.as_str())
        .bind(account_scope)
        .bind(f.epoch)
        .bind(UID)
        .bind(None::<i64>)
}

/// A capture, `change` after its pre, then `still`.
async fn still_after(
    f: &Fixture,
    options: &PgConnectOptions,
    gate: &OperationGate,
    change: impl FnOnce(),
) -> Result<AttemptOutcome> {
    let capture = Capture::before(gate, options, &f.scope(), deadline()).await;
    let capture = capture.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    change();
    Ok(capture.still(gate, options, &f.scope(), deadline(), valid).await)
}

/// Connect options through a counting proxy.
async fn proxied(pool: &PgPool) -> Result<(DbProxy, PgConnectOptions)> {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .context("host")?;
    let proxy = DbProxy::start(upstream).await?;
    let database = options.get_database().context("database")?;
    let url = database_url_through(&std::env::var("DATABASE_URL")?, proxy.addr(), database)?;
    Ok((proxy, url.parse()?))
}

/// The capture's two statements, one fresh connection each and nothing
/// else: the pre (`v₀`) and the post with `v₀` (8l-5's two pre-reads).
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn non_trading_scope_reads_v0_on_an_empty_ledger(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    f.bump(&pool).await?;
    let gate = gate(&f.venue);
    let (proxy, options) = proxied(&pool).await?;
    proxy.enable_exact_query_trace(8);
    let start = proxy.stats();
    let capture = Capture::before(&gate, &options, &f.scope(), deadline()).await;
    let capture = capture.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let pre = proxy.stats();
    ensure!(
        capture.pre == PreSnapshot { version: 1, ..PreSnapshot::default() },
        "{capture:?}"
    );
    ensure!(capture.marks == gate.invalidity_marks(), "{capture:?}");
    ensure!(capture.marks.pending == PendingEvidence::Unbound);
    ensure!((pre.statements - start.statements, pre.connections - start.connections) == (1, 1));
    let outcome = capture.still(&gate, &options, &f.scope(), deadline(), valid).await;
    ensure!(outcome == AttemptOutcome::Qualified(interval()), "{outcome:?}");
    let post = proxy.stats();
    ensure!((post.statements - pre.statements, post.connections - pre.connections) == (1, 1));
    let trace = proxy.query_trace();
    ensure!(
        trace.len() == 2 && trace.iter().all(|s| s.contains("AS uid_bound")),
        "{trace:?}"
    );
    println!("NON_TRADING_COUNTS pre_sql=1 post_sql=1 connections=2 trace={}", trace.len());
    Ok(())
}

/// A3: a leg, a venue order or an intent of the scope refuses the scope
/// (permanent within a scan); rows of another scope or venue are ignored. A
/// row that appears between pre and post refuses the post.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_non_trading_scan_refuses_a_ledger_with_rows(pool: PgPool) -> Result<()> {
    let options = pool.connect_options();
    for row in ["leg", "order", "intent"] {
        let f = Fixture::new(&pool).await?;
        let gate = gate(&f.venue);
        let other = format!("{}-other", f.scope);
        // Another scope's ledger, another venue's journal: ignored.
        f.leg(&pool, &other).await?;
        f.order(&pool, &other).await?;
        f.intent(&pool, &other, &other).await?;
        f.intent(&pool, "binance:usdm", &f.scope).await?;
        let capture = Capture::before(&gate, &options, &f.scope(), deadline()).await;
        let capture = capture.map_err(|e| anyhow::anyhow!("{row}: {e:?}"))?;
        match row {
            "leg" => f.leg(&pool, &f.scope).await?,
            "order" => f.order(&pool, &f.scope).await?,
            _ => f.intent(&pool, "binance:spot", &f.scope).await?,
        }
        // The row came without a bump: the post still refuses it.
        let outcome = capture.still(&gate, &options, &f.scope(), deadline(), valid).await;
        let moved = AttemptOutcome::Unqualified(UnqualifiedReason::VersionChanged);
        ensure!(outcome == moved, "{row}: {outcome:?}");
        let refused = Capture::before(&gate, &options, &f.scope(), deadline()).await;
        ensure!(matches!(refused, Err(UnqualifiedReason::LedgerNotEmpty)), "{row}: {refused:?}");
        ensure!(UnqualifiedReason::LedgerNotEmpty.permanent());
    }
    Ok(())
}

/// No accounting row, another UID, a newer epoch and a clean-stopped epoch
/// each refuse the scope.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_non_trading_scope_refuses_an_unbound_or_stale_scope(pool: PgPool) -> Result<()> {
    let options = pool.connect_options();
    let refused = |result: Result<Capture, UnqualifiedReason>| {
        matches!(result, Err(UnqualifiedReason::Scope))
    };
    let f = Fixture::new(&pool).await?;
    let gate = gate(&f.venue);
    // No accounting row for this tenant.
    let unknown = Fixture {
        tenant: Uuid::now_v7(),
        scope: f.scope.clone(),
        epoch: f.epoch,
        venue: VenueId::new(VenueId::BINANCE_SPOT),
    };
    ensure!(refused(Capture::before(&gate, &options, &unknown.scope(), deadline()).await));
    ensure!(refused(
        Capture::before(&gate, &options, &f.scope_as(&f.scope, "4243"), deadline()).await
    ));
    ensure!(refused(
        Capture::before(&gate, &options, &f.scope_as("unbound", UID), deadline()).await
    ));
    ensure!(Capture::before(&gate, &options, &f.scope(), deadline()).await.is_ok());
    // A newer epoch of the tenant.
    let newer = Fixture::new(&pool).await?;
    Fixture::epoch_of(&pool, newer.tenant, Uuid::now_v7()).await?;
    ensure!(refused(Capture::before(&gate, &options, &newer.scope(), deadline()).await));
    // A clean-stopped epoch.
    sqlx::query(
        "UPDATE process_epochs SET clean_stop_at = now(), marker_version = 1 WHERE epoch_id = $1",
    )
    .bind(f.epoch)
    .execute(&pool)
    .await?;
    ensure!(refused(Capture::before(&gate, &options, &f.scope(), deadline()).await));
    Ok(())
}

/// Review r1: a malformed scope is refused before any connection, even with
/// backing rows the statement alone would match: another venue, a nil tenant,
/// a nil process epoch, a blank account scope; and a blank UID or pair.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_malformed_scope_is_refused_before_any_statement(pool: PgPool) -> Result<()> {
    let (proxy, options) = proxied(&pool).await?;
    let spot = VenueId::BINANCE_SPOT;
    let malformed = [
        Fixture::bound(&pool, Uuid::now_v7(), VenueId::BINANCE_USDM, "nt-usdm", Uuid::now_v7())
            .await?,
        Fixture::bound(&pool, Uuid::nil(), spot, "nt-nil-tenant", Uuid::now_v7()).await?,
        Fixture::bound(&pool, Uuid::now_v7(), spot, "nt-nil-epoch", Uuid::nil()).await?,
        Fixture::bound(&pool, Uuid::now_v7(), spot, " ", Uuid::now_v7()).await?,
    ];
    let gate = gate(&VenueId::new(spot));
    for f in &malformed {
        // The backing rows match: the statement alone would qualify it.
        let row = non_trading(NON_TRADING, f, &f.scope).fetch_one(&pool).await?;
        let flags: (i64, bool, bool, bool) = sqlx::FromRow::from_row(&row)?;
        ensure!(flags == (0, true, true, true), "{}: {flags:?}", f.scope);
        let before = proxy.stats().connections;
        let refused = Capture::before(&gate, &options, &f.scope(), deadline()).await;
        ensure!(matches!(refused, Err(UnqualifiedReason::Scope)), "{}: {refused:?}", f.scope);
        ensure!(proxy.stats().connections == before, "{}: no connection", f.scope);
    }
    let f = Fixture::new(&pool).await?;
    let mut blank = f.scope();
    blank.detector.operated = "";
    for scope in [f.scope_as(&f.scope, ""), blank] {
        let refused = Capture::before(&gate, &options, &scope, deadline()).await;
        ensure!(matches!(refused, Err(UnqualifiedReason::Scope)), "{refused:?}");
    }
    ensure!(proxy.stats().connections == 0, "nothing connected");
    ensure!(Capture::before(&gate, &options, &f.scope(), deadline()).await.is_ok());
    Ok(())
}

/// A4: a version change between pre and post refuses the attempt; one
/// before pre is only its `v₀`.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_version_change_between_pre_and_post_unqualifies(pool: PgPool) -> Result<()> {
    let options = pool.connect_options();
    let f = Fixture::new(&pool).await?;
    let gate = gate(&f.venue);
    f.bump(&pool).await?;
    let capture = Capture::before(&gate, &options, &f.scope(), deadline()).await;
    let capture = capture.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    ensure!(capture.pre.version == f.version(&pool).await?);
    f.bump(&pool).await?;
    let outcome = capture.still(&gate, &options, &f.scope(), deadline(), valid).await;
    ensure!(
        outcome == AttemptOutcome::Unqualified(UnqualifiedReason::VersionChanged),
        "{outcome:?}"
    );
    Ok(())
}

/// After pre: a version change, an occurrence or (crash-points) a pending
/// rise refuses at `still`; an unbound or clear slot passes.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn capture_still_refuses_a_version_or_marks_change(pool: PgPool) -> Result<()> {
    use UnqualifiedReason as U;
    let options = pool.connect_options();
    let f = Fixture::new(&pool).await?;
    let unbound = gate(&f.venue);
    let passed = still_after(&f, &options, &unbound, || ()).await?;
    ensure!(passed == AttemptOutcome::Qualified(interval()), "{passed:?}");
    let occurred = still_after(&f, &options, &unbound, || {
        let _ = unbound.record(IncidentReason::UncertainOutcome);
    })
    .await?;
    ensure!(occurred == AttemptOutcome::Unqualified(U::GenerationChanged), "{occurred:?}");
    let capture = Capture::before(&gate(&f.venue), &options, &f.scope(), deadline()).await;
    let capture = capture.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    f.bump(&pool).await?;
    let moved = capture.still(&gate(&f.venue), &options, &f.scope(), deadline(), valid).await;
    ensure!(moved == AttemptOutcome::Unqualified(U::VersionChanged), "{moved:?}");
    #[cfg(feature = "crash-points")]
    {
        let (gate, owner) = pending::bound(&f.venue);
        let clear = still_after(&f, &options, &gate, || ()).await?;
        ensure!(clear == AttemptOutcome::Qualified(interval()), "clear: {clear:?}");
        let mut ticket = None;
        let risen = still_after(&f, &options, &gate, || ticket = pending::rise(&owner)).await?;
        ensure!(ticket.is_some(), "issued");
        ensure!(risen == AttemptOutcome::Unqualified(U::PendingEvidence), "{risen:?}");
    }
    Ok(())
}

/// The ACCOUNT receipt's revalidation runs after the post and only refuses:
/// a sample rotated before it never qualifies the old capture.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn later_sample_never_qualifies_old_capture(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let gate = gate(&f.venue);
    let (proxy, options) = proxied(&pool).await?;
    let capture = Capture::before(&gate, &options, &f.scope(), deadline()).await;
    let capture = capture.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let at_pre = proxy.stats().statements;
    let mut seen = None;
    let rotated = || {
        seen = Some(proxy.stats().statements);
        ClockOutcome::Invalid
    };
    let outcome = capture.still(&gate, &options, &f.scope(), deadline(), rotated).await;
    ensure!(outcome == AttemptOutcome::Unqualified(UnqualifiedReason::Clock), "{outcome:?}");
    ensure!(seen == Some(at_pre + 1), "revalidated after the post: {seen:?} {at_pre}");
    Ok(())
}

/// A2 (F4): the marks are read before the pre statement. With the pre held
/// on a table lock, an invalidator moves the invalidity generation and leaves
/// nothing pending: an occurrence, or (crash-points) a pending rise
/// acknowledged at once. Both captures then read the same pending state, so
/// only the generation can refuse, and only marks read before pre see it.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_pending_rise_between_marks_and_pre_unqualifies(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let occurrence = gate(&f.venue);
    between_marks_and_pre(&pool, &f, &occurrence, || {
        let _ = occurrence.record(IncidentReason::UncertainOutcome);
    })
    .await?;
    #[cfg(feature = "crash-points")]
    {
        let (gate, owner) = pending::bound(&f.venue);
        between_marks_and_pre(&pool, &f, &gate, || {
            let ticket = pending::rise(&owner).expect("issued");
            assert!(owner.acknowledge(ticket), "acknowledged");
        })
        .await?;
    }
    Ok(())
}

async fn between_marks_and_pre(
    pool: &PgPool,
    f: &Fixture,
    gate: &OperationGate,
    invalidate: impl FnOnce(),
) -> Result<()> {
    let options = pool.connect_options();
    let mut lock = pool.begin().await?;
    sqlx::query("LOCK TABLE public.accounting_state IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await?;
    let hook = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND query LIKE '%AS uid_bound%'")
                    .fetch_one(pool)
                    .await?;
                if waiting == 1 {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        invalidate();
        lock.commit().await?;
        Ok::<_, anyhow::Error>(())
    };
    let scope = f.scope();
    let (capture, hooked) = tokio::join!(Capture::before(gate, &options, &scope, deadline()), hook);
    hooked?;
    let capture = capture.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let now = gate.invalidity_marks();
    ensure!(
        capture.marks.pending == now.pending && !now.pending.blocks_publication(),
        "{now:?}"
    );
    ensure!(now.generation == capture.marks.generation + 1, "{:?} {now:?}", capture.marks);
    let outcome = capture.still(gate, &options, &f.scope(), deadline(), valid).await;
    let refused = AttemptOutcome::Unqualified(UnqualifiedReason::GenerationChanged);
    ensure!(outcome == refused, "{outcome:?}");
    Ok(())
}

/// A13: 8k-2's pre reader holds at most `PRE_ROW_CAP + 1` rows. With 200
/// candidate intents the server returns 65 rows, refused as Cap before any
/// is decoded; 63 intents (64 rows) still decode.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_pre_read_returns_at_most_cap_plus_one_rows(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let (generation, audit) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO spot_order_set_generation (tenant_id,venue_id,account_scope,generation_id,process_epoch,source_audit_id,accounting_version,position_hints_complete) VALUES ($1,'binance:spot',$2,$3,$4,$5,0,true)")
        .bind(f.tenant).bind(&f.scope).bind(generation).bind(f.epoch).bind(audit)
        .execute(&pool).await?;
    sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at)
        SELECT gen_random_uuid(),$1,'binance:spot',$2,'cap-'||lpad(n::text,4,'0'),'entry','{}',now() FROM generate_series(1,200) n")
        .bind(f.tenant).bind(&f.scope).execute(&pool).await?;
    let scope = DetectorScope {
        expected: ExpectedGeneration {
            generation_id: generation,
            source_audit_id: audit,
            ..f.scope().detector.expected
        },
        ..f.scope().detector
    };
    let (proxy, options) = proxied(&pool).await?;
    let read = read_pre(&options, scope, deadline()).await;
    ensure!(read == Err(PreRefusal::Cap), "{read:?}");
    let rows = proxy.data_rows();
    ensure!(rows == 65, "data rows returned: {rows}");
    for (keep, expected) in [(64, Err(PreRefusal::Cap)), (63, Ok(63))] {
        sqlx::query("DELETE FROM order_intents WHERE account_scope=$1 AND client_order_id > 'cap-'||lpad($2::text,4,'0')")
            .bind(&f.scope).bind(keep).execute(&pool).await?;
        let read = read_pre(&pool.connect_options(), scope, deadline()).await;
        ensure!(read.map(|pre| pre.intents.len()) == expected, "{keep}");
    }
    // The statement's own bound, and none decoded: the cap is refused in
    // the connection owner, which never decodes; `read_pre` decodes after it.
    let pre = include_str!("btc_partition.sql");
    ensure!(pre.trim_end().ends_with(&format!("LIMIT {}", PRE_ROW_CAP + 1)));
    let source = include_str!("snapshot.rs");
    let cap = source.find("if rows.len() > PRE_ROW_CAP").context("cap check")?;
    let owner = source.find("async fn fetch(").context("owner")?;
    let decoded = source.find("    decode(&rows)").context("decode")?;
    ensure!(owner < cap && !source[owner..cap].contains("\n}\n") && decoded < owner);
    println!("PRE_CAP data_rows={rows} cap={PRE_ROW_CAP}");
    Ok(())
}

/// [AE] The non-trading statement at 8l's §7.2 cardinality (200 legs, half
/// closed, 2,000 intents and 1,000 venue orders in another scope of the
/// tenant): EXPLAIN (ANALYZE, BUFFERS) and a max of 20 runs, each far
/// inside 1 s, for the empty scope and for the full one.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_non_trading_statement_stays_within_its_bound(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let full = format!("{}-full", f.scope);
    let (tenant, scope) = (f.tenant, &full);
    sqlx::raw_sql(&format!("
        CREATE TEMP TABLE g AS SELECT n, ('10000000-0000-0000-0000-'||lpad(n::text,12,'0'))::uuid AS p,
            ('20000000-0000-0000-0000-'||lpad(n::text,12,'0'))::uuid AS a, n % 2 = 0 AS closed
            FROM generate_series(1,200) n;
        INSERT INTO leg_routes SELECT p,'{tenant}','binance:spot','{scope}','2026-10-01T00:00:00Z' FROM g;
        INSERT INTO balance_leg_admissions (entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,reference_price,remainder_reserve,admission_charge,initial_trigger,created_at)
            SELECT a,p,'{tenant}','binance:spot','{scope}','BTCUSDT','2026-10-01',10,10,2,5,0.00001,'base','fixture rules','2026-10-01T00:00:00Z','fixture-hash',1,1,100,0.1,1,90,'2026-10-01T00:00:00Z' FROM g;
        INSERT INTO balance_legs (position_id,tenant_id,venue_id,account_scope,symbol,admission_intent_id,decided_month,target_trigger,gross_qty,acquisition_cost,base_commission,base_commission_final,executable_qty,consumed_cum,created_at,closed_at)
            SELECT p,'{tenant}','binance:spot','{scope}','BTCUSDT',a,'2026-10-01',90,1,100,0.001,true,0.9,
                   CASE WHEN closed THEN 0.9 ELSE 0 END,'2026-10-01T00:00:00Z',CASE WHEN closed THEN timestamptz '2026-10-20T00:00:00Z' END FROM g;
        CREATE TEMP TABLE j AS SELECT g.p, k, ('30000000-0000-0000-'||lpad(g.n::text,4,'0')||'-'||lpad(k::text,12,'0'))::uuid AS i
            FROM g, generate_series(1,10) k;
        INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,venue_outcome,created_at)
            SELECT i,'{tenant}','binance:spot','{scope}',i::text,p,CASE WHEN k=1 THEN 'entry' WHEN k<=5 THEN 'protection' ELSE 'cancel' END,
                   '{{}}','success','2026-10-02T00:00:00Z' FROM j;
        INSERT INTO venue_orders (venue_id,account_scope,symbol,exchange_order_id,tenant_id,client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,orig_qty,stop_price,status,phase,executed_cum,fills_qty,last_checked_at,live_seen_at,terminal_seen_at)
            SELECT 'binance:spot','{scope}','BTCUSDT',(2000000000+row_number() OVER ())::text,'{tenant}',i::text,i::text,'leg',i,p,
                   CASE WHEN k=1 THEN 'entry' ELSE 'stop' END,CASE WHEN k=1 THEN 'BUY' ELSE 'SELL' END,'MARKET',1,
                   CASE WHEN k>1 THEN 90 END,'FILLED','terminal',1,1,'2026-10-03T00:00:00Z','2026-10-02T00:00:00Z','2026-10-03T00:00:00Z' FROM j WHERE k<=5;
        ANALYZE;"))
    .execute(&pool)
    .await?;
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM balance_legs), (SELECT count(*) FROM order_intents), (SELECT count(*) FROM venue_orders)")
        .fetch_one(&pool).await?;
    ensure!(counts == (200, 2000, 1000), "{counts:?}");
    let explain = format!("EXPLAIN (ANALYZE, BUFFERS)\n{NON_TRADING}");
    for (label, account_scope) in [("empty", f.scope.as_str()), ("full", full.as_str())] {
        let plan = non_trading(&explain, &f, account_scope).fetch_all(&pool).await?;
        let plan: Vec<String> = plan.iter().map(|row| row.get(0)).collect();
        for line in &plan {
            println!("NON_TRADING_BOUND {label} {line}");
        }
        let mut max = Duration::ZERO;
        for _ in 0..20 {
            let at = std::time::Instant::now();
            non_trading(NON_TRADING, &f, account_scope).fetch_one(&pool).await?;
            max = max.max(at.elapsed());
        }
        println!("NON_TRADING_BOUND {label} max_ms={:.3}", max.as_secs_f64() * 1e3);
        ensure!(max < Duration::from_secs(1), "{label}: {max:?}");
    }
    let empty = read_non_trading(&pool.connect_options(), &f.scope(), None, deadline()).await;
    ensure!(empty == Ok(0), "{empty:?}");
    let full_scope = f.scope_as(&full, UID);
    let full_read = read_non_trading(&pool.connect_options(), &full_scope, None, deadline()).await;
    ensure!(full_read == Err(PreRefusal::Scope), "unbound scope: {full_read:?}");
    Ok(())
}

/// The gate's pending slot, as the composition binds it (crash-points).
#[cfg(feature = "crash-points")]
mod pending {
    use robson_domain::OrderSide;
    use robson_exec::{
        clock_records::RawClockCapture,
        gate::{
            EpochView, PendingExecutionEvidence, PendingExecutionOwner,
            PendingExecutionRegistration, PendingExecutionTicket,
        },
        venue::{OrderPhase, VenueOrderRecord},
        venue_clock::ClockDomain,
    };
    use rust_decimal::Decimal;

    use super::*;

    const EPOCH: Uuid = Uuid::from_u128(0xe9);

    pub(super) fn bound(venue: &VenueId) -> (Arc<OperationGate>, PendingExecutionOwner) {
        let gate = gate(venue);
        gate.begin_epoch(EpochView { epoch_seq: 7, unclean_reason: None }, None, Vec::new());
        let owner = gate.bind_pending_execution(EPOCH).expect("binds once");
        (gate, owner)
    }

    /// One retained rise of native evidence (the gate's own rules).
    pub(super) fn rise(owner: &PendingExecutionOwner) -> Option<PendingExecutionTicket> {
        let domain = ClockDomain::new(Uuid::from_u128(0xb0), 0, 0).ok()?;
        let at = |second: i64| RawClockCapture {
            epoch: EPOCH,
            wall_seconds: second,
            wall_nanoseconds: 0,
            domain,
            before_ns: i128::from(second) * 1_000_000_000,
            after_ns: i128::from(second) * 1_000_000_000 + 1,
        };
        let executed = Decimal::new(25, 2);
        let evidence = PendingExecutionEvidence {
            sample: Uuid::from_u128(0x5a),
            send: at(100),
            receive: at(101),
            executed,
        };
        let record = VenueOrderRecord {
            exchange_order_id: "1".into(),
            client_order_id: "local".into(),
            orig_client_order_id: None,
            symbol: "BTCUSDT".into(),
            side: OrderSide::Sell,
            order_type: "MARKET".into(),
            protection_kind: None,
            status: "PARTIALLY_FILLED".into(),
            phase: OrderPhase::Live,
            orig_qty: Decimal::ONE,
            executed_qty: executed,
            cumulative_quote_qty: None,
            stop_price: None,
            expiry_reason: None,
            transact_time_ms: None,
            time_ms: None,
            update_time_ms: None,
            working_time_ms: None,
        };
        match owner.register(evidence, Arc::new(record)) {
            PendingExecutionRegistration::Issued(ticket) => Some(ticket),
            _ => None,
        }
    }
}

// 8k-3b (PLAN-r2 §6.2, §6.3, §8): the production attempt and scan over a
// scripted inventory port, on the non-trading statement in PostgreSQL.

/// One scripted triple: a failure after `sent` handoffs, or an answer
/// holding `btc` free whose ACCOUNT receipt is the `n`-th; `bump` moves the
/// accounting version while the triple is in flight.
enum Step {
    Fail(u8, ReadFailure),
    Answer { btc: &'static str, bump: bool },
}

struct Scripted<'a> {
    pool: &'a PgPool,
    f: &'a Fixture,
    script: std::collections::VecDeque<Step>,
    hold: Duration,
    calls: u32,
    handed_off: u32,
    entered: Vec<std::time::Instant>,
    arrived: Vec<std::time::Instant>,
    /// Sessions of the scan's application name seen during each triple:
    /// (connected, busy or in a transaction).
    sessions: Vec<(i64, i64)>,
    receipts: Vec<AccountReceipt>,
    /// What the receipt's revalidation after the post answers.
    valid: bool,
}

impl<'a> Scripted<'a> {
    fn new(pool: &'a PgPool, f: &'a Fixture, script: Vec<Step>) -> Self {
        Self {
            pool,
            f,
            script: script.into(),
            hold: Duration::ZERO,
            calls: 0,
            handed_off: 0,
            entered: Vec::new(),
            arrived: Vec::new(),
            sessions: Vec::new(),
            receipts: Vec::new(),
            valid: true,
        }
    }

    fn receipt(&self, btc: &str) -> Result<AccountReceipt> {
        let n = i64::from(self.calls);
        let domain = robson_exec::venue_clock::ClockDomain::new(Uuid::from_u128(0xb0), 0, 0)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let at = |second: i64, tick: i128| robson_exec::clock_records::RawClockCapture {
            epoch: self.f.epoch,
            wall_seconds: second,
            wall_nanoseconds: 0,
            domain,
            before_ns: tick,
            after_ns: tick + 1,
        };
        let body = serde_json::json!({
            "uid": 4242, "canTrade": true, "accountType": "SPOT", "permissions": ["SPOT"],
            "balances": [
                {"asset": "USDT", "free": "1000", "locked": "0"},
                {"asset": "BTC", "free": btc, "locked": "0"},
            ],
        });
        Ok(AccountReceipt {
            sample: Some((Uuid::from_u128(0x5a), 2)),
            send: at(1_794_614_400 + 10 * n, i128::from(n) * 100),
            receive: at(1_794_614_401 + 10 * n, i128::from(n) * 100 + 50),
            body: serde_json::from_value(body)?,
        })
    }
}

impl InventoryReads for Scripted<'_> {
    async fn triple(&mut self, _deadline: Instant) -> (u8, Result<Triple, ReadFailure>) {
        self.calls += 1;
        self.entered.push(std::time::Instant::now());
        let sessions = sqlx::query_as("SELECT count(*), count(*) FILTER (WHERE state <> 'idle' OR xact_start IS NOT NULL) FROM pg_stat_activity WHERE application_name = '8k3b-scan'")
            .fetch_one(self.pool)
            .await
            .expect("activity");
        self.sessions.push(sessions);
        tokio::time::sleep(self.hold).await;
        let step = self.script.pop_front().expect("a scripted triple");
        let (sent, answer) = match step {
            Step::Fail(sent, failure) => (sent, Err(failure)),
            Step::Answer { btc, bump } => {
                if bump {
                    self.f.bump(self.pool).await.expect("bump");
                }
                let receipt = self.receipt(btc).expect("receipt");
                self.receipts.push(receipt.clone());
                let triple = Triple {
                    account: receipt,
                    orders: Vec::new(),
                    lists: Vec::new(),
                };
                (3, Ok(triple))
            },
        };
        self.handed_off += u32::from(sent);
        self.arrived.push(std::time::Instant::now());
        (sent, answer)
    }

    fn still_valid(&self, _: &AccountReceipt) -> bool {
        self.valid
    }
}

/// The scan's own sessions are recognizable in `pg_stat_activity`.
fn scan_options(options: &PgConnectOptions) -> PgConnectOptions {
    options.clone().application_name("8k3b-scan")
}

/// B3: at most three attempts, whatever the port keeps answering.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_scan_makes_at_most_three_attempts(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let gate = gate(&f.venue);
    let failures = (0..5).map(|_| Step::Fail(1, ReadFailure::Venue)).collect();
    let mut reads = Scripted::new(&pool, &f, failures);
    let s = scan_non_trading(&mut reads, &gate, &pool.connect_options(), &f.scope()).await;
    let s = s.context("one attempt")?;
    ensure!((s.outcome.attempts, reads.calls) == (3, 3), "{s:?}");
    ensure!(s.verdict == BaseVerdict::Unqualified(UnqualifiedReason::Venue) && s.cash.is_none());
    ensure!(
        s.outcome.cost == ScanCost { sql: 3, http: 3, weight: 60 },
        "{:?}",
        s.outcome.cost
    );
    Ok(())
}

/// B2: every attempt's prefix is counted and nothing resets the counters;
/// SQL and connections by proxy, handoffs by the port, equal the model; every
/// connection the scan opened is closed when it returns.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn scan_cost_counts_all_three_attempts_and_cleanup(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let gate = gate(&f.venue);
    let (proxy, options) = proxied(&pool).await?;
    let script = vec![
        Step::Fail(2, ReadFailure::Venue),
        Step::Fail(1, ReadFailure::Clock),
        Step::Answer { btc: "0", bump: false },
    ];
    let mut reads = Scripted::new(&pool, &f, script);
    let s = scan_non_trading(&mut reads, &gate, &options, &f.scope()).await;
    let s = s.context("one attempt")?;
    // 1 + 1 + 2 SQL, 2 + 1 + 3 requests, 26 + 20 + 32 weight.
    ensure!(
        s.outcome.cost == ScanCost { sql: 4, http: 6, weight: 78 },
        "{:?}",
        s.outcome.cost
    );
    let stats = proxy.stats();
    ensure!(
        (stats.statements, stats.connections, reads.handoffs()) == (4, 4, 6),
        "{stats:?}"
    );
    ensure!(matches!(s.verdict, BaseVerdict::Clean { .. }) && s.cash.is_some(), "{s:?}");
    tokio::time::timeout(Duration::from_secs(5), async {
        while proxy.closed_relays() < 4 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("every scan connection closed")?;
    println!("SCAN_8K3B case=cost attempts=3 sql=4 http=6 weight=78 connections_closed=4");
    Ok(())
}

impl Scripted<'_> {
    fn handoffs(&self) -> u64 {
        u64::from(self.handed_off)
    }
}

/// B4: a read refused before any send ends the scan: one pre statement, no
/// request, no weight. The same refusal after a handoff is a venue failure
/// and retries.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_read_refused_before_any_send_ends_the_scan(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let gate = gate(&f.venue);
    let (proxy, options) = proxied(&pool).await?;
    let deferred = (0..3).map(|_| Step::Fail(0, ReadFailure::Deferred)).collect();
    let mut reads = Scripted::new(&pool, &f, deferred);
    let s = scan_non_trading(&mut reads, &gate, &options, &f.scope()).await;
    let s = s.context("one attempt")?;
    ensure!(s.verdict == BaseVerdict::Unqualified(UnqualifiedReason::Deferred), "{s:?}");
    ensure!(
        (s.outcome.attempts, reads.calls, proxy.stats().statements) == (1, 1, 1),
        "{s:?}"
    );
    ensure!(
        s.outcome.cost == ScanCost { sql: 1, http: 0, weight: 0 },
        "{:?}",
        s.outcome.cost
    );
    let sent = vec![Step::Fail(1, ReadFailure::Deferred), Step::Answer {
        btc: "0",
        bump: false,
    }];
    let mut reads = Scripted::new(&pool, &f, sent);
    let s = scan_non_trading(&mut reads, &gate, &options, &f.scope()).await;
    let s = s.context("one attempt")?;
    ensure!(
        s.outcome.attempts == 2 && matches!(s.verdict, BaseVerdict::Clean { .. }),
        "{s:?}"
    );
    Ok(())
}

/// B1: only the last attempt is classified, on its own bodies; an
/// unqualified last attempt concludes nothing, whatever came before.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn an_unqualified_last_attempt_concludes_nothing(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let gate = gate(&f.venue);
    let options = pool.connect_options();
    // Three full triples, each with a ledger write in flight.
    let moved = (0..3).map(|_| Step::Answer { btc: "0", bump: true }).collect();
    let mut reads = Scripted::new(&pool, &f, moved);
    let s = scan_non_trading(&mut reads, &gate, &options, &f.scope()).await;
    let s = s.context("one attempt")?;
    ensure!(
        s.verdict == BaseVerdict::Unqualified(UnqualifiedReason::VersionChanged),
        "{s:?}"
    );
    ensure!(s.cash.is_none() && s.outcome.attempts == 3, "{s:?}");
    // An excess seen by a discarded attempt never reaches the verdict of the
    // qualified one, which concludes on its own receipt.
    let script = vec![Step::Answer { btc: "0.5", bump: true }, Step::Answer {
        btc: "0",
        bump: false,
    }];
    let mut reads = Scripted::new(&pool, &f, script);
    let s = scan_non_trading(&mut reads, &gate, &options, &f.scope()).await;
    let s = s.context("one attempt")?;
    let last = AccountReading::from_receipt(&reads.receipts[1], &f.scope())
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    ensure!(s.verdict == BaseVerdict::Clean { interval: last.interval }, "{s:?}");
    let cash = s.cash.context("the writer's input")?;
    ensure!(
        (cash.version, &cash.verdict) == (f.version(&pool).await?, &s.verdict),
        "{cash:?}"
    );
    Ok(())
}

/// The ACCOUNT receipt's revalidation after the post refuses: a receipt
/// that no longer validates never qualifies, and is retried like any clock
/// refusal.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_receipt_failing_revalidation_never_qualifies(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let gate = gate(&f.venue);
    let answers = (0..3).map(|_| Step::Answer { btc: "0", bump: false }).collect();
    let mut reads = Scripted::new(&pool, &f, answers);
    reads.valid = false;
    let s = scan_non_trading(&mut reads, &gate, &pool.connect_options(), &f.scope()).await;
    let s = s.context("one attempt")?;
    ensure!(s.verdict == BaseVerdict::Unqualified(UnqualifiedReason::Clock), "{s:?}");
    ensure!(s.cash.is_none() && s.outcome.attempts == 3, "{s:?}");
    ensure!(
        s.outcome.cost == ScanCost { sql: 6, http: 9, weight: 96 },
        "{:?}",
        s.outcome.cost
    );
    Ok(())
}

/// B5: the freshness floor is taken before the ACCOUNT request, so the
/// writer's 60 s bound counts from before the request, never from arrival.
/// The ACCOUNT response is held 300 ms here.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn publication_freshness_counts_from_before_the_account_request(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let gate = gate(&f.venue);
    let mut reads = Scripted::new(&pool, &f, vec![Step::Answer { btc: "0", bump: false }]);
    reads.hold = Duration::from_millis(300);
    let s = scan_non_trading(&mut reads, &gate, &pool.connect_options(), &f.scope()).await;
    let cash = s.context("one attempt")?.cash.context("the writer's input")?;
    let (entered, arrived) = (reads.entered[0], reads.arrived[0]);
    ensure!(cash.received <= entered, "the floor precedes the request");
    ensure!(arrived.duration_since(cash.received) >= Duration::from_millis(300));
    ensure!(cash.received.elapsed() > arrived.elapsed(), "age counted from the floor");
    Ok(())
}

/// Nothing is held across the inventory: while each triple is in flight, no
/// session of the scan is busy or inside a transaction; once the scan has
/// returned, none of its sessions remains.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_scan_holds_nothing_across_the_inventory(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let gate = gate(&f.venue);
    let options = scan_options(&pool.connect_options());
    let script = vec![Step::Answer { btc: "0", bump: true }, Step::Answer {
        btc: "0",
        bump: false,
    }];
    let mut reads = Scripted::new(&pool, &f, script);
    reads.hold = Duration::from_millis(200);
    let s = scan_non_trading(&mut reads, &gate, &options, &f.scope()).await;
    ensure!(s.context("one attempt")?.outcome.attempts == 2);
    ensure!(reads.sessions.iter().all(|(_, busy)| *busy == 0), "{:?}", reads.sessions);
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let open: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE application_name = '8k3b-scan'",
            )
            .fetch_one(&pool)
            .await?;
            if open == 0 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    closed.await.context("every scan session closed")??;
    println!("SCAN_8K3B case=holds-nothing sessions_during_triples={:?}", reads.sessions);
    Ok(())
}

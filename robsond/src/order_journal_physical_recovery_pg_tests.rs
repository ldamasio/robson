#![cfg(test)]

//! Test-only real Journal/Spot audit composition driven by a disposable PG16
//! controller.
use std::{io, os::fd::AsFd, sync::Arc};

use robson_domain::OrderSide;
use robson_exec::{
    order_journal::{IntentSpec, OrderJournal, ReleaseRefusal},
    venue::{OrderPhase, VenueOrderRecord},
    venue_ledger::{OrderLink, OrderOwner, SeenAt},
};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::unix::pipe::{Receiver, Sender},
    time::{timeout_at, Instant},
};

use super::*;
use crate::{
    spot_order_set_store::{
        cold_install::{ClosedWriterFence, FencePhase, FenceReceipt, FixtureFenceError},
        cold_owner::RestoreIdentity,
        observe_generation,
        physical_fixture::{
            audit_cold_sources, AttemptSource, ColdAuditRefusal, ColdAuditScope, ColdSourceSink,
            Family, IntentSource, OrderSource, PageBudget, RouteClass, RouteSource, SinkFull,
            SourceKey,
        },
        spot_startup::{record_epoch, Admission, RestoredTarget, SpotSupervisor},
        ExpectedGeneration,
    },
    venue_budget::{fixture as budget_fixture, physical_recovery as budget_recovery},
    venue_ledger_store::PgVenueLedger,
};

const SYMBOL: &str = "BTCUSDT";
const BUY_ID: &str = "417011";
const BUY_CLIENT: &str = "physical-recovery-observed-buy";
const OWNED_ID: &str = "417013";
const OWNED_CLIENT: &str = "physical-recovery-owned-buy";
const BUDGET: PageBudget = PageBudget {
    intents: 1,
    attempts: 1,
    orders: 1,
    routes: 2,
};

const PIPE_BOUND: Duration = Duration::from_secs(3);

struct Channel {
    input: BufReader<Receiver>,
    output: Sender,
    restored: Option<RestoredTarget>,
    writer_options: Option<sqlx::postgres::PgConnectOptions>,
    successor_login: String,
    substitute_installer: Option<PgPool>,
    operation: Uuid,
    owner: String,
    fixture: String,
    previous: String,
    seq: u64,
}

impl Channel {
    async fn open() -> Result<(Self, Value)> {
        let mut input =
            BufReader::new(Receiver::from_owned_fd(io::stdin().as_fd().try_clone_to_owned()?)?);
        let output = Sender::from_owned_fd(io::stdout().as_fd().try_clone_to_owned()?)?;
        let greeting = Self::read(&mut input, Instant::now() + PIPE_BOUND).await?;
        ensure!(greeting["schema"] == 1);
        let revision = Self {
            input,
            output,
            restored: None,
            writer_options: None,
            successor_login: Self::string(&greeting, "new_user")?.to_owned(),
            substitute_installer: None,
            operation: Self::string(&greeting, "operation")?.parse()?,
            owner: Self::string(&greeting, "owner")?.to_owned(),
            fixture: Self::string(&greeting, "fixture")?.to_owned(),
            previous: Self::string(&greeting, "previous")?.to_owned(),
            seq: 0,
        };
        ensure!(!revision.operation.is_nil() && !revision.owner.is_empty());
        ensure!(
            revision.fixture.len() == 64 && revision.fixture.bytes().all(|b| b.is_ascii_hexdigit())
        );
        Ok((revision, greeting))
    }

    fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
        value[key].as_str().with_context(|| format!("missing controller field {key}"))
    }

    async fn read(input: &mut BufReader<Receiver>, deadline: Instant) -> Result<Value> {
        let mut bytes = Vec::new();
        let n = timeout_at(deadline, input.take(16_385).read_until(b'\n', &mut bytes)).await??;
        ensure!(
            n > 0 && n <= 16_384 && bytes.ends_with(b"\n"),
            "invalid private controller frame size"
        );
        serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("malformed private controller frame"))
    }

    async fn phase(&mut self, phase: &str, facts: Value) -> Result<Value> {
        let bound = if phase == "retired" {
            Duration::from_secs(8)
        } else {
            PIPE_BOUND
        };
        self.phase_at(phase, facts, Instant::now() + bound).await
    }

    async fn phase_at(&mut self, phase: &str, facts: Value, deadline: Instant) -> Result<Value> {
        self.seq += 1;
        let previous = if phase == "retired"
            && std::env::var("ROBSON_PHYSICAL_FAULT").as_deref() == Ok("bad-frame")
        {
            "unmatched-previous-frame"
        } else {
            &self.previous
        };
        let frame = json!({"operation": self.operation, "owner": self.owner,
            "fixture": self.fixture, "seq": self.seq, "phase": phase,
            "previous": previous, "facts": facts});
        let payload = frame.to_string();
        let expected_ack = format!("{:x}", Sha256::digest(payload.as_bytes()));
        let bytes = format!("\nROBSON_PHASE {payload}\n");
        ensure!(bytes.len() <= 16_384, "oversized private controller frame");
        timeout_at(deadline, self.output.write_all(bytes.as_bytes())).await??;
        let received = Self::read(&mut self.input, deadline).await?;
        ensure!(
            received["schema"] == 1
                && received["ok"] == true
                && received["phase"] == phase
                && received["seq"] == self.seq
                && received["operation"] == self.operation.to_string()
                && received["owner"] == self.owner
                && received["fixture"] == self.fixture,
            "foreign, stale, or wrong-phase controller reply"
        );
        let previous = Self::string(&received, "previous")?;
        ensure!(
            previous == expected_ack && previous != self.previous,
            "invalid controller receipt chain"
        );
        self.previous = previous.to_owned();
        Ok(received)
    }
}

fn validated_role(role: &str) -> Result<&str> {
    ensure!(
        !role.is_empty()
            && role.len() < 64
            && role.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    );
    Ok(role)
}

async fn validate_writer_roles(pool: &PgPool, old: &str, new: &str) -> Result<()> {
    let old = validated_role(old)?;
    let new = validated_role(new)?;
    ensure!(old != new);
    let both: bool = sqlx::query_scalar(
        "SELECT count(*)=2 AND bool_and(rolcanlogin AND NOT rolsuper AND NOT rolcreaterole
             AND NOT rolcreatedb AND NOT rolreplication AND NOT rolbypassrls AND NOT rolinherit)
         FROM pg_roles WHERE rolname IN ($1,$2)",
    )
    .bind(old)
    .bind(new)
    .fetch_one(pool)
    .await?;
    ensure!(both);
    Ok(())
}

/// Rehearsal only: one authored entry J through the ordinary writer, which
/// writes its admission in the same transaction, then its native order under
/// the existing route 1. Synthetic fixture success, no attempt or clock.
async fn seed_owned_entry(pool: &PgPool, g: &JournalGeneration) -> Result<Uuid> {
    use robson_exec::{
        order_journal::{AdmissionRow, OutcomeRow, VenueOutcome},
        venue_ledger::LegOrderKind,
    };
    let position = Uuid::from_u128(1);
    let mut batch = sell();
    batch.intent.kind = IntentKind::Entry;
    batch.intent.client_order_id = OWNED_CLIENT.into();
    batch.intent.position_id = Some(position);
    batch.intent.account_scope = g.account_scope.clone();
    batch.outcome = Some(OutcomeRow {
        venue_outcome: Some(VenueOutcome::Success),
        venue_evidence: Some(json!({"fixture": true, "symbol": SYMBOL,
            "orderId": OWNED_ID, "clientOrderId": OWNED_CLIENT})),
        ..OutcomeRow::default()
    });
    batch.balance.as_mut().context("balance batch")?.admission = Some(Arc::new(AdmissionRow {
        position_id: position,
        symbol: SYMBOL.to_owned(),
        decided_month: chrono::NaiveDate::from_ymd_opt(2026, 11, 1).context("month")?,
        taker_bps: Decimal::new(10, 0),
        gap_bps: Decimal::new(10, 0),
        notional_margin_bps: Decimal::new(2, 0),
        min_notional: Decimal::new(10, 0),
        lot_step: Decimal::new(1, 3),
        fee_asset_policy: "base".into(),
        envelope_source: "isolated-entry-terms".into(),
        envelope_as_of: Utc::now(),
        envelope_hash: "fixture".into(),
        planned_qty: Decimal::ONE,
        requested_qty: Decimal::ONE,
        reference_price: Decimal::new(100, 0),
        remainder_reserve: Decimal::new(1, 1),
        admission_charge: Decimal::new(10, 0),
        initial_trigger: Decimal::new(90, 0),
    }));
    let start = fields(pool, g).await?.0;
    insist(PgJournalBackend::new(pool, g.tenant_id).commit(&batch).await)?;
    ensure!(fields(pool, g).await?.0 == start + 1, "ordinary J commit bumps once");
    let owned = VenueOrderRecord {
        exchange_order_id: OWNED_ID.into(),
        client_order_id: OWNED_CLIENT.into(),
        orig_client_order_id: None,
        symbol: SYMBOL.into(),
        side: OrderSide::Buy,
        order_type: "MARKET".into(),
        protection_kind: None,
        status: "NEW".into(),
        phase: OrderPhase::Live,
        orig_qty: Decimal::ONE,
        executed_qty: Decimal::ZERO,
        cumulative_quote_qty: None,
        stop_price: None,
        expiry_reason: None,
        transact_time_ms: None,
        time_ms: None,
        update_time_ms: None,
        working_time_ms: None,
    };
    let link = OrderLink {
        venue: VenueId::new(VenueId::BINANCE_SPOT),
        account_scope: g.account_scope.clone(),
        owner: OrderOwner::Leg {
            intent_id: batch.intent.intent_id,
            position_id: position,
            kind: LegOrderKind::Entry,
        },
        client_order_id: OWNED_CLIENT.into(),
    };
    let written = PgVenueLedger::new(pool, g.tenant_id)
        .record_order(&owned, &link, SeenAt { at: Utc::now(), clock: None, capture: None })
        .await?;
    ensure!(written.changed && fields(pool, g).await?.0 == start + 2);
    Ok(batch.intent.intent_id)
}

async fn seed_native_sources(
    pool: &PgPool,
    g: &JournalGeneration,
    uid: &str,
    rehearsal: bool,
) -> Result<Option<Uuid>> {
    let venue = VenueId::new(VenueId::BINANCE_SPOT);
    sqlx::query(
        "INSERT INTO public.venue_accounts(venue_id,account_scope,tenant_id,uid,bound_at,cutover_id)
         VALUES('binance:spot',$1,$2,$3,now(),$4)",
    ).bind(&g.account_scope).bind(g.tenant_id).bind(uid).bind(Uuid::now_v7()).execute(pool).await?;
    sqlx::query("CREATE INDEX idx_leg_routes_spot_scope_position ON public.leg_routes(account_scope,position_id) WHERE venue_id='binance:spot'")
        .execute(pool).await?;
    let ledger = PgVenueLedger::new(pool, g.tenant_id);
    for position in (1..=8).map(Uuid::from_u128) {
        ledger.insert_route(position, &venue, &g.account_scope, Utc::now()).await?;
    }
    let buy = VenueOrderRecord {
        exchange_order_id: BUY_ID.into(),
        client_order_id: BUY_CLIENT.into(),
        orig_client_order_id: None,
        symbol: SYMBOL.into(),
        side: OrderSide::Buy,
        order_type: "LIMIT".into(),
        protection_kind: None,
        status: "NEW".into(),
        phase: OrderPhase::Live,
        orig_qty: Decimal::ONE,
        executed_qty: Decimal::ZERO,
        cumulative_quote_qty: None,
        stop_price: None,
        expiry_reason: None,
        transact_time_ms: None,
        time_ms: None,
        update_time_ms: None,
        working_time_ms: None,
    };
    let link = OrderLink {
        venue,
        account_scope: g.account_scope.clone(),
        owner: OrderOwner::Discovered,
        client_order_id: BUY_CLIENT.into(),
    };
    ensure!(
        ledger
            .record_order(&buy, &link, SeenAt {
                at: Utc::now(),
                clock: None,
                capture: None
            })
            .await?
            .changed
    );
    let owned = if rehearsal {
        Some(seed_owned_entry(pool, g).await?)
    } else {
        None
    };
    let seeded = sqlx::query(
        "UPDATE public.spot_order_set_generation g SET accounting_version=a.version
         FROM public.accounting_state a
         WHERE (g.tenant_id,g.venue_id,g.account_scope,g.generation_id)=($1,'binance:spot',$2,$3)
           AND (a.tenant_id,a.venue_id)=(g.tenant_id,g.venue_id)",
    )
    .bind(g.tenant_id)
    .bind(&g.account_scope)
    .bind(g.generation_id)
    .execute(pool)
    .await?;
    ensure!(seeded.rows_affected() == 1);
    Ok(owned)
}

/// `owned` admits the one rehearsal J/order pair at route 1.
#[derive(Default)]
struct NativeKeys {
    owned: bool,
    intents: Vec<(Uuid, String, Option<Uuid>)>,
    attempts: usize,
    orders: Vec<(String, String, String)>,
    routes: Vec<Uuid>,
    legs: usize,
}
impl NativeKeys {
    fn cap(&self) -> usize {
        1 + usize::from(self.owned)
    }
}
impl ColdSourceSink for NativeKeys {
    fn intents(&mut self, rows: &[IntentSource]) -> std::result::Result<(), SinkFull> {
        if self.intents.len() + rows.len() > self.cap() {
            return Err(SinkFull);
        }
        self.intents
            .extend(rows.iter().map(|j| (j.intent_id, j.client_order_id.clone(), j.position_id)));
        Ok(())
    }
    fn attempts(&mut self, rows: &[AttemptSource]) -> std::result::Result<(), SinkFull> {
        self.attempts += rows.len();
        if self.attempts > 0 {
            return Err(SinkFull);
        }
        Ok(())
    }
    fn orders(&mut self, rows: &[OrderSource]) -> std::result::Result<(), SinkFull> {
        if self.orders.len() + rows.len() > self.cap() {
            return Err(SinkFull);
        }
        self.orders.extend(
            rows.iter()
                .map(|o| (o.symbol.clone(), o.exchange_order_id.clone(), o.side.clone())),
        );
        Ok(())
    }
    fn routes(&mut self, rows: &[RouteSource]) -> std::result::Result<(), SinkFull> {
        let owned = self.owned;
        let leg = move |r: &RouteSource| {
            owned
                && r.position_id == Uuid::from_u128(1)
                && r.classify() == RouteClass::Leg { closed: false }
        };
        if self.routes.len() + rows.len() > 8
            || rows.iter().any(|r| !leg(r) && r.classify() != RouteClass::ArmedOnly)
        {
            return Err(SinkFull);
        }
        self.legs += rows.iter().filter(|r| leg(r)).count();
        self.routes.extend(rows.iter().map(|r| r.position_id));
        Ok(())
    }
}

fn key_value(key: Option<&SourceKey>) -> Value {
    match key {
        None => Value::Null,
        Some(SourceKey::ClientOrder(id)) => json!(id),
        Some(SourceKey::Order { symbol, exchange_order_id }) => json!([symbol, exchange_order_id]),
        Some(SourceKey::Route(id)) => json!(id),
    }
}

impl ClosedWriterFence for Channel {
    async fn check(
        &mut self,
        restore: RestoreIdentity,
        phase: FencePhase,
        deadline: Instant,
    ) -> std::result::Result<FenceReceipt, FixtureFenceError> {
        if restore.operation != self.operation || Instant::now() >= deadline {
            return Err(FixtureFenceError);
        }
        let name = match phase {
            FencePhase::BeforeEpoch => "fence_before_epoch",
            FencePhase::BeforeCapture => "fence_before_capture",
            FencePhase::BeforeRead => "fence_before_install",
            FencePhase::BeforePublish => "fence_before_publish",
            FencePhase::AfterReadback => "fence_after_readback",
            FencePhase::BeforeSourceAudit | FencePhase::AfterSourceAudit => {
                return Err(FixtureFenceError);
            },
        };
        let approved = self
            .phase_at(
                name,
                json!({"target_id": restore.target}),
                deadline.min(Instant::now() + PIPE_BOUND),
            )
            .await
            .map_err(|_| FixtureFenceError)?;
        if approved["target_id"] != restore.target.to_string() || Instant::now() >= deadline {
            return Err(FixtureFenceError);
        }
        let owner = self.owner.parse().map_err(|_| FixtureFenceError)?;
        Ok(FenceReceipt { restore, owner })
    }
}

impl SpotSupervisor for Channel {
    fn target(&self) -> &RestoredTarget {
        self.restored.as_ref().expect("restore receipt configured")
    }

    fn successor_login(&self) -> &str {
        &self.successor_login
    }

    async fn admit(
        &mut self,
        admission: &Admission<'_>,
        deadline: Instant,
    ) -> std::result::Result<PgPool, FixtureFenceError> {
        let g = admission.generation;
        let permit = self.phase_at(
            "admit",
            json!({"epoch":g.process_epoch,"audit":g.source_audit_id,"generation":g.generation_id}),
            deadline.min(Instant::now() + PIPE_BOUND),
        ).await.map_err(|_| FixtureFenceError)?;
        if permit["new_user"].as_str() != Some(admission.login) {
            return Err(FixtureFenceError);
        }
        if let Some(installer) = self.substitute_installer.take() {
            return Ok(installer);
        }
        let (login, password_key) = if std::env::var("ROBSON_PHYSICAL_FAULT").as_deref()
            == Ok("startup-wrong-restricted-login")
        {
            (
                Self::string(&permit, "probe_user").map_err(|_| FixtureFenceError)?,
                "probe_password",
            )
        } else {
            (admission.login, "new_password")
        };
        let password = Self::string(&permit, password_key).map_err(|_| FixtureFenceError)?;
        let options = self
            .writer_options
            .take()
            .ok_or(FixtureFenceError)?
            .username(login)
            .password(password)
            .ssl_mode(PgSslMode::Disable);
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .test_before_acquire(false)
            .connect_with(options)
            .await
            .map_err(|_| FixtureFenceError)?;
        if std::env::var("ROBSON_PHYSICAL_FAULT").as_deref() == Ok("post-admit-eof") {
            std::process::exit(68);
        }
        Ok(pool)
    }
}

fn binding<'a>(g: &'a JournalGeneration, venue: &'a VenueId) -> ExpectedGeneration<'a> {
    ExpectedGeneration {
        tenant_id: g.tenant_id,
        venue,
        account_scope: &g.account_scope,
        generation_id: g.generation_id,
        process_epoch: g.process_epoch,
        source_audit_id: g.source_audit_id,
    }
}

async fn witness_daemon_start(
    restore: PgPool,
    ready: crate::spot_order_set_store::spot_startup::SpotReady,
    tenant: Uuid,
    epoch: Uuid,
) -> Result<Arc<OrderJournal>> {
    use robson_connectors::{binance::spot::BinanceSpotAdapter, binance_rest::BinanceRestClient};
    use robson_exec::{StubOhlcv, StubTradeFeed};

    #[cfg(feature = "crash-points")]
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    #[cfg(feature = "crash-points")]
    let base = format!("http://{}", listener.local_addr()?);
    let venue = || -> Result<_> {
        #[cfg(feature = "crash-points")]
        let client = BinanceRestClient::with_harness_base_url(
            base.clone(),
            "fixture-key".into(),
            "fixture-secret".into(),
        );
        #[cfg(not(feature = "crash-points"))]
        let client = BinanceRestClient::new("fixture-key".into(), "fixture-secret".into());
        Ok(Arc::new(BinanceSpotAdapter::new(Arc::new(client))?))
    };
    let mut config = crate::Config::test();
    config.projection.tenant_id = Some(tenant);
    let unqualified = crate::Daemon::with_venue_and_recovery(
        config.clone(),
        venue()?,
        Arc::new(StubTradeFeed::default()),
        Arc::new(StubOhlcv::default()),
        None,
        Some(Arc::new(restore.clone())),
    );
    let refused = Box::pin(unqualified.run_until(crate::stop::StopRequest::default())).await;
    ensure!(matches!(refused, Err(crate::DaemonError::Config(reason))
        if reason == "Spot startup requires a supervised journal"));
    let daemon = crate::Daemon::with_supervised_spot(
        config,
        venue()?,
        Arc::new(StubTradeFeed::default()),
        Arc::new(StubOhlcv::default()),
        ready,
    )?;
    let journal = daemon.journal_for_startup_witness();
    let refused = Box::pin(daemon.run_until(crate::stop::StopRequest::default())).await;
    ensure!(
        matches!(refused, Err(crate::DaemonError::Config(reason))
        if reason == "Spot TIME prerequisites unavailable: PrerequisiteRefusal"),
        "supervised startup did not reach the existing C2 gate"
    );
    let epochs: Vec<Uuid> = sqlx::query_scalar(
        "SELECT epoch_id FROM public.process_epochs WHERE tenant_id=$1 ORDER BY epoch_seq DESC LIMIT 3",
    ).bind(tenant).fetch_all(&restore).await?;
    ensure!(
        epochs.len() == 2 && epochs[0] == epoch,
        "daemon recorded an extra process epoch"
    );
    #[cfg(feature = "crash-points")]
    ensure!(
        tokio::time::timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err(),
        "Spot attempted wire I/O before C2",
    );
    Ok(journal)
}

#[sqlx::test(migrations = false)]
#[ignore = "requires dedicated physical controller and isolated PG16 over loopback"]
async fn physical_spot_writer_survives_exact_native_restore(pool: PgPool) -> Result<()> {
    physical_spot_restore(pool, false).await
}

#[cfg(feature = "crash-points")]
#[sqlx::test(migrations = false)]
#[ignore = "requires dedicated physical PG16 no-capital controller"]
async fn physical_spot_no_capital_rehearsal(pool: PgPool) -> Result<()> {
    physical_spot_restore(pool, true).await
}

async fn physical_spot_restore(pool: PgPool, rehearsal: bool) -> Result<()> {
    #[cfg(not(feature = "crash-points"))]
    ensure!(!rehearsal, "rehearsal compiler gate");
    let uid = "physical-recovery-fixture".to_owned();
    #[cfg(feature = "crash-points")]
    let uid = if rehearsal {
        robson_connectors::inventory_loopback::LOOPBACK_UID.to_string()
    } else {
        uid
    };
    let (mut channel, greeting) = Channel::open().await?;
    let old_role = Channel::string(&greeting, "old_user")?;
    let new_role = Channel::string(&greeting, "new_user")?;
    let upstream = pool.connect_options();
    let server = tokio::net::lookup_host((upstream.get_host(), upstream.get_port()))
        .await?
        .next()
        .context("isolated local server")?;
    ensure!(server.ip().is_loopback());
    let db = upstream.get_database().context("SQLx isolated database name")?;
    ensure!(db.starts_with("_sqlx_test"));
    robson_db::migrate(&pool).await?;
    let clock = NativeClock::initialize()?;
    let copied_budget = budget_fixture::prepare(
        &pool,
        &clock,
        Instant::now() + Duration::from_secs(3),
        &Trace::default(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("initialize disposable budget: {e:?}"))?;
    let g = seed_generation_for(&pool, robson_exec::order_journal::DEFAULT_ACCOUNT_SCOPE).await?;
    let owned = seed_native_sources(&pool, &g, &uid, rehearsal).await?;
    #[cfg(feature = "crash-points")]
    if rehearsal {
        crate::venue_budget::n5_claim::rehearsal_c2::seed_manifest(&pool, SYMBOL).await?;
    }
    validate_writer_roles(&pool, old_role, new_role).await?;
    let before = fields(&pool, &g).await?;
    ensure!(
        before.0 == before.1 && before.0 == if rehearsal { 3 } else { 1 },
        "pre-fence setup versions"
    );
    let proxy = DbProxy::start(server).await?;
    let old_options = upstream
        .as_ref()
        .clone()
        .host("127.0.0.1")
        .port(proxy.addr().port())
        .ssl_mode(PgSslMode::Disable)
        .username(old_role)
        .password(Channel::string(&greeting, "old_password")?);
    let old_pool = PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with(old_options)
        .await
        .map_err(|_| anyhow::anyhow!("old fixture connection refused"))?;
    let (pid,birth,user): (i32,String,String) = sqlx::query_as(
        "SELECT pid,backend_start::text,current_user::text FROM pg_stat_activity WHERE pid=pg_backend_pid()",
    ).fetch_one(&old_pool).await?;
    ensure!(user == old_role);
    channel
        .phase_at(
            "old_connected",
            json!({"database":db,"pid":pid,"backend_start":birth,
        "current_user":user,"tenant":g.tenant_id}),
            Instant::now() + if rehearsal { Duration::from_secs(10) } else { PIPE_BOUND },
        )
        .await?;
    if std::env::var("ROBSON_PHYSICAL_FAULT").as_deref() == Ok("eof") {
        std::process::exit(67);
    }
    // A fresh PgJournalBackend::new has its own lazy pool. It would relogin
    // after HBA closes and would not exercise the still-authenticated backend.
    let writer = PgJournalBackend {
        pool: old_pool.clone(),
        daemon_pool: pool.clone(),
        tenant_id: g.tenant_id,
        ready_wait: JOURNAL_READY_WAIT,
        balance_account: false,
        order_set_projection: Some(g.clone()),
    };
    proxy.enable_exact_query_trace(12);
    proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE public.spot_order_set_generation g"));
    let mut original = sell();
    original.intent.account_scope = g.account_scope.clone();
    let result = writer.commit(&original).await;
    ensure!(
        matches!(result, Err(CommitFailure::Uncertain(_))),
        "the still-open original did not reach uncertain COMMIT"
    );
    ensure!(
        proxy.stats().commit_acks_dropped == 1
            && proxy.query_trace().len() == 6
            && proxy.query_trace_dropped() == 0
    );
    native_retired(&pool, pid, &birth).await?;
    let committed = fields(&pool, &g).await?;
    ensure!(
        committed == (before.0 + 1, before.1 + 1, 1, 0, 1, 0)
            && j_present(&pool, original.intent.intent_id).await?
    );
    old_pool.close().await;
    drop(writer);
    drop(proxy);
    pool.close().await;
    let proof = channel.phase("retired",json!({"pid":pid,"backend_start":birth,"intent":original.intent.intent_id,
        "epoch":g.process_epoch,"generation":g.generation_id,"audit":g.source_audit_id,"version":committed.0,
        "native_statements":6,"lost_commit_acks":1})).await?;
    let restore_identity = RestoreIdentity {
        operation: channel.operation,
        target: Channel::string(&proof, "target_id")?.parse()?,
    };
    channel.restored = Some(RestoredTarget {
        restore: restore_identity,
        database: db.to_owned(),
        system_identifier: Channel::string(&proof, "system_id")?.to_owned(),
        data_directory: Channel::string(&proof, "target")?.to_owned(),
        postmaster_started: Channel::string(&proof, "postmaster_started")?.to_owned(),
    });
    let restored_wire = DbProxy::start(server).await?;
    let restore_options = upstream
        .as_ref()
        .clone()
        .host("127.0.0.1")
        .port(restored_wire.addr().port())
        .ssl_mode(PgSslMode::Disable);
    channel.writer_options = Some(restore_options.clone());
    let restore = PgPoolOptions::new()
        .max_connections(4)
        .test_before_acquire(false)
        .connect_with(restore_options)
        .await?;
    if std::env::var("ROBSON_PHYSICAL_FAULT").as_deref() == Ok("startup-wrong-live-role") {
        channel.substitute_installer = Some(restore.clone());
    }
    let venue = VenueId::new(VenueId::BINANCE_SPOT);
    let mut reader = restore.connect_options().connect().await?;
    let predecessor = observe_generation(&mut reader, binding(&g, &venue))
        .await?
        .context("copied structural generation absent")?;
    ensure!(
        predecessor.accounting_version == committed.0 && fields(&restore, &g).await? == committed
    );
    let handshake_start = Instant::now();
    restored_wire.enable_exact_query_trace(256);
    let deadline = handshake_start + Duration::from_secs(20);
    let mut epoched = record_epoch(
        restore.clone(),
        channel,
        g.tenant_id,
        &g.account_scope,
        "physical-writer-fixture",
        deadline,
    )
    .await
    .map_err(|e| anyhow::anyhow!("supervised Spot epoch: {e:?}"))?;
    let first = restored_wire.query_trace();
    let installer_checks =
        first.iter().filter(|q| q.contains("FROM pg_control_system() c")).count();
    ensure!(installer_checks == 1, "installer identity query bound");
    let new = epoched.epoch().clone();
    let stamp = new.unclean_generation.context("missing native original recovery stamp")?;
    let actual: Option<i64> = sqlx::query_scalar(
        "SELECT uncertain_generation FROM public.order_intents WHERE intent_id=$1",
    )
    .bind(original.intent.intent_id)
    .fetch_one(&restore)
    .await?;
    ensure!(
        actual == Some(stamp)
            && new.stamped_intents == 1
            && new.predecessor_epoch_id == Some(g.process_epoch)
            && observe_generation(&mut reader, binding(&g, &venue)).await?.is_none()
            && fields(&restore, &g).await? == committed
    );
    epoched
        .supervisor()
        .phase(
            "epoch",
            json!({"predecessor":g.process_epoch,"epoch":new.epoch.epoch_id,"stamp":stamp}),
        )
        .await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let scope = ColdAuditScope {
        tenant_id: g.tenant_id,
        account_scope: &g.account_scope,
    };
    let stopped = audit_cold_sources(
        &restore,
        scope,
        PageBudget { routes: 1, ..BUDGET },
        deadline,
        &mut NativeKeys { owned: rehearsal, ..NativeKeys::default() },
    )
    .await;
    ensure!(
        matches!(stopped,Err(ColdAuditRefusal::PageBudget {family:Family::Routes,last_key:Some(SourceKey::Route(key))}) if key==Uuid::from_u128(8))
    );
    let mut captured = epoched
        .capture(&uid, committed.0)
        .await
        .map_err(|e| anyhow::anyhow!("supervised Spot capture: {e:?}"))?;
    let cut = captured.cut();
    let mut keys = NativeKeys { owned: rehearsal, ..NativeKeys::default() };
    keys.intents(cut.intents())
        .map_err(|_| anyhow::anyhow!("original J fixture oracle"))?;
    keys.attempts(cut.attempts())
        .map_err(|_| anyhow::anyhow!("attempt fixture oracle"))?;
    keys.orders(cut.orders())
        .map_err(|_| anyhow::anyhow!("native order fixture oracle"))?;
    keys.routes(cut.routes()).map_err(|_| anyhow::anyhow!("route fixture oracle"))?;
    let receipt = &cut.receipt;
    ensure!(
        receipt.tenant_id == g.tenant_id
            && receipt.account_scope == g.account_scope
            && receipt.statements == 11
    );
    let mut intents = vec![(
        original.intent.intent_id,
        original.intent.client_order_id.clone(),
        original.intent.position_id,
    )];
    let mut orders: Vec<(String, String, String)> =
        vec![(SYMBOL.into(), BUY_ID.into(), "BUY".into())];
    if let Some(owned) = owned {
        intents.push((owned, OWNED_CLIENT.into(), Some(Uuid::from_u128(1))));
        orders.push((SYMBOL.into(), OWNED_ID.into(), "BUY".into()));
    }
    // Source order is by key, so compare as sets of exact identities.
    keys.intents.sort();
    intents.sort();
    keys.orders.sort();
    orders.sort();
    ensure!(
        keys.intents == intents
            && keys.attempts == 0
            && keys.orders == orders
            && keys.routes == (1..=8).map(Uuid::from_u128).collect::<Vec<_>>()
            && keys.legs == usize::from(rehearsal)
    );
    let families=receipt.families.iter().zip(["intents","attempts","orders","routes"])
        .map(|(f,name)| json!({"family":name,"rows":f.rows,"pages":f.pages,"last_key":key_value(f.last_key.as_ref())}))
        .collect::<Vec<_>>();
    let statements = receipt.statements;
    let cold_facts = json!({"tenant":g.tenant_id,"scope":g.account_scope,"epoch":new.epoch.epoch_id,
        "version":committed.0,"families":families,"statements":statements});
    if rehearsal {
        captured.supervisor().phase_at("cold", cold_facts, Instant::now() + Duration::from_secs(12)).await?;
    } else {
        captured.supervisor().phase("cold", cold_facts).await?;
    }
    #[cfg(feature = "crash-points")]
    if rehearsal {
        crate::venue_budget::n5_claim::rehearsal_c2::cut_legacy_reason(&restore).await?;
        captured.supervisor().phase("catalogue_cut", json!({"target_id": restore_identity.target})).await?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let audit = budget_recovery::audit_empty_budget(&restore, &copied_budget, deadline).await?;
    captured.supervisor().phase("budget",json!({"audit":audit.audit,"quiescence_generation":audit.generation,
        "source":0,"clocks":0,"charges":0,"statements":audit.start_commands+audit.pass_commands+audit.terminal_commands})).await?;
    #[cfg(feature = "crash-points")]
    if rehearsal {
        crate::venue_budget::n5_claim::rehearsal_c2::load_fixture_registry(
            &restore, g.tenant_id, &g.account_scope, uid.parse()?, SYMBOL, &clock, deadline,
        ).await?;
    }
    let (bootstrap, rows) = budget_recovery::bootstrap_empty_budget(
        &restore,
        g.tenant_id,
        &new,
        &copied_budget,
        &audit,
        &clock,
        deadline,
    )
    .await?;
    captured.supervisor().phase("bootstrap",json!({"audit":bootstrap.audit,"quiescence_generation":bootstrap.generation,
        "epoch":bootstrap.process_epoch,"statements":bootstrap.bootstrap_commands,"stale_statements":bootstrap.stale_commands})).await?;
    let mut installed = captured
        .install()
        .await
        .map_err(|e| anyhow::anyhow!("supervised Spot installation: {e:?}"))?;
    let successor = installed.generation().clone();
    ensure!(observe_generation(&mut reader, binding(&successor, &venue)).await?.is_some());
    ensure!(observe_generation(&mut reader, binding(&g, &venue)).await?.is_none());
    installed.supervisor().phase("installed",json!({"tenant":g.tenant_id,"scope":g.account_scope,"epoch":successor.process_epoch,
        "version":committed.0,"audit":successor.source_audit_id,"generation":successor.generation_id})).await?;
    #[cfg(feature = "crash-points")]
    if rehearsal {
        installed.supervisor().phase("rehearsal_begin", json!({
            "epoch": successor.process_epoch, "generation": successor.generation_id,
        })).await?;
        let owned = owned.context("rehearsal owned J")?;
        let witness = crate::venue_clock_owner::run_held_inventory_rehearsal(
            &restore, &restored_wire, &new, g.tenant_id, std::sync::Arc::new(clock), rows,
            &successor, &uid, committed.0, SYMBOL, owned,
        ).await?;
        ensure!(witness.intent == owned && witness.generation == successor.generation_id
            && witness.lookup_send.epoch == successor.process_epoch
            && witness.lookup_receive.epoch == successor.process_epoch
            && witness.lookup_send.after_ns < witness.lookup_receive.before_ns,
            "pending observation retained across close under its exact binding");
        drop(reader);
        restore.close().await;
        installed.supervisor().phase("rehearsal_complete", json!({
            "epoch": successor.process_epoch, "generation": successor.generation_id,
            "sample": witness.sample, "handoffs": 4, "weight": 107, "inventory_sql": 0,
            "uid": uid.parse::<u64>()?, "owned_intent": owned,
            "lookup_handoffs": witness.lookup_handoffs, "lookup_weight": 4, "lookup_sql": witness.lookup_sql,
            "cumulative_handoffs": witness.cumulative_handoffs, "cumulative_weight": 111,
            "lookup_sample": witness.lookup_sample, "durable_executed": witness.durable_executed.normalize().to_string(),
            "lookup_executed": witness.lookup_executed.normalize().to_string(), "pending_execution_observed": true,
        })).await?;
        return Ok(());
    }
    drop(rows);
    let before_admit = restored_wire.query_trace().len();
    let (ready, mut channel) = installed
        .admit()
        .await
        .map_err(|e| anyhow::anyhow!("admitted Spot writer: {e:?}"))?;
    let wire = restored_wire.query_trace();
    let admitted_checks = &wire[before_admit..];
    ensure!(restored_wire.query_trace_dropped() == 0 && admitted_checks.len() == 2);
    ensure!(
        admitted_checks[0].contains("inet_server_port")
            && admitted_checks[1].contains("g.position_hints_complete")
    );
    let handshake_ms = u64::try_from(handshake_start.elapsed().as_millis())?;
    let candidate = ready.writer().clone();
    let journal =
        witness_daemon_start(restore.clone(), ready, g.tenant_id, successor.process_epoch).await?;
    // This restricted role deliberately cannot update incidents. The same
    // release using the installer's pool would be able to commit.
    let release = robson_exec::order_journal::ReleaseRecord {
        decision_id: Uuid::now_v7(),
        generation: stamp,
        actor: "disposable-fixture".into(),
        auth_kind: "verified",
        note: "restricted successor credential control".into(),
        treatment: "unbooked_accepted",
        decided_at: Utc::now(),
    };
    ensure!(journal.gate().generation() == stamp);
    let refused = journal.release(release).await;
    ensure!(
        matches!(refused, Err(ReleaseRefusal::Backend(reason))
        if reason.contains("permission denied") && reason.contains("recovery_incidents")),
        "supervised release did not use the restricted writer"
    );
    // Feed a simulated completion to the journal attached to this daemon.
    // It adds no attempt, clock, request or ticket.
    let j = journal.open_intent(IntentSpec {
        position_id: Some(Uuid::now_v7()),
        kind: IntentKind::Exit,
        client_order_id: format!("fixture-{}", Uuid::now_v7().simple()),
        cancels_order_id: None,
        params: json!({}),
    });
    journal.finish_success(&j, Some(json!({"fixture": true}))).await;
    ensure!(journal.retry_set_depth() == 0);
    ensure!(fields(&restore, &successor).await? == (committed.0 + 1, committed.1 + 1, 2, 0, 2, 0));
    ensure!(
        j_present(&restore, j.intent_id()).await?
            && j_present(&restore, original.intent.intent_id).await?
    );
    candidate.close().await;
    drop(journal);
    drop(reader);
    restore.close().await;
    channel.phase("successor",json!({"generation":successor.generation_id,"version":committed.0+1,"intent":j.intent_id(),
        "startup_identity_queries":installer_checks,"successor_verification_queries":admitted_checks.len(),
        "handshake_ms":handshake_ms,"daemon_gate_verified":true,"release_login_verified":true,
        "venue_wire_observed":cfg!(feature = "crash-points")})).await?;
    Ok(())
}

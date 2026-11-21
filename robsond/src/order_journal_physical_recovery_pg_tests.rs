//! Test-only real Journal/Spot audit composition driven by a disposable PG16
//! controller.
use std::io::{self, BufRead, Read, Write};

use robson_domain::OrderSide;
use robson_exec::{
    venue::{OrderPhase, VenueOrderRecord},
    venue_ledger::{OrderLink, OrderOwner, SeenAt},
};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use tokio::time::Instant;

use super::*;
use crate::{
    spot_order_set_store::{
        cold_install::{install, ClosedWriterFence, FencePhase, FenceReceipt, FixtureFenceError},
        cold_owner::{capture, OwnerRequest, RestoreIdentity},
        observe_generation,
        physical_fixture::{
            audit_cold_sources, AttemptSource, ColdAuditRefusal, ColdAuditScope, ColdSourceSink,
            Family, IntentSource, OrderSource, PageBudget, RouteClass, RouteSource, SinkFull,
            SourceKey,
        },
        ExpectedGeneration,
    },
    venue_budget::{fixture as budget_fixture, physical_recovery as budget_recovery},
    venue_ledger_store::PgVenueLedger,
};

const SYMBOL: &str = "BTCUSDT";
const BUY_ID: &str = "417011";
const BUY_CLIENT: &str = "physical-recovery-observed-buy";
const BUDGET: PageBudget = PageBudget {
    intents: 1,
    attempts: 1,
    orders: 1,
    routes: 2,
};

struct Channel {
    operation: Uuid,
    owner: String,
    fixture: String,
    previous: String,
    seq: u64,
}

impl Channel {
    fn open() -> Result<(Self, Value)> {
        let greeting = Self::read()?;
        ensure!(greeting["schema"] == 1);
        let revision = Self {
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

    fn read() -> Result<Value> {
        let mut bytes = String::new();
        let n = io::stdin().lock().take(16_385).read_line(&mut bytes)?;
        ensure!(
            n > 0 && n <= 16_384 && bytes.ends_with('\n'),
            "invalid private controller frame size"
        );
        serde_json::from_str(&bytes)
            .map_err(|_| anyhow::anyhow!("malformed private controller frame"))
    }

    fn phase(&mut self, phase: &str, facts: Value) -> Result<Value> {
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
        {
            let mut stdout = io::stdout().lock();
            writeln!(stdout, "\nROBSON_PHASE {frame}")?;
            stdout.flush()?;
        }
        let received = Self::read()?;
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
            previous != self.previous
                && previous.len() == 64
                && previous.bytes().all(|b| b.is_ascii_hexdigit()),
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

async fn seed_native_sources(pool: &PgPool, g: &JournalGeneration) -> Result<()> {
    let venue = VenueId::new(VenueId::BINANCE_SPOT);
    sqlx::query(
        "INSERT INTO public.venue_accounts(venue_id,account_scope,tenant_id,uid,bound_at,cutover_id)
         VALUES('binance:spot',$1,$2,'physical-recovery-fixture',now(),$3)",
    ).bind(&g.account_scope).bind(g.tenant_id).bind(Uuid::now_v7()).execute(pool).await?;
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
    Ok(())
}

#[derive(Default)]
struct NativeKeys {
    intents: Vec<(Uuid, String, Option<Uuid>)>,
    attempts: usize,
    orders: Vec<(String, String, String)>,
    routes: Vec<Uuid>,
}
impl ColdSourceSink for NativeKeys {
    fn intents(&mut self, rows: &[IntentSource]) -> std::result::Result<(), SinkFull> {
        if self.intents.len() + rows.len() > 1 {
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
        if self.orders.len() + rows.len() > 1 {
            return Err(SinkFull);
        }
        self.orders.extend(
            rows.iter()
                .map(|o| (o.symbol.clone(), o.exchange_order_id.clone(), o.side.clone())),
        );
        Ok(())
    }
    fn routes(&mut self, rows: &[RouteSource]) -> std::result::Result<(), SinkFull> {
        if self.routes.len() + rows.len() > 8
            || rows.iter().any(|r| r.classify() != RouteClass::ArmedOnly)
        {
            return Err(SinkFull);
        }
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
            FencePhase::BeforeRead => "fence_before_install",
            FencePhase::BeforePublish => "fence_before_publish",
            FencePhase::AfterReadback => "fence_after_readback",
        };
        let approved = self
            .phase(name, json!({"target_id": restore.target}))
            .map_err(|_| FixtureFenceError)?;
        if approved["target_id"] != restore.target.to_string() || Instant::now() >= deadline {
            return Err(FixtureFenceError);
        }
        let owner = self.owner.parse().map_err(|_| FixtureFenceError)?;
        Ok(FenceReceipt { restore, owner })
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

#[sqlx::test(migrations = false)]
#[ignore = "requires dedicated physical controller and isolated PG16 over loopback"]
async fn physical_spot_writer_survives_exact_native_restore(pool: PgPool) -> Result<()> {
    let (mut channel, greeting) = Channel::open()?;
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
    let g = seed_generation(&pool).await?;
    seed_native_sources(&pool, &g).await?;
    validate_writer_roles(&pool, old_role, new_role).await?;
    let before = fields(&pool, &g).await?;
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
    channel.phase(
        "old_connected",
        json!({"database":db,"pid":pid,"backend_start":birth,
        "current_user":user,"tenant":g.tenant_id}),
    )?;
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
        accounting_generation: Default::default(),
    };
    proxy.enable_exact_query_trace(12);
    proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE public.spot_order_set_generation g"));
    let original = sell();
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
        "native_statements":6,"lost_commit_acks":1}))?;
    let target = Channel::string(&proof, "target")?;
    let restore_identity = RestoreIdentity {
        operation: channel.operation,
        target: Channel::string(&proof, "target_id")?.parse()?,
    };
    ensure!(!restore_identity.target.is_nil());
    let restore = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(upstream.as_ref().clone())
        .await?;
    let exact_cluster: (String, String) = sqlx::query_as(
        "SELECT system_identifier::text,current_setting('data_directory') FROM pg_control_system()",
    )
    .fetch_one(&restore)
    .await?;
    ensure!(exact_cluster.0 == Channel::string(&proof, "system_id")? && exact_cluster.1 == target);
    let venue = VenueId::new(VenueId::BINANCE_SPOT);
    let mut reader = restore.connect_options().connect().await?;
    let predecessor = observe_generation(&mut reader, binding(&g, &venue))
        .await?
        .context("copied structural generation absent")?;
    ensure!(
        predecessor.accounting_version == committed.0 && fields(&restore, &g).await? == committed
    );
    let new =
        crate::process_epoch::start_epoch(&restore, g.tenant_id, "physical-writer-fixture").await?;
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
    channel.phase(
        "epoch",
        json!({"predecessor":g.process_epoch,"epoch":new.epoch.epoch_id,"stamp":stamp}),
    )?;
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
        &mut NativeKeys::default(),
    )
    .await;
    ensure!(
        matches!(stopped,Err(ColdAuditRefusal::PageBudget {family:Family::Routes,last_key:Some(SourceKey::Route(key))}) if key==Uuid::from_u128(8))
    );
    let request = OwnerRequest {
        scope,
        expected_uid: "physical-recovery-fixture",
        epoch: new.epoch.epoch_id,
        accounting_version: committed.0,
        restore: restore_identity,
    };
    let cut = capture(&restore, &request, deadline)
        .await
        .map_err(|e| anyhow::anyhow!("real anchored Spot cold audit: {e:?}"))?;
    let mut keys = NativeKeys::default();
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
    ensure!(
        keys.intents
            == [(
                original.intent.intent_id,
                original.intent.client_order_id.clone(),
                original.intent.position_id
            )]
            && keys.attempts == 0
            && keys.orders == [(SYMBOL.into(), BUY_ID.into(), "BUY".into())]
            && keys.routes == (1..=8).map(Uuid::from_u128).collect::<Vec<_>>()
    );
    let families=receipt.families.iter().zip(["intents","attempts","orders","routes"])
        .map(|(f,name)| json!({"family":name,"rows":f.rows,"pages":f.pages,"last_key":key_value(f.last_key.as_ref())}))
        .collect::<Vec<_>>();
    channel.phase(
        "cold",
        json!({"tenant":g.tenant_id,"scope":g.account_scope,"epoch":new.epoch.epoch_id,
        "version":committed.0,"families":families,"statements":receipt.statements}),
    )?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let audit = budget_recovery::audit_empty_budget(&restore, &copied_budget, deadline).await?;
    channel.phase("budget",json!({"audit":audit.audit,"quiescence_generation":audit.generation,
        "source":0,"clocks":0,"charges":0,"statements":audit.start_commands+audit.pass_commands+audit.terminal_commands}))?;
    let bootstrap = budget_recovery::bootstrap_empty_budget(
        &restore,
        g.tenant_id,
        &new,
        &copied_budget,
        &audit,
        &clock,
        deadline,
    )
    .await?;
    channel.phase("bootstrap",json!({"audit":bootstrap.audit,"quiescence_generation":bootstrap.generation,
        "epoch":bootstrap.process_epoch,"statements":bootstrap.bootstrap_commands,"stale_statements":bootstrap.stale_commands}))?;
    let installed = install(&restore, &cut, &mut channel, Instant::now() + Duration::from_secs(5))
        .await
        .map_err(|e| anyhow::anyhow!("exact fenced cold installation: {e:?}"))?;
    ensure!(installed.restore == restore_identity && installed.accounting_version == committed.0);
    let successor = installed.generation;
    ensure!(observe_generation(&mut reader, binding(&successor, &venue)).await?.is_some());
    ensure!(observe_generation(&mut reader, binding(&g, &venue)).await?.is_none());
    channel.phase("installed",json!({"tenant":g.tenant_id,"scope":g.account_scope,"epoch":successor.process_epoch,
        "version":committed.0,"audit":successor.source_audit_id,"generation":successor.generation_id}))?;
    let permit = channel.phase(
        "admit",
        json!({"epoch":successor.process_epoch,"audit":successor.source_audit_id,
        "generation":successor.generation_id}),
    )?;
    ensure!(Channel::string(&permit, "new_user")? == new_role);
    let candidate = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            upstream
                .as_ref()
                .clone()
                .username(new_role)
                .password(Channel::string(&permit, "new_password")?)
                .ssl_mode(PgSslMode::Disable),
        )
        .await
        .map_err(|_| anyhow::anyhow!("admitted candidate failed to connect"))?;
    let as_role: String =
        sqlx::query_scalar("SELECT current_user::text").fetch_one(&candidate).await?;
    ensure!(as_role == new_role);
    if std::env::var("ROBSON_PHYSICAL_FAULT").as_deref() == Ok("post-admit-eof") {
        // The supervisor already verified the new role and will close the
        // HBA before destroying the exact restored fixture.
        std::process::exit(68);
    }
    let current = PgJournalBackend {
        pool: candidate.clone(),
        daemon_pool: restore.clone(),
        tenant_id: g.tenant_id,
        ready_wait: JOURNAL_READY_WAIT,
        balance_account: false,
        order_set_projection: Some(successor.clone()),
        accounting_generation: Default::default(),
    };
    let j = sell();
    insist(current.commit(&j).await)?;
    ensure!(fields(&restore, &successor).await? == (committed.0 + 1, committed.1 + 1, 2, 0, 2, 0));
    ensure!(
        j_present(&restore, j.intent.intent_id).await?
            && j_present(&restore, original.intent.intent_id).await?
    );
    candidate.close().await;
    drop(current);
    drop(reader);
    restore.close().await;
    channel.phase("successor",json!({"generation":successor.generation_id,"version":committed.0+1,"intent":j.intent.intent_id}))?;
    Ok(())
}

//! Test-only bridge from the one initialized fixture read to the private
//! first-cancel writer. A prepared value grants no venue route, R1/R3 policy
//! verdict, runtime readiness or authority to send. Its accounting version is
//! enforced only by the writer's own locked comparison before either insert.
use std::{ptr, time::Duration};

use anyhow::{anyhow, ensure, Result};
use chrono::{DateTime, Utc};
use proptest::{
    collection::vec as cases,
    strategy::{Strategy, ValueTree},
    test_runner::{Config, RngSeed, TestRunner},
};
use robson_domain::{MonthlyBudgetModel, OrderSide};
use robson_exec::{
    balance_liability::TransitionKind,
    order_journal::{
        AttemptRow, BalanceBatch, CommitFailure, IntentKind, IntentRow, JournalBackend,
        JournalBatch, OutcomeRow, VenueOutcome, COMMIT_BOUND,
    },
    venue::{OrderPhase, ProtectionKind, VenueId, VenueOrderRecord},
    venue_ledger::{LegOrderKind, OrderLink, OrderOwner, SeenAt, VenueIntId},
};
use robson_testkit::db_proxy::ProxyMode;
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use self::ready::{initialize, statement, Keys, ProjectionReady, ReplayOrder, ORDER_SET_COLUMNS};
use super::{
    faults::{blackhole_commit, durable, Wire},
    positive::migrate_fixture,
    ExpectedTransition, StoredFirstCancel, TransitionWrite,
};
use crate::{order_journal_store::PgJournalBackend, venue_ledger_store::PgVenueLedger};

#[allow(
    dead_code,
    reason = "the shared initialization fixture includes reference-target controls"
)]
#[path = "../tests/spot_order_set_reference_pg/initialized_fixture.rs"]
mod ready;

#[path = "../tests/venue_ledger_schema/empty_budget.rs"]
mod empty_budget;

const VENUE: &str = "binance:spot";
const SYMBOL: &str = "BTCUSDT";
const STOP: &str = "4100001";

/// Exact target-journal linkage and transition/journal readiness, in the same
/// statement as the initialized order set. `$10/$11` are the cancel identity.
const TARGET_COLUMNS: &str = "g.generation,
    o.tenant_id AS target_tenant, o.position_id AS target_position,
    o.client_order_id AS target_client, tj.intent_id AS target_intent,
    tj.client_order_id AS target_intent_client,
    (SELECT t.transition_id FROM balance_transitions t
      WHERE (t.tenant_id,t.venue_id,t.account_scope,t.symbol,t.position_id)=($1,$2,$3,$5,$4)
        AND t.completed_at IS NULL AND t.first_cancel_intent_id IS NULL
        AND t.original_target_order_id IS NULL) AS open_transition,
    EXISTS (SELECT FROM balance_transitions t
      WHERE (t.tenant_id,t.venue_id,t.account_scope,t.symbol,t.position_id)=($1,$2,$3,$5,$4)
        AND t.completed_at IS NULL
        AND (t.first_cancel_intent_id IS NOT NULL OR t.original_target_order_id IS NOT NULL))
      AS bound_transition,
    EXISTS (SELECT FROM order_intents i WHERE i.intent_id=$10
      OR (i.venue_id=$2 AND i.account_scope=$3 AND i.client_order_id=$11)) AS cancel_spent";
const TARGET_JOINS: &str = "
    LEFT JOIN venue_orders o
      ON (o.venue_id,o.account_scope,o.symbol,o.exchange_order_id)=($2,$3,$5,$6::text)
    LEFT JOIN order_intents tj ON tj.intent_id=o.intent_id AND tj.kind='protection'";

fn target_statement() -> String {
    statement(&format!("{ORDER_SET_COLUMNS}, {TARGET_COLUMNS}"), TARGET_JOINS)
}

#[derive(sqlx::FromRow)]
struct InitializedTarget {
    generation: Uuid,
    position_id: Uuid,
    accounting_version: i64,
    leg_sell_conflicts: i64,
    symbol_sell: i64,
    symbol_sell_conflicts: i64,
    grouped_cancel: i64,
    grouped_cancel_conflicts: i64,
    ungroupable_cancel: i64,
    ungroupable_cancel_conflicts: i64,
    unbound_cancel: i64,
    unbound_cancel_conflicts: i64,
    unbound_sell: i64,
    unbound_sell_conflicts: i64,
    stop_id: Option<String>,
    stop_phase: Option<String>,
    stop_status: Option<String>,
    exact_stop_identity: bool,
    target_tenant: Option<Uuid>,
    target_position: Option<Uuid>,
    target_client: Option<String>,
    target_intent: Option<Uuid>,
    target_intent_client: Option<String>,
    open_transition: Option<Uuid>,
    bound_transition: bool,
    cancel_spent: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    Input,
    Read(String),
    Uninitialized,
    Spent,
    Target,
    BoundTransition,
    Conflicts,
    Mismatch,
    Write(CommitFailure),
}

/// One initialized generation and one exact writer backend. Requests written
/// through it are consumed even when their outcome is unknown.
struct FirstCancelBridge<'a> {
    ready: &'a ProjectionReady,
    backend: &'a PgJournalBackend,
}

/// Created only from a completed initialized read. Not Clone, Copy or
/// Deserialize; a write consumes it.
struct PreparedFirstCancel<'a> {
    bridge: &'a FirstCancelBridge<'a>,
    generation: Uuid,
    intent: &'a IntentRow,
    symbol: &'a str,
    position_id: Uuid,
    original: &'a str,
    target_intent: Uuid,
    observed_accounting_version: i64,
    expected_transition: ExpectedTransition,
}

impl<'a> FirstCancelBridge<'a> {
    fn new(ready: &'a ProjectionReady, backend: &'a PgJournalBackend) -> Self {
        Self { ready, backend }
    }

    async fn prepare<'b>(
        &'b self,
        intent: &'b IntentRow,
        symbol: &'b str,
    ) -> Result<PreparedFirstCancel<'b>, Refusal> {
        let position = intent.position_id.filter(|id| !id.is_nil()).ok_or(Refusal::Input)?;
        let original = intent.cancels_order_id.as_deref().ok_or(Refusal::Input)?;
        if intent.venue_id != VenueId::BINANCE_SPOT
            || intent.kind != IntentKind::Cancel
            || intent.intent_id.is_nil()
            || intent.account_scope.trim().is_empty()
            || intent.client_order_id.trim().is_empty()
            || symbol.trim().is_empty()
            || !original.parse::<VenueIntId>().is_ok_and(|id| id.to_string() == original)
        {
            return Err(Refusal::Input);
        }
        if self.ready.cancel_consumed(intent) {
            return Err(Refusal::Spent);
        }
        let keys = Keys {
            tenant: self.backend.tenant_id,
            venue: intent.venue_id.as_str(),
            account: intent.account_scope.as_str(),
            position,
            symbol,
        };
        let sql = target_statement();
        let read = self
            .ready
            .query_as::<InitializedTarget>(&sql, keys, Some(original))
            .bind(intent.intent_id)
            .bind(intent.client_order_id.as_str())
            .fetch_optional(&self.backend.pool);
        let row = tokio::time::timeout(COMMIT_BOUND, read)
            .await
            .map_err(|_| Refusal::Read("initialized read missed its bound".into()))?
            .map_err(|error| Refusal::Read(error.to_string()))?
            .ok_or(Refusal::Uninitialized)?;
        if row.cancel_spent {
            return Err(Refusal::Spent);
        }
        if row.generation != self.ready.generation() || row.position_id != position {
            return Err(Refusal::Uninitialized);
        }
        let target_intent = row.target_intent.ok_or(Refusal::Target)?;
        if row.stop_id.as_deref() != Some(original)
            || row.stop_phase.as_deref() != Some("live")
            || row.stop_status.as_deref().map(OrderPhase::from_binance_spot_status)
                != Some(OrderPhase::Live)
            || !row.exact_stop_identity
            || row.target_tenant != Some(self.backend.tenant_id)
            || row.target_position != Some(position)
            || row.target_client.is_none()
            || row.target_client != row.target_intent_client
        {
            return Err(Refusal::Target);
        }
        if row.bound_transition {
            return Err(Refusal::BoundTransition);
        }
        if [
            row.leg_sell_conflicts,
            row.symbol_sell,
            row.symbol_sell_conflicts,
            row.grouped_cancel,
            row.grouped_cancel_conflicts,
            row.ungroupable_cancel,
            row.ungroupable_cancel_conflicts,
            row.unbound_cancel,
            row.unbound_cancel_conflicts,
            row.unbound_sell,
            row.unbound_sell_conflicts,
        ]
        .iter()
        .any(|&count| count != 0)
        {
            return Err(Refusal::Conflicts);
        }
        Ok(PreparedFirstCancel {
            bridge: self,
            generation: row.generation,
            intent,
            symbol,
            position_id: position,
            original,
            target_intent,
            observed_accounting_version: row.accounting_version,
            expected_transition: row.open_transition.map_or_else(
                || ExpectedTransition::New(Uuid::now_v7()),
                ExpectedTransition::ExistingOpen,
            ),
        })
    }
}

impl PreparedFirstCancel<'_> {
    /// Every binding is compared before SQL. The request is consumed before
    /// the transaction, so an unknown COMMIT is never replayed by a new read.
    async fn write(
        self,
        bridge: &FirstCancelBridge<'_>,
        intent: &IntentRow,
        kind: TransitionKind,
        target_trigger: Decimal,
        reason: &str,
    ) -> Result<StoredFirstCancel, Refusal> {
        if !ptr::addr_eq(self.bridge, bridge)
            || self.generation != bridge.ready.generation()
            || self.intent != intent
            || intent.position_id != Some(self.position_id)
            || intent.cancels_order_id.as_deref() != Some(self.original)
        {
            return Err(Refusal::Mismatch);
        }
        if !bridge.ready.consume_cancel(intent) {
            return Err(Refusal::Spent);
        }
        bridge
            .backend
            .commit_first_cancel(
                intent,
                TransitionWrite {
                    symbol: self.symbol,
                    kind,
                    target_trigger,
                    reason,
                    expected_accounting_version: self.observed_accounting_version,
                    expected_transition: self.expected_transition,
                },
            )
            .await
            .map_err(Refusal::Write)
    }
}

fn seen(second: i64) -> SeenAt {
    SeenAt {
        at: DateTime::from_timestamp(1_794_614_400 + second, 0).expect("fixture time"),
        clock: None,
        capture: None,
    }
}

fn journal_batch(intent: IntentRow) -> JournalBatch {
    JournalBatch {
        intent,
        attempts: Vec::new(),
        outcome: None,
        balance: Some(BalanceBatch {
            admission: None,
            handed_off: false,
            commit_bound: Duration::from_millis(500),
        }),
    }
}

fn order(id: &str, client: &str, status: &str, phase: OrderPhase) -> VenueOrderRecord {
    VenueOrderRecord {
        exchange_order_id: id.into(),
        client_order_id: client.into(),
        orig_client_order_id: None,
        symbol: SYMBOL.into(),
        side: OrderSide::Sell,
        order_type: "STOP_LOSS".into(),
        protection_kind: Some(ProtectionKind::MarketOnTrigger),
        status: status.into(),
        phase,
        orig_qty: Decimal::from(10),
        executed_qty: Decimal::ZERO,
        cumulative_quote_qty: Some(Decimal::ZERO),
        stop_price: Some(Decimal::from(90)),
        expiry_reason: None,
        transact_time_ms: None,
        time_ms: None,
        update_time_ms: None,
        working_time_ms: None,
    }
}

/// An admitted leg whose live native stop is linked to its protection
/// journal, seeded through the canonical journal and venue writers.
struct ProtectedLeg {
    tenant: Uuid,
    position: Uuid,
    account: String,
    ledger: PgVenueLedger,
    journal: PgJournalBackend,
    stop_link: OrderLink,
    stop_client: String,
}

impl ProtectedLeg {
    async fn admit(pool: &PgPool) -> Result<Self> {
        let tenant = Uuid::now_v7();
        let position = Uuid::now_v7();
        let admission = Uuid::now_v7();
        let account = format!("bridge-{tenant}");
        let venue = VenueId::new(VENUE);
        let ledger = PgVenueLedger::new(pool, tenant);
        ledger
            .ensure_accounting_row(&venue, MonthlyBudgetModel::NetFromStartNonExpandingV1)
            .await
            .map_err(|error| anyhow!("accounting row: {error:?}"))?;
        ledger
            .insert_route(position, &venue, &account, seen(0).at)
            .await
            .map_err(|error| anyhow!("leg route: {error:?}"))?;
        sqlx::query(
            "INSERT INTO order_intents(intent_id,tenant_id,venue_id,account_scope,client_order_id,
             position_id,kind,params,created_at) VALUES($1,$2,$3,$4,$5,$6,'entry','{}',$7)",
        )
        .bind(admission)
        .bind(tenant)
        .bind(VENUE)
        .bind(&account)
        .bind(admission.to_string())
        .bind(position)
        .bind(seen(0).at)
        .execute(pool)
        .await?;
        let mut tx = pool.begin().await?;
        sqlx::query(
            "UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2",
        )
        .bind(tenant)
        .bind(VENUE)
        .execute(&mut *tx)
        .await?;
        // Synthetic chart fixture: 90 is its analysis's second support, never a
        // percentage stop. BTCUSDT is only an example symbol.
        sqlx::query("SELECT balance_leg_admit($1,$2,$3,$4,$5,$6,'2026-11-01',10,10,200,5,0.01,'base','fixture-chart',$7,'fixture-hash',1,1.01,100,0.1,12,90,$7,false)")
            .bind(tenant).bind(VENUE).bind(&account).bind(SYMBOL).bind(position).bind(admission)
            .bind(seen(0).at).execute(&mut *tx).await?;
        tx.commit().await?;

        let journal = PgJournalBackend::new(pool, tenant);
        let mut leg = Self {
            tenant,
            position,
            account,
            ledger,
            journal,
            stop_link: OrderLink {
                venue,
                account_scope: String::new(),
                owner: OrderOwner::Discovered,
                client_order_id: String::new(),
            },
            stop_client: String::new(),
        };
        let protection = leg.intent(IntentKind::Protection, None);
        leg.commit(&journal_batch(protection.clone())).await?;
        leg.stop_link = OrderLink {
            venue: VenueId::new(VENUE),
            account_scope: leg.account.clone(),
            owner: OrderOwner::Leg {
                position_id: position,
                intent_id: protection.intent_id,
                kind: LegOrderKind::Stop,
            },
            client_order_id: protection.client_order_id.clone(),
        };
        leg.stop_client = protection.client_order_id;
        leg.record(&order(STOP, &leg.stop_client, "NEW", OrderPhase::Live), &leg.stop_link, 1)
            .await?;
        Ok(leg)
    }

    fn intent(&self, kind: IntentKind, target: Option<&str>) -> IntentRow {
        let id = Uuid::now_v7();
        IntentRow {
            intent_id: id,
            venue_id: VENUE.to_owned(),
            account_scope: self.account.clone(),
            client_order_id: if kind == IntentKind::Cancel {
                id.to_string()
            } else {
                format!("reference-{id}")
            },
            position_id: Some(self.position),
            kind,
            cancels_order_id: target.map(str::to_owned),
            params: if kind == IntentKind::Cancel {
                json!({})
            } else {
                json!({ "quantity": "10" })
            },
            created_at: if kind == IntentKind::Cancel {
                Utc::now()
            } else {
                seen(0).at
            },
        }
    }

    fn cancel(&self) -> IntentRow {
        self.intent(IntentKind::Cancel, Some(STOP))
    }

    async fn commit(&self, batch: &JournalBatch) -> Result<()> {
        self.journal
            .commit(batch)
            .await
            .map_err(|failure| anyhow!("fixture journal commit: {failure:?}"))?;
        Ok(())
    }

    async fn record(&self, record: &VenueOrderRecord, link: &OrderLink, at: i64) -> Result<()> {
        let written = self
            .ledger
            .record_order(record, link, seen(at))
            .await
            .map_err(|error| anyhow!("fixture venue write: {error:?}"))?;
        ensure!(written.changed, "fixture venue write changed nothing");
        Ok(())
    }

    async fn version(&self, pool: &PgPool) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id=$2",
        )
        .bind(self.tenant)
        .bind(VENUE)
        .fetch_one(pool)
        .await?)
    }

    async fn open_transition(&self, pool: &PgPool) -> Result<Uuid> {
        let transition = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO balance_transitions
            (transition_id,position_id,tenant_id,venue_id,account_scope,symbol,kind,target_trigger,reason,started_at)
            VALUES($1,$2,$3,$4,$5,$6,'replace',95,'existing-open',now())",
        )
        .bind(transition)
        .bind(self.position)
        .bind(self.tenant)
        .bind(VENUE)
        .bind(&self.account)
        .bind(SYMBOL)
        .execute(pool)
        .await?;
        Ok(transition)
    }

    /// One canonical source writer on the same accounting owner. Each moves
    /// the accounting version under the writer's own lock.
    async fn canonical_mutation(&self, action: u8, index: usize) -> Result<()> {
        match action {
            0 => self.commit(&journal_batch(self.intent(IntentKind::Exit, None))).await,
            1 => {
                let mut batch = journal_batch(self.intent(IntentKind::Exit, None));
                batch.attempts.push(AttemptRow {
                    attempt_id: Uuid::now_v7(),
                    created_at: seen(601).at,
                    rejected_at: Some(seen(602).at),
                });
                batch.outcome = Some(OutcomeRow {
                    venue_outcome: Some(VenueOutcome::Rejected),
                    ..OutcomeRow::default()
                });
                self.commit(&batch).await
            },
            2 => {
                let link = OrderLink {
                    owner: OrderOwner::Discovered,
                    client_order_id: format!("race-{index}"),
                    ..self.stop_link.clone()
                };
                let id = (20_000 + index).to_string();
                self.record(&order(&id, &link.client_order_id, "NEW", OrderPhase::Live), &link, 603)
                    .await
            },
            _ => unreachable!(),
        }
    }
}

async fn fixture(pool: &PgPool) -> Result<()> {
    migrate_fixture(pool).await?;
    empty_budget::initialize(pool, seen(0).at).await?;
    Ok(())
}

fn written(result: Result<StoredFirstCancel, Refusal>) -> Result<StoredFirstCancel> {
    result.map_err(|refusal| anyhow!("prepared first cancel refused: {refusal:?}"))
}

fn assert_trace(wire: &Wire, prefixes: &[&str]) {
    let trace = wire.proxy.query_trace();
    assert_eq!(trace.len(), prefixes.len(), "unexpected bridge statements: {trace:?}");
    for (command, prefix) in trace.iter().zip(prefixes) {
        assert!(command.trim_start().starts_with(prefix), "expected {prefix}: {command:?}");
    }
}

const READ: &str = "SELECT";
const WRITER_FENCE: [&str; 3] = [
    "BEGIN",
    "SELECT set_config('lock_timeout'",
    "UPDATE accounting_state",
];

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn prepared_first_cancel_bridge_new_and_existing_open(pool: PgPool) -> Result<()> {
    fixture(&pool).await?;
    let fresh = ProtectedLeg::admit(&pool).await?;
    let attached = ProtectedLeg::admit(&pool).await?;
    let open = attached.open_transition(&pool).await?;
    let ready = initialize(&pool, ReplayOrder::MembersFirst, None).await?;
    for (leg, existing) in [(&fresh, None), (&attached, Some(open))] {
        let wire = Wire::new(&pool, leg.tenant).await?;
        let bridge = FirstCancelBridge::new(&ready, &wire.store);
        let version = leg.version(&pool).await?;
        let intent = leg.cancel();
        let prepared = bridge.prepare(&intent, SYMBOL).await.map_err(|r| anyhow!("{r:?}"))?;
        assert_trace(&wire, &[READ]);
        assert_eq!(prepared.observed_accounting_version, version);
        assert_eq!(
            prepared.target_intent,
            match leg.stop_link.owner {
                OrderOwner::Leg { intent_id, .. } => intent_id,
                _ => unreachable!(),
            }
        );
        let transition = match (existing, prepared.expected_transition) {
            (Some(open), ExpectedTransition::ExistingOpen(id)) if id == open => id,
            (None, ExpectedTransition::New(id)) if !id.is_nil() => id,
            (open, expected) => anyhow::bail!("prepared {expected:?} for open {open:?}"),
        };
        let stored = written(
            prepared
                .write(&bridge, &intent, TransitionKind::Replace, Decimal::new(96, 0), "bridge")
                .await,
        )?;
        assert_eq!(
            stored,
            StoredFirstCancel {
                transition_id: transition,
                intent_id: intent.intent_id,
                accounting_version: version + 1,
            }
        );
        let transition_sql = if existing.is_some() {
            "UPDATE balance_transitions"
        } else {
            "INSERT INTO balance_transitions"
        };
        let mut expected = vec![READ];
        expected.extend(WRITER_FENCE);
        expected.extend(["INSERT INTO order_intents", transition_sql, "COMMIT"]);
        assert_trace(&wire, &expected);
        let actual = durable(&pool, leg.tenant).await?;
        assert_eq!(actual["version"], version + 1);
        assert_eq!(actual["transitions"].as_array().map(Vec::len), Some(1));
        assert_eq!(actual["transitions"][0]["transition_id"], json!(transition));
        assert_eq!(actual["transitions"][0]["first_cancel_intent_id"], json!(intent.intent_id));
        assert_eq!(actual["transitions"][0]["original_target_order_id"], STOP);
        // The consumed request and a fresh bridge on the same generation both
        // refuse to prepare it again; the bound transition refuses a new one.
        assert_eq!(bridge.prepare(&intent, SYMBOL).await.err(), Some(Refusal::Spent));
        let again = FirstCancelBridge::new(&ready, &wire.store);
        assert_eq!(again.prepare(&intent, SYMBOL).await.err(), Some(Refusal::Spent));
        let next = leg.cancel();
        assert_eq!(again.prepare(&next, SYMBOL).await.err(), Some(Refusal::BoundTransition));
        assert_eq!(durable(&pool, leg.tenant).await?, actual);
    }
    eprintln!("SPOT_PREPARED_FIRST_CANCEL new=committed existing_open=attached hot_sql=1 writer_sql=6 replay=refused");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn prepared_first_cancel_bridge_refuses_before_the_writer(pool: PgPool) -> Result<()> {
    fixture(&pool).await?;
    let leg = ProtectedLeg::admit(&pool).await?;
    let foreign = ProtectedLeg::admit(&pool).await?;
    let pending = ProtectedLeg::admit(&pool).await?;
    pending.commit(&journal_batch(pending.cancel())).await?;
    let terminal = ProtectedLeg::admit(&pool).await?;
    let untracked = ProtectedLeg::admit(&pool).await?;
    let corrupt = ProtectedLeg::admit(&pool).await?;
    // Deliberately bypass the canonical record writer for this corruption control.
    let affected = sqlx::query(
        "UPDATE venue_orders SET status='CANCELED'
        WHERE (tenant_id,venue_id,account_scope,symbol,exchange_order_id)=($1,$2,$3,$4,$5)
          AND phase='live' AND status='NEW'",
    )
    .bind(corrupt.tenant)
    .bind(VENUE)
    .bind(&corrupt.account)
    .bind(SYMBOL)
    .bind(STOP)
    .execute(&pool)
    .await?;
    assert_eq!(affected.rows_affected(), 1);
    let ready = initialize(&pool, ReplayOrder::TargetsFirst, None).await?;
    // Post-initialization canonical writes are visible to the projection.
    terminal
        .record(
            &order(STOP, &terminal.stop_client, "CANCELED", OrderPhase::Terminal),
            &terminal.stop_link,
            2,
        )
        .await?;
    let discovered = OrderLink {
        owner: OrderOwner::Discovered,
        client_order_id: "untracked".into(),
        ..untracked.stop_link.clone()
    };
    untracked
        .record(&order("4100009", "untracked", "NEW", OrderPhase::Live), &discovered, 2)
        .await?;

    let wire = Wire::new(&pool, leg.tenant).await?;
    let before = durable(&pool, leg.tenant).await?;
    let base = leg.cancel();
    let wrong_target = IntentRow {
        cancels_order_id: Some("4100019".into()),
        ..leg.cancel()
    };
    let wrong_account = IntentRow {
        account_scope: foreign.account.clone(),
        ..leg.cancel()
    };
    let wrong_position = IntentRow {
        position_id: Some(foreign.position),
        ..leg.cancel()
    };
    let not_cancel = IntentRow { kind: IntentKind::Exit, ..leg.cancel() };
    let malformed = IntentRow {
        cancels_order_id: Some("04100001".into()),
        ..leg.cancel()
    };
    let bridge = FirstCancelBridge::new(&ready, &wire.store);
    for (intent, symbol, refusal, statements) in [
        (&not_cancel, SYMBOL, Refusal::Input, 0),
        (&malformed, SYMBOL, Refusal::Input, 0),
        (&base, "ETHUSDT", Refusal::Uninitialized, 1),
        (&wrong_account, SYMBOL, Refusal::Uninitialized, 1),
        (&wrong_position, SYMBOL, Refusal::Uninitialized, 1),
        (&wrong_target, SYMBOL, Refusal::Target, 1),
    ] {
        let read = wire.proxy.query_trace().len();
        assert_eq!(bridge.prepare(intent, symbol).await.err(), Some(refusal));
        assert_eq!(wire.proxy.query_trace().len() - read, statements);
    }
    // A foreign accounting owner cannot prepare this leg's cancel.
    let foreign_wire = Wire::new(&pool, foreign.tenant).await?;
    let foreign_bridge = FirstCancelBridge::new(&ready, &foreign_wire.store);
    assert_eq!(foreign_bridge.prepare(&base, SYMBOL).await.err(), Some(Refusal::Uninitialized));
    // The same initialized statement under an uninitialized generation is empty.
    let sql = target_statement();
    let keys = Keys {
        tenant: leg.tenant,
        venue: VENUE,
        account: &leg.account,
        position: leg.position,
        symbol: SYMBOL,
    };
    let absent = ready
        .foreign_query_as::<InitializedTarget>(&sql, keys, Some(STOP))
        .bind(base.intent_id)
        .bind(base.client_order_id.as_str())
        .fetch_optional(&pool)
        .await?;
    ensure!(absent.is_none(), "an uninitialized generation produced a prepared row");
    for (other, refusal) in [
        (&pending, Refusal::Conflicts),
        (&terminal, Refusal::Target),
        (&untracked, Refusal::Conflicts),
        (&corrupt, Refusal::Target),
    ] {
        let before = durable(&pool, other.tenant).await?;
        let wire = Wire::new(&pool, other.tenant).await?;
        let intent = other.cancel();
        let bridge = FirstCancelBridge::new(&ready, &wire.store);
        assert_eq!(bridge.prepare(&intent, SYMBOL).await.err(), Some(refusal));
        assert_trace(&wire, &[READ]);
        assert_eq!(durable(&pool, other.tenant).await?, before);
    }

    // A prepared value refuses another intent, another bridge on the same
    // generation and backend, and another backend, all before SQL.
    let twin_wire = Wire::new(&pool, leg.tenant).await?;
    let twin = FirstCancelBridge::new(&ready, &twin_wire.store);
    let retargeted = IntentRow {
        cancels_order_id: Some("4100003".into()),
        ..base.clone()
    };
    let other_intent = leg.cancel();
    for (written_by, intent) in [
        (&bridge, &other_intent),
        (&bridge, &retargeted),
        (&bridge, &wrong_account),
        (&twin, &base),
        (&foreign_bridge, &base),
    ] {
        let prepared = bridge.prepare(&base, SYMBOL).await.map_err(|r| anyhow!("{r:?}"))?;
        let read = wire.proxy.query_trace().len();
        let result = prepared
            .write(written_by, intent, TransitionKind::Replace, Decimal::new(96, 0), "mismatch")
            .await;
        assert_eq!(result, Err(Refusal::Mismatch));
        assert_eq!(wire.proxy.query_trace().len(), read);
        assert!(!ready.cancel_consumed(&base));
    }
    assert!(
        twin_wire.proxy.query_trace().is_empty() && foreign_wire.proxy.query_trace().len() == 1
    );
    assert_eq!(durable(&pool, leg.tenant).await?, before);
    let lost_read = Wire::new(&pool, leg.tenant).await?;
    lost_read
        .proxy
        .set_mode(ProxyMode::DropAutocommitAckOf("spot_shadow_initialization"));
    let missing = FirstCancelBridge::new(&ready, &lost_read.store);
    assert!(matches!(missing.prepare(&base, SYMBOL).await, Err(Refusal::Read(_))));
    assert_eq!(lost_read.proxy.autocommit_acks_dropped(), 1);
    assert_trace(&lost_read, &[READ]);
    lost_read.retires(&pool).await?;
    assert!(!ready.cancel_consumed(&base));
    assert_eq!(durable(&pool, leg.tenant).await?, before);
    eprintln!("SPOT_PREPARED_FIRST_CANCEL refusals=before_writer foreign_generation=empty mismatches=before_sql lost_read=refused");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn prepared_first_cancel_generated_canonical_race_is_fenced(pool: PgPool) -> Result<()> {
    fixture(&pool).await?;
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(2026111903),
        failure_persistence: None,
        ..Config::default()
    });
    // None: no writer between read and lock. Some(action): one canonical
    // accounting-first journal, rejected attempt or venue writer.
    let generated = cases(proptest::option::of(0_u8..3), 8)
        .new_tree(&mut runner)
        .map_err(|error| anyhow!("schedule generation: {error:?}"))?
        .current();
    let schedule: Vec<_> = [None, Some(0), Some(1), Some(2)].into_iter().chain(generated).collect();
    let mut legs = Vec::new();
    for _ in &schedule {
        legs.push(ProtectedLeg::admit(&pool).await?);
    }
    let ready = initialize(&pool, ReplayOrder::MembersFirst, None).await?;
    let mut fenced = 0;
    for (index, (leg, action)) in legs.iter().zip(&schedule).enumerate() {
        let wire = Wire::new(&pool, leg.tenant).await?;
        let bridge = FirstCancelBridge::new(&ready, &wire.store);
        let intent = leg.cancel();
        let prepared = bridge.prepare(&intent, SYMBOL).await.map_err(|r| anyhow!("{r:?}"))?;
        let observed = prepared.observed_accounting_version;
        if let Some(action) = *action {
            leg.canonical_mutation(action, index).await?;
        }
        let before = durable(&pool, leg.tenant).await?;
        let current = leg.version(&pool).await?;
        let result = prepared
            .write(&bridge, &intent, TransitionKind::Flatten, Decimal::new(97, 0), "generated")
            .await;
        if action.is_none() {
            ensure!(current == observed, "unscheduled accounting write");
            let stored = written(result)?;
            assert_eq!(
                (stored.intent_id, stored.accounting_version),
                (intent.intent_id, observed + 1)
            );
            continue;
        }
        fenced += 1;
        ensure!(current > observed, "canonical writer {action:?} did not move the version");
        assert_eq!(
            result,
            Err(Refusal::Write(CommitFailure::Known(
                "stale first-cancel accounting version".into()
            )))
        );
        let mut expected = vec![READ];
        expected.extend(WRITER_FENCE);
        assert_trace(&wire, &expected);
        wire.retires(&pool).await?;
        assert_eq!(
            durable(&pool, leg.tenant).await?,
            before,
            "a stale write left J or transition rows"
        );
        assert!(ready.cancel_consumed(&intent));
        assert_eq!(bridge.prepare(&intent, SYMBOL).await.err(), Some(Refusal::Spent));
    }
    eprintln!(
        "SPOT_PREPARED_FIRST_CANCEL cases={} fenced={fenced} journal_attempt_venue=covered seed=2026111903",
        schedule.len()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn prepared_first_cancel_lost_commit_is_never_replayed(pool: PgPool) -> Result<()> {
    fixture(&pool).await?;
    let leg = ProtectedLeg::admit(&pool).await?;
    let ready = initialize(&pool, ReplayOrder::MembersFirst, None).await?;
    let wire = Wire::new(&pool, leg.tenant).await?;
    wire.proxy
        .set_mode(ProxyMode::DropCommitAckOf("INSERT INTO balance_transitions"));
    let bridge = FirstCancelBridge::new(&ready, &wire.store);
    let intent = leg.cancel();
    let version = leg.version(&pool).await?;
    let prepared = bridge.prepare(&intent, SYMBOL).await.map_err(|r| anyhow!("{r:?}"))?;
    let result = prepared
        .write(&bridge, &intent, TransitionKind::Replace, Decimal::new(96, 0), "lost-ack")
        .await;
    assert!(
        matches!(&result, Err(Refusal::Write(CommitFailure::Uncertain(_)))),
        "{result:?}"
    );
    wire.retires(&pool).await?;
    assert_eq!(wire.proxy.stats().commit_acks_dropped, 1);
    let actual = durable(&pool, leg.tenant).await?;
    assert_eq!(actual["version"], version + 1);
    assert_eq!(actual["cancels"].as_array().map(Vec::len), Some(1));
    assert_eq!(actual["transitions"].as_array().map(Vec::len), Some(1));
    // The owning generation remembers the request across bridge recreation.
    let traced = wire.proxy.query_trace().len();
    assert_eq!(bridge.prepare(&intent, SYMBOL).await.err(), Some(Refusal::Spent));
    assert_eq!(wire.proxy.query_trace().len(), traced);
    let fresh_wire = Wire::new(&pool, leg.tenant).await?;
    let fresh = FirstCancelBridge::new(&ready, &fresh_wire.store);
    assert_eq!(fresh.prepare(&intent, SYMBOL).await.err(), Some(Refusal::Spent));
    assert_trace(&fresh_wire, &[]);
    assert_eq!(durable(&pool, leg.tenant).await?, actual);
    eprintln!("SPOT_PREPARED_FIRST_CANCEL lost_ack=uncertain replayed=false");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn prepared_first_cancel_unanswered_commit_blocks_new_reader(pool: PgPool) -> Result<()> {
    fixture(&pool).await?;
    let leg = ProtectedLeg::admit(&pool).await?;
    let ready = initialize(&pool, ReplayOrder::MembersFirst, None).await?;
    let wire = Wire::new(&pool, leg.tenant).await?;
    let bridge = FirstCancelBridge::new(&ready, &wire.store);
    let intent = leg.cancel();
    let before = durable(&pool, leg.tenant).await?;
    let prepared = bridge.prepare(&intent, SYMBOL).await.map_err(|r| anyhow!("{r:?}"))?;
    let result = blackhole_commit(
        prepared.write(&bridge, &intent, TransitionKind::Replace, Decimal::new(96, 0), "no-answer"),
        &wire.proxy,
    )
    .await?;
    assert!(matches!(result, Err(Refusal::Write(CommitFailure::Uncertain(_)))), "{result:?}");
    wire.retires(&pool).await?;
    assert_eq!(durable(&pool, leg.tenant).await?, before);
    drop(bridge);

    let next_wire = Wire::new(&pool, leg.tenant).await?;
    let next = FirstCancelBridge::new(&ready, &next_wire.store);
    let alias = IntentRow {
        intent_id: Uuid::now_v7(),
        ..intent.clone()
    };
    for request in [&intent, &alias] {
        assert_eq!(next.prepare(request, SYMBOL).await.err(), Some(Refusal::Spent));
    }
    assert_trace(&next_wire, &[]);
    assert_eq!(durable(&pool, leg.tenant).await?, before);
    eprintln!("SPOT_PREPARED_FIRST_CANCEL unanswered_commit=uncertain durable_change=false new_reader_replay=false");
    Ok(())
}

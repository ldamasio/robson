//! Positive local PostgreSQL witnesses of the first-cancel transition write:
//! a new transition, an existing open one, and an exact replay. Faults, wire
//! and proxy checks belong to the parent's other tests.

use anyhow::{anyhow, bail, ensure, Result};
use chrono::{DateTime, Utc};
use robson_exec::{
    balance_liability::TransitionKind,
    order_journal::{CommitFailure, IntentKind, IntentRow},
    venue::VenueId,
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use super::*;
use crate::order_journal_store::PgJournalBackend;

/// A dummy route: never bound to a venue account.
pub(super) const SYMBOL: &str = "BTCUSDT";
/// The original stop's canonical venue order id, distinct from any local
/// cancel client id.
pub(super) const ORIGINAL: &str = "4100001";

/// One admitted M36 leg on `binance:spot` with its live original stop.
pub(super) struct PositiveLeg {
    pub(super) tenant: Uuid,
    position: Uuid,
    scope: String,
    original: String,
    backend: PgJournalBackend,
}

/// (kind, target_trigger, reason, first_cancel_intent_id,
/// original_target_order_id, started_at, open)
type TransitionSnapshot =
    (String, String, String, Option<Uuid>, Option<String>, DateTime<Utc>, bool);

impl PositiveLeg {
    /// Admits the leg and its stop in one transaction that bumps the venue's
    /// accounting version once, as its first write. Assumes a migrated pool.
    pub(super) async fn admit(pool: &PgPool) -> Result<Self> {
        let tenant = Uuid::now_v7();
        Self::admit_on(pool, tenant, &format!("atomic-local-{tenant}"), ORIGINAL, SYMBOL).await
    }

    pub(super) async fn another_leg(&self, pool: &PgPool, original: &str) -> Result<Self> {
        Self::admit_on(pool, self.tenant, &self.scope, original, SYMBOL).await
    }

    pub(super) async fn alias_on_another_symbol(
        &self,
        pool: &PgPool,
        symbol: &str,
    ) -> Result<Self> {
        Self::admit_on(pool, self.tenant, &self.scope, &self.original, symbol).await
    }

    async fn admit_on(
        pool: &PgPool,
        tenant: Uuid,
        scope: &str,
        original: &str,
        symbol: &str,
    ) -> Result<Self> {
        let venue = VenueId::BINANCE_SPOT;
        let position = Uuid::now_v7();
        let entry = Uuid::now_v7();
        let stop = Uuid::now_v7();
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO accounting_state(tenant_id,venue_id,budget_model) VALUES($1,$2,'hwm_v1')
             ON CONFLICT DO NOTHING",
        )
        .bind(tenant)
        .bind(venue)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2",
        )
        .bind(tenant)
        .bind(venue)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO leg_routes(position_id,tenant_id,venue_id,account_scope,routed_at)
            VALUES($1,$2,$3,$4,now())",
        )
        .bind(position)
        .bind(tenant)
        .bind(venue)
        .bind(scope)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO order_intents
            (intent_id,tenant_id,venue_id,account_scope,client_order_id,position_id,kind,params,created_at)
            VALUES($1,$2,$3,$4,$1::uuid::text,$5,'entry','{}',now())",
        )
        .bind(entry)
        .bind(tenant)
        .bind(venue)
        .bind(scope)
        .bind(position)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "SELECT balance_leg_admit($1,$2,$3,$4,$5,$6,'2026-11-01',
            10,10,200,5,0.01,'base','fixture-chart',now(),'fixture-hash',1,1.01,100,0.1,12,90,now(),false)",
        )
        .bind(tenant)
        .bind(venue)
        .bind(scope)
        .bind(symbol)
        .bind(position)
        .bind(entry)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO venue_orders(venue_id,account_scope,symbol,exchange_order_id,tenant_id,
            client_order_id,last_client_order_id,owner,intent_id,position_id,kind,side,order_type,
            orig_qty,stop_price,status,phase,last_checked_at,live_seen_at)
            VALUES($1,$2,$3,$4,$5,$6::uuid::text,$6::uuid::text,
            'leg',$6,$7,'stop','SELL','STOP_LOSS',1,90,'NEW','live',now(),now())",
        )
        .bind(venue)
        .bind(scope)
        .bind(symbol)
        .bind(original)
        .bind(tenant)
        .bind(stop)
        .bind(position)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Self {
            tenant,
            position,
            scope: scope.to_owned(),
            original: original.to_owned(),
            backend: PgJournalBackend::new(pool, tenant),
        })
    }

    /// The local cancel of the original stop. Its client id is the intent's
    /// UUID, never a venue order id.
    pub(super) fn cancel_intent(&self) -> IntentRow {
        let intent_id = Uuid::now_v7();
        IntentRow {
            intent_id,
            venue_id: VenueId::BINANCE_SPOT.to_string(),
            account_scope: self.scope.clone(),
            client_order_id: intent_id.to_string(),
            position_id: Some(self.position),
            kind: IntentKind::Cancel,
            cancels_order_id: Some(self.original.clone()),
            params: serde_json::json!({}),
            created_at: Utc::now(),
        }
    }

    pub(super) async fn accounting_version(&self, pool: &PgPool) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id=$2",
        )
        .bind(self.tenant)
        .bind(VenueId::BINANCE_SPOT)
        .fetch_one(pool)
        .await?)
    }

    /// An open transition without a first cancel, written directly.
    async fn open_transition(
        &self,
        pool: &PgPool,
        kind: &str,
        target: i32,
        reason: &str,
    ) -> Result<Uuid> {
        let transition = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO balance_transitions
            (transition_id,position_id,tenant_id,venue_id,account_scope,symbol,kind,target_trigger,reason,started_at)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,now())",
        )
        .bind(transition)
        .bind(self.position)
        .bind(self.tenant)
        .bind(VenueId::BINANCE_SPOT)
        .bind(&self.scope)
        .bind(SYMBOL)
        .bind(kind)
        .bind(target)
        .bind(reason)
        .execute(pool)
        .await?;
        Ok(transition)
    }

    async fn transitions(&self, pool: &PgPool) -> Result<Vec<Uuid>> {
        Ok(
            sqlx::query_scalar(
                "SELECT transition_id FROM balance_transitions WHERE position_id=$1",
            )
            .bind(self.position)
            .fetch_all(pool)
            .await?,
        )
    }

    /// The journal row is exactly the cancel intent, with no attempts, no
    /// outcome and no handoff of the leg.
    pub(super) async fn assert_bare_cancel(&self, pool: &PgPool, intent: &IntentRow) -> Result<()> {
        let row: (Uuid, String, String, String, Option<Uuid>, String, Option<String>) = sqlx::query_as(
            "SELECT tenant_id,venue_id,account_scope,client_order_id,position_id,kind,cancels_order_id
            FROM order_intents WHERE intent_id=$1",
        )
        .bind(intent.intent_id)
        .fetch_one(pool)
        .await?;
        assert_eq!(
            row,
            (
                self.tenant,
                intent.venue_id.clone(),
                intent.account_scope.clone(),
                intent.client_order_id.clone(),
                intent.position_id,
                "cancel".to_string(),
                intent.cancels_order_id.clone(),
            )
        );
        let owners: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM order_intents WHERE venue_id=$1 AND account_scope=$2 AND client_order_id=$3",
        )
        .bind(&intent.venue_id)
        .bind(&intent.account_scope)
        .bind(&intent.client_order_id)
        .fetch_one(pool)
        .await?;
        assert_eq!(owners, 1);
        let bare: bool = sqlx::query_scalar(
            "SELECT venue_outcome IS NULL AND cancel_class IS NULL AND NOT not_sent
                AND venue_evidence IS NULL AND uncertain_at IS NULL AND uncertain_generation IS NULL
                AND disposition_kind IS NULL AND params=$2 AND created_at=$3
            FROM order_intents WHERE intent_id=$1",
        )
        .bind(intent.intent_id)
        .bind(&intent.params)
        .bind(intent.created_at)
        .fetch_one(pool)
        .await?;
        ensure!(bare, "the first cancel carries an outcome or disposition");
        let attempts: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM submit_attempts
                     WHERE venue_id=$1 AND account_scope=$2 AND client_order_id=$3)
                  + (SELECT count(*) FROM spot_intent_clock WHERE intent_id=$4)",
        )
        .bind(&intent.venue_id)
        .bind(&intent.account_scope)
        .bind(&intent.client_order_id)
        .bind(intent.intent_id)
        .fetch_one(pool)
        .await?;
        assert_eq!(attempts, 0);
        let handed_off: bool = sqlx::query_scalar(
            "SELECT cohort_month IS NOT NULL OR gross_qty IS NOT NULL OR closed_at IS NOT NULL
            FROM balance_legs WHERE position_id=$1",
        )
        .bind(self.position)
        .fetch_one(pool)
        .await?;
        ensure!(!handed_off, "the leg moved past admission");
        Ok(())
    }
}

async fn transition_snapshot(pool: &PgPool, transition: Uuid) -> Result<TransitionSnapshot> {
    Ok(sqlx::query_as(
        "SELECT kind,target_trigger::text,reason,first_cancel_intent_id,original_target_order_id,
            started_at,completed_at IS NULL AND outcome IS NULL
        FROM balance_transitions WHERE transition_id=$1",
    )
    .bind(transition)
    .fetch_one(pool)
    .await?)
}

fn trigger(text: &str) -> Result<Decimal> {
    Ok(text.parse()?)
}

fn committed(result: Result<StoredFirstCancel, CommitFailure>) -> Result<StoredFirstCancel> {
    result.map_err(|failure| anyhow!("first cancel refused: {failure:?}"))
}

pub(super) async fn migrate_fixture(pool: &PgPool) -> Result<()> {
    let options = pool.connect_options();
    let mut addresses = tokio::net::lookup_host((options.get_host(), options.get_port())).await?;
    ensure!(
        addresses.next().is_some_and(|address| address.ip().is_loopback()),
        "expected a local test database"
    );
    robson_db::migrate(pool).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_positive_new_transition(pool: PgPool) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let version = leg.accounting_version(&pool).await?;
    let intent = leg.cancel_intent();
    let transition = Uuid::now_v7();
    let stored = committed(
        leg.backend
            .commit_first_cancel(&intent, TransitionWrite {
                symbol: SYMBOL,
                kind: TransitionKind::Replace,
                target_trigger: Decimal::new(95, 0),
                reason: "positive-new",
                expected_accounting_version: version,
                expected_transition: ExpectedTransition::New(transition),
            })
            .await,
    )?;
    assert_eq!(
        (stored.transition_id, stored.intent_id, stored.accounting_version),
        (transition, intent.intent_id, version + 1)
    );
    assert_eq!(leg.accounting_version(&pool).await?, version + 1);
    assert_eq!(leg.transitions(&pool).await?, [transition]);
    let (kind, target, reason, first, original, _, open) =
        transition_snapshot(&pool, transition).await?;
    assert_eq!(
        (
            kind.as_str(),
            trigger(&target)?,
            reason.as_str(),
            first,
            original.as_deref(),
            open
        ),
        (
            "replace",
            Decimal::new(95, 0),
            "positive-new",
            Some(intent.intent_id),
            Some(ORIGINAL),
            true
        )
    );
    leg.assert_bare_cancel(&pool, &intent).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_positive_existing_open_transition(pool: PgPool) -> Result<()> {
    migrate_fixture(&pool).await?;
    // (requested kind, trigger, reason) -> (stored kind, trigger, reason):
    // the equal kind keeps its reason and raises the target; the stronger kind
    // takes its reason and keeps the higher stored target.
    let cases = [
        ((TransitionKind::Reprotect, 97, "equal-rank"), ("replace", 97, "existing-open")),
        (
            (TransitionKind::Flatten, 93, "upgrade-flatten"),
            ("flatten", 95, "upgrade-flatten"),
        ),
    ];
    for ((kind, requested, reason), (want_kind, want_target, want_reason)) in cases {
        let leg = PositiveLeg::admit(&pool).await?;
        let transition = leg.open_transition(&pool, "replace", 95, "existing-open").await?;
        let (.., started, _) = transition_snapshot(&pool, transition).await?;
        let version = leg.accounting_version(&pool).await?;
        let intent = leg.cancel_intent();
        let stored = committed(
            leg.backend
                .commit_first_cancel(&intent, TransitionWrite {
                    symbol: SYMBOL,
                    kind,
                    target_trigger: Decimal::new(requested, 0),
                    reason,
                    expected_accounting_version: version,
                    expected_transition: ExpectedTransition::ExistingOpen(transition),
                })
                .await,
        )?;
        assert_eq!(
            (stored.transition_id, stored.intent_id, stored.accounting_version),
            (transition, intent.intent_id, version + 1)
        );
        assert_eq!(leg.accounting_version(&pool).await?, version + 1);
        assert_eq!(leg.transitions(&pool).await?, [transition]);
        let (kind, target, reason, first, original, saved_start, open) =
            transition_snapshot(&pool, transition).await?;
        assert_eq!(
            (
                kind.as_str(),
                trigger(&target)?,
                reason.as_str(),
                first,
                original.as_deref(),
                saved_start,
                open
            ),
            (
                want_kind,
                Decimal::new(want_target, 0),
                want_reason,
                Some(intent.intent_id),
                Some(ORIGINAL),
                started,
                true
            )
        );
        leg.assert_bare_cancel(&pool, &intent).await?;
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn first_cancel_positive_exact_replay_is_id_spent(pool: PgPool) -> Result<()> {
    migrate_fixture(&pool).await?;
    let leg = PositiveLeg::admit(&pool).await?;
    let version = leg.accounting_version(&pool).await?;
    let intent = leg.cancel_intent();
    let transition = Uuid::now_v7();
    let write = |expected_accounting_version| TransitionWrite {
        symbol: SYMBOL,
        kind: TransitionKind::Replace,
        target_trigger: Decimal::new(95, 0),
        reason: "positive-replay",
        expected_accounting_version,
        expected_transition: ExpectedTransition::New(transition),
    };
    committed(leg.backend.commit_first_cancel(&intent, write(version)).await)?;
    let current = leg.accounting_version(&pool).await?;
    assert_eq!(current, version + 1);
    let before = transition_snapshot(&pool, transition).await?;
    match leg.backend.commit_first_cancel(&intent, write(current)).await {
        Err(CommitFailure::IdSpent) => {},
        Err(failure) => bail!("replay refused as {failure:?}, not IdSpent"),
        Ok(_) => bail!("the exact replay committed again"),
    }
    assert_eq!(leg.accounting_version(&pool).await?, current);
    assert_eq!(leg.transitions(&pool).await?, [transition]);
    assert_eq!(transition_snapshot(&pool, transition).await?, before);
    assert_eq!((before.3, before.4.as_deref()), (Some(intent.intent_id), Some(ORIGINAL)));
    leg.assert_bare_cancel(&pool, &intent).await
}

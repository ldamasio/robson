//! Private local R1/R3 evaluation of one initialized Spot leg and its exact
//! candidate stop, from the one initialized read. The facts are versioned and
//! descriptive: no send, cancel, activation or settlement decision, no public
//! constructor and no runtime caller. Live consumers still need complete
//! startup ownership, the pending-evidence gate and generation/disarm around
//! the first transport poll.
use std::marker::PhantomData;

use anyhow::{anyhow, ensure, Result};
use chrono::{DateTime, Utc};
use robson_exec::{
    balance_liability::{
        order_facts, partition_facts, FactError, FeeEvidence, RecordedPartition, Remainder,
    },
    order_journal::{IntentKind, COMMIT_BOUND},
    venue::{OrderPhase, VenueId},
    venue_ledger::{
        LedgerError, LegOrderKind, OrderKey, OrderLink, OrderOwner, StoredOrder, VenueIntId,
    },
};
use robson_testkit::db_proxy::ProxyMode;
use rust_decimal::Decimal;
use sqlx::{postgres::PgRow, FromRow, PgPool};
use uuid::Uuid;

use super::{
    assert_trace, fixture, journal_batch, order,
    ready::{
        initialize, relation_nodes, statement, Keys, ProjectionReady, ReplayOrder,
        CANDIDATE_COLUMNS, CANDIDATE_JOINS,
    },
    ProtectedLeg, Wire, READ, STOP, SYMBOL, VENUE,
};
use crate::{
    order_journal_store::PgJournalBackend, venue_clock_store::codec::ORDER_COLUMNS,
    venue_ledger_store::decode_order_row,
};

/// The accepted order-set aggregates without `hot.*`, so that the exact
/// candidate's full venue row keeps its own column names for `decode_order`.
/// Lifecycle, route and admission are read by their primary keys.
const POLICY_COLUMNS: &str = "g.generation, $1::uuid AS read_tenant,
    hot.position_id AS leg_position, hot.accounting_version, hot.gross_qty,
    hot.base_commission, hot.base_commission_final, hot.executable_qty, hot.consumed_cum,
    hot.target_trigger, hot.leg_sell, hot.leg_sell_conflicts, hot.symbol_sell,
    hot.symbol_sell_conflicts, hot.grouped_cancel, hot.grouped_cancel_conflicts,
    hot.ungroupable_cancel, hot.ungroupable_cancel_conflicts, hot.unbound_cancel,
    hot.unbound_cancel_conflicts, hot.exact_stop_identity,
    coalesce(u.potential_sell, 0::bigint) AS unbound_sell,
    coalesce(u.conflict_count, 0::bigint) AS unbound_sell_conflicts,
    tj.intent_id AS stop_journal, tj.kind AS stop_journal_kind,
    bl.closed_at AS leg_closed_at, bl.recovery_state AS leg_recovery,
    (lr.tenant_id,lr.venue_id,lr.account_scope)=($1,$2,$3) AS route_exact,
    (ba.position_id,ba.tenant_id,ba.venue_id,ba.account_scope,ba.symbol)=($4,$1,$2,$3,$5)
      AS admission_exact";
const POLICY_JOINS: &str = "
    LEFT JOIN venue_orders o
      ON (o.venue_id,o.account_scope,o.symbol,o.exchange_order_id)=($2,$3,$5,$6::text)
    LEFT JOIN order_intents tj ON tj.intent_id=o.intent_id
    LEFT JOIN balance_legs bl ON bl.position_id=$4
    LEFT JOIN leg_routes lr ON lr.position_id=$4
    LEFT JOIN balance_leg_admissions ba ON ba.entry_intent_id=bl.admission_intent_id";

fn policy_statement() -> String {
    statement(
        &format!("{POLICY_COLUMNS}, {CANDIDATE_COLUMNS}, {ORDER_COLUMNS}"),
        &format!("{POLICY_JOINS} {CANDIDATE_JOINS}"),
    )
}

#[derive(sqlx::FromRow)]
struct PolicyColumns {
    generation: Uuid,
    read_tenant: Uuid,
    leg_position: Uuid,
    accounting_version: i64,
    gross_qty: Option<Decimal>,
    base_commission: Option<Decimal>,
    base_commission_final: bool,
    executable_qty: Option<Decimal>,
    consumed_cum: Decimal,
    target_trigger: Decimal,
    leg_sell: i64,
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
    exact_stop_identity: bool,
    stop_journal: Option<Uuid>,
    stop_journal_kind: Option<String>,
    leg_closed_at: Option<DateTime<Utc>>,
    leg_recovery: Option<String>,
    route_exact: Option<bool>,
    admission_exact: Option<bool>,
    candidate_scope: Option<String>,
    candidate_position: Option<Uuid>,
    candidate_potential: Option<bool>,
    candidate_conflict: Option<bool>,
    target_cancel_pending: Option<i64>,
    target_cancel_conflicts: Option<i64>,
    target_cancel_terminal: Option<bool>,
}

/// One completed row. The candidate is decoded from the same row by the
/// ledger's own decoder against the tenant this read was bound to; a decode
/// failure stays a typed value instead of failing the read.
struct PolicyRead {
    columns: PolicyColumns,
    candidate: Result<Option<StoredOrder>, LedgerError>,
}

impl<'r> FromRow<'r, PgRow> for PolicyRead {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        let columns = PolicyColumns::from_row(row)?;
        let candidate = decode_order_row(row, columns.read_tenant);
        Ok(Self { columns, candidate })
    }
}

/// No sealed snapshot exists; any partial data was discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoSnapshot {
    Input,
    Read,
    Uninitialized,
}

/// A liability other than the exact candidate that holds a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    CandidateStop,
    LegSell,
    LegSellConflict,
    Symbol,
    AccountSell,
    LegCancel,
    TargetCancel,
    CancelConflict,
    UngroupableCancel,
    UnboundCancel,
}

/// Descriptive R1 facts at the captured version only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum R1 {
    Clear,
    Held(Hold),
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Membership {
    ExactLive,
    NotMember,
    Contradictory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Closed,
    Recovery,
    Route,
    Admission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unresolved {
    Lifecycle(Lifecycle),
    Candidate,
    Malformed,
    Identity,
    Phase,
    Quantity,
    Membership,
    CancelGroup,
}

/// Local R3 evidence. `Protected` is a classification, not a permit.
#[derive(Debug, Clone, PartialEq, Eq)]
enum R3 {
    Protected {
        remainder: Decimal,
        trigger: Decimal,
        fee: FeeEvidence,
    },
    TargetCancel,
    Held(Hold),
    Executing,
    Pending,
    Terminal {
        status: String,
        executed: Decimal,
    },
    ShapeMismatch,
    Deficit {
        excess: Decimal,
    },
    Unresolved(Unresolved),
}

/// Not Default or Deserialize; built only by `evaluate` from a completed read.
#[derive(Debug, Clone, PartialEq)]
struct LocalR1R3<'owner> {
    generation_owner: PhantomData<&'owner ProjectionReady>,
    generation: Uuid,
    accounting_version: i64,
    tenant: Uuid,
    position: Uuid,
    key: OrderKey,
    owner: Option<OrderOwner>,
    leg_sell: i64,
    member: Membership,
    r1: R1,
    r3: R3,
}

/// One statement, no writes, locks, timers beyond the read bound or venue
/// requests.
async fn evaluate<'owner>(
    ready: &'owner ProjectionReady,
    backend: &PgJournalBackend,
    account: &str,
    position: Uuid,
    symbol: &str,
    original: &str,
) -> Result<LocalR1R3<'owner>, NoSnapshot> {
    let exchange_order_id: VenueIntId = original.parse().map_err(|_| NoSnapshot::Input)?;
    if exchange_order_id.to_string() != original
        || account.trim().is_empty()
        || symbol.trim().is_empty()
        || position.is_nil()
    {
        return Err(NoSnapshot::Input);
    }
    let keys = Keys {
        tenant: backend.tenant_id,
        venue: VENUE,
        account,
        position,
        symbol,
    };
    let sql = policy_statement();
    let read = ready
        .query_as::<PolicyRead>(&sql, keys, Some(original))
        .fetch_optional(&backend.pool);
    let row = tokio::time::timeout(COMMIT_BOUND, read)
        .await
        .map_err(|_| NoSnapshot::Read)?
        .map_err(|_| NoSnapshot::Read)?
        .ok_or(NoSnapshot::Uninitialized)?;
    let columns = &row.columns;
    if columns.generation != ready.generation()
        || columns.read_tenant != backend.tenant_id
        || columns.leg_position != position
    {
        return Err(NoSnapshot::Uninitialized);
    }
    let key = OrderKey {
        venue: VenueId::new(VENUE),
        account_scope: account.to_owned(),
        symbol: symbol.to_owned(),
        exchange_order_id,
    };
    let owner = columns
        .stop_journal
        .filter(|_| columns.stop_journal_kind.as_deref() == Some("protection"))
        .map(|intent_id| OrderOwner::Leg {
            position_id: position,
            intent_id,
            kind: LegOrderKind::Stop,
        });
    let member = membership(columns, position);
    let other = other_liabilities(columns, member);
    let r3 = classify(&row, &key, owner.as_ref(), member, other);
    let r1 = liability_with_candidate(other, &r3);
    Ok(LocalR1R3 {
        generation_owner: PhantomData,
        generation: columns.generation,
        accounting_version: columns.accounting_version,
        tenant: backend.tenant_id,
        position,
        key,
        owner,
        leg_sell: columns.leg_sell,
        member,
        r1,
        r3,
    })
}

/// The candidate's own keyed member. An aggregate alone never identifies it.
fn membership(columns: &PolicyColumns, position: Uuid) -> Membership {
    match (
        columns.candidate_scope.as_deref(),
        columns.candidate_position,
        columns.candidate_potential,
        columns.candidate_conflict,
    ) {
        (Some("leg"), Some(owner), Some(true), Some(false)) if owner == position => {
            Membership::ExactLive
        },
        (None, None, None, None) | (Some("none"), None, Some(false), Some(false)) => {
            Membership::NotMember
        },
        _ => Membership::Contradictory,
    }
}

fn target_cancel_active(columns: &PolicyColumns) -> Option<i64> {
    match (
        columns.target_cancel_pending,
        columns.target_cancel_conflicts,
        columns.target_cancel_terminal,
    ) {
        (None, None, None) | (Some(0..), Some(0), Some(true)) => Some(0),
        (Some(pending @ 0..), Some(0), Some(false)) => Some(pending),
        _ => None,
    }
}

fn other_liabilities(columns: &PolicyColumns, member: Membership) -> R1 {
    let own = match member {
        Membership::ExactLive => 1,
        Membership::NotMember => 0,
        Membership::Contradictory => return R1::Unresolved,
    };
    let Some(target_active) = target_cancel_active(columns) else {
        return R1::Unresolved;
    };
    if columns.leg_sell < own || columns.grouped_cancel < target_active {
        return R1::Unresolved;
    }
    [
        (columns.leg_sell_conflicts > 0, Hold::LegSellConflict),
        (columns.leg_sell > own, Hold::LegSell),
        (columns.symbol_sell > 0 || columns.symbol_sell_conflicts > 0, Hold::Symbol),
        (
            columns.unbound_sell > 0 || columns.unbound_sell_conflicts > 0,
            Hold::AccountSell,
        ),
        (columns.grouped_cancel_conflicts > 0, Hold::CancelConflict),
        (columns.grouped_cancel > target_active, Hold::LegCancel),
        (
            columns.ungroupable_cancel > 0 || columns.ungroupable_cancel_conflicts > 0,
            Hold::UngroupableCancel,
        ),
        (
            columns.unbound_cancel > 0 || columns.unbound_cancel_conflicts > 0,
            Hold::UnboundCancel,
        ),
    ]
    .into_iter()
    .find_map(|(held, hold)| held.then_some(R1::Held(hold)))
    .unwrap_or(R1::Clear)
}

/// The candidate stays a liability unless its own valid evidence discharges
/// it or qualifies it for the protective-stop exception.
fn liability_with_candidate(other: R1, candidate: &R3) -> R1 {
    if other != R1::Clear {
        return other;
    }
    match candidate {
        R3::Protected { .. } | R3::Terminal { .. } => R1::Clear,
        R3::TargetCancel => R1::Held(Hold::TargetCancel),
        R3::Executing | R3::Pending | R3::ShapeMismatch => R1::Held(Hold::CandidateStop),
        R3::Held(hold) => R1::Held(*hold),
        R3::Deficit { .. } | R3::Unresolved(_) => R1::Unresolved,
    }
}

fn classify(
    row: &PolicyRead,
    key: &OrderKey,
    owner: Option<&OrderOwner>,
    member: Membership,
    other: R1,
) -> R3 {
    use Unresolved as U;
    let columns = &row.columns;
    let lifecycle = [
        (columns.leg_closed_at.is_some(), Lifecycle::Closed),
        (columns.leg_recovery.is_some(), Lifecycle::Recovery),
        (columns.route_exact != Some(true), Lifecycle::Route),
        (columns.admission_exact != Some(true), Lifecycle::Admission),
    ];
    if let Some((_, lifecycle)) = lifecycle.into_iter().find(|(failed, _)| *failed) {
        return R3::Unresolved(U::Lifecycle(lifecycle));
    }
    // Absence or corrupt linkage is never terminality or a missing stop.
    let order = match &row.candidate {
        Ok(Some(order)) => order,
        Ok(None) => return R3::Unresolved(U::Candidate),
        Err(_) => return R3::Unresolved(U::Malformed),
    };
    let Some(owner) = owner.filter(|_| columns.exact_stop_identity) else {
        return R3::Unresolved(U::Identity);
    };
    let facts = match order_facts(order, key, owner) {
        Ok(facts) => facts,
        Err(FactError::IdentityMismatch) => return R3::Unresolved(U::Identity),
        Err(_) => return R3::Unresolved(U::Malformed),
    };
    if order.phase == OrderPhase::Unknown
        || OrderPhase::from_binance_spot_status(&order.status) != order.phase
    {
        return R3::Unresolved(U::Phase);
    }
    let Some(target_active) = target_cancel_active(columns) else {
        return R3::Unresolved(U::CancelGroup);
    };
    if order.phase == OrderPhase::Terminal {
        if member != Membership::NotMember {
            return R3::Unresolved(U::Membership);
        }
        if target_active > 0 {
            return R3::TargetCancel;
        }
        return R3::Terminal {
            status: order.status.clone(),
            executed: order.executed_cum,
        };
    }
    let (Some(gross), Some(base_fee), Some(owned)) =
        (columns.gross_qty, columns.base_commission, columns.executable_qty)
    else {
        return R3::Unresolved(U::Quantity);
    };
    let fee = if columns.base_commission_final {
        FeeEvidence::FinalCommission
    } else {
        FeeEvidence::ProvisionalBound
    };
    let Ok(partition) = partition_facts(RecordedPartition {
        gross,
        base_fee,
        owned,
        consumed: columns.consumed_cum,
        fee_evidence: fee,
    }) else {
        return R3::Unresolved(U::Quantity);
    };
    let remainder = match partition.remainder() {
        Remainder::Quantity(remainder) => remainder,
        Remainder::Deficit { excess } => return R3::Deficit { excess },
    };
    if member != Membership::ExactLive {
        return R3::Unresolved(U::Membership);
    }
    if columns.target_cancel_terminal == Some(true) {
        return R3::Unresolved(U::CancelGroup);
    }
    if target_active > 0 {
        return R3::TargetCancel;
    }
    // Only the three statuses the existing parser already maps to Live.
    match order.status.as_str() {
        "PARTIALLY_FILLED" => return R3::Executing,
        "PENDING_NEW" => return R3::Pending,
        "NEW" => {},
        _ => return R3::Unresolved(U::Phase),
    }
    match other {
        R1::Clear => {},
        R1::Held(hold) => return R3::Held(hold),
        R1::Unresolved => return R3::Unresolved(U::Membership),
    }
    if remainder <= Decimal::ZERO {
        return R3::ShapeMismatch;
    }
    match (
        facts.matches_protection_shape(remainder, columns.target_trigger),
        order.stop_price,
    ) {
        (Ok(true), Some(trigger)) => R3::Protected { remainder, trigger, fee },
        (Ok(_), _) => R3::ShapeMismatch,
        (Err(_), _) => R3::Unresolved(U::Quantity),
    }
}

/// Recorded Q/B/X for a fixture leg under its accounting row, before any
/// read. No canonical entry-partition writer exists in this scope; the
/// evaluator only reads these values and never derives or changes X.
async fn record_partition(
    pool: &PgPool,
    leg: &ProtectedLeg,
    owned: Decimal,
    final_fee: bool,
    closed: bool,
) -> Result<()> {
    let fee = Decimal::new(1, 2);
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2")
        .bind(leg.tenant)
        .bind(VENUE)
        .execute(&mut *tx)
        .await?;
    let updated = sqlx::query(
        "UPDATE balance_legs SET gross_qty=$2, acquisition_cost=$2*100, base_commission=$3,
         base_commission_final=$4, executable_qty=$5,
         cohort_month=CASE WHEN $6 THEN decided_month ELSE cohort_month END,
         closed_at=CASE WHEN $6 THEN now() END
         WHERE (position_id,tenant_id,venue_id)=($1,$7,$8)",
    )
    .bind(leg.position)
    .bind(owned + fee)
    .bind(fee)
    .bind(final_fee)
    .bind(owned)
    .bind(closed)
    .bind(leg.tenant)
    .bind(VENUE)
    .execute(&mut *tx)
    .await?;
    ensure!(updated.rows_affected() == 1, "fixture leg partition");
    tx.commit().await?;
    Ok(())
}

async fn observe<'owner>(
    ready: &'owner ProjectionReady,
    pool: &PgPool,
    leg: &ProtectedLeg,
) -> Result<LocalR1R3<'owner>> {
    observe_as(ready, pool, leg, leg.position, &leg.stop).await
}

/// Through a traced single-connection owner: exactly one statement.
async fn observe_as<'owner>(
    ready: &'owner ProjectionReady,
    pool: &PgPool,
    leg: &ProtectedLeg,
    position: Uuid,
    original: &str,
) -> Result<LocalR1R3<'owner>> {
    let wire = Wire::new(pool, leg.tenant).await?;
    let facts = evaluate(ready, &wire.store, &leg.account, position, SYMBOL, original)
        .await
        .map_err(|refusal| anyhow!("no local R1/R3 snapshot: {refusal:?}"))?;
    assert_trace(&wire, &[READ]);
    Ok(facts)
}

fn protected(remainder: i64, fee: FeeEvidence) -> R3 {
    R3::Protected {
        remainder: Decimal::from(remainder),
        trigger: Decimal::from(90),
        fee,
    }
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn local_r1_r3_classifies_initialized_order_set_cells(pool: PgPool) -> Result<()> {
    fixture(&pool).await?;
    let ten = Decimal::from(10);
    // A (own unresolved exit) and B share tenant, account and symbol.
    let a = ProtectedLeg::admit(&pool).await?;
    let b = ProtectedLeg::admit_stop(&pool, Some(&a), "4100002", "NEW", Decimal::ZERO).await?;
    a.commit(&journal_batch(a.intent(IntentKind::Exit, None))).await?;
    let cancelled = ProtectedLeg::admit(&pool).await?;
    cancelled.commit(&journal_batch(cancelled.cancel())).await?;
    let pending = ProtectedLeg::admit_stop(&pool, None, STOP, "PENDING_NEW", Decimal::ZERO).await?;
    let unknown =
        ProtectedLeg::admit_stop(&pool, None, STOP, "PENDING_CANCEL", Decimal::ZERO).await?;
    let executing =
        ProtectedLeg::admit_stop(&pool, None, STOP, "PARTIALLY_FILLED", Decimal::ONE).await?;
    let deficit =
        ProtectedLeg::admit_stop(&pool, None, STOP, "PARTIALLY_FILLED", Decimal::ONE).await?;
    let terminal = ProtectedLeg::admit(&pool).await?;
    let terminal_fill =
        ProtectedLeg::admit_stop(&pool, None, STOP, "CANCELED", Decimal::ONE).await?;
    let unrecorded = ProtectedLeg::admit(&pool).await?;
    let short = ProtectedLeg::admit(&pool).await?;
    let symbol = ProtectedLeg::admit(&pool).await?;
    symbol.canonical_mutation(2, 0).await?;
    let closed = ProtectedLeg::admit(&pool).await?;
    let provisional = ProtectedLeg::admit(&pool).await?;
    let miscounted = ProtectedLeg::admit(&pool).await?;
    miscounted
        .commit(&journal_batch(miscounted.intent(IntentKind::Exit, None)))
        .await?;
    let contradicted = ProtectedLeg::admit(&pool).await?;
    for leg in [
        &a,
        &b,
        &cancelled,
        &pending,
        &unknown,
        &executing,
        &terminal,
        &terminal_fill,
        &symbol,
        &miscounted,
        &contradicted,
    ] {
        record_partition(&pool, leg, ten, true, false).await?;
    }
    record_partition(&pool, &deficit, Decimal::new(5, 1), true, false).await?;
    record_partition(&pool, &short, Decimal::from(9), true, false).await?;
    record_partition(&pool, &closed, ten, true, true).await?;
    record_partition(&pool, &provisional, ten, false, false).await?;
    // Corruption control, bypassing the canonical writer as in the bridge tests.
    let affected = sqlx::query(
        "UPDATE venue_orders SET status='CANCELED'
        WHERE (tenant_id,venue_id,account_scope,symbol,exchange_order_id)=($1,$2,$3,$4,$5)
          AND phase='live' AND status='NEW'",
    )
    .bind(contradicted.tenant)
    .bind(VENUE)
    .bind(&contradicted.account)
    .bind(SYMBOL)
    .bind(STOP)
    .execute(&pool)
    .await?;
    ensure!(affected.rows_affected() == 1);

    let ready = initialize(&pool, ReplayOrder::MembersFirst, None).await?;
    terminal
        .record(
            &order(STOP, &terminal.stop_client, "CANCELED", OrderPhase::Terminal),
            &terminal.stop_link,
            2,
        )
        .await?;
    // Corruption control: remove the own stop's member and its unit, so the
    // one remaining leg count is the exit journal, not the candidate.
    sqlx::query(
        "DELETE FROM sell_shadow_orders
        WHERE (tenant_id,venue_id,account_scope,symbol,exchange_order_id)=($1,$2,$3,$4,$5)",
    )
    .bind(miscounted.tenant)
    .bind(VENUE)
    .bind(&miscounted.account)
    .bind(SYMBOL)
    .bind(STOP)
    .execute(&pool)
    .await?;
    sqlx::query("SELECT sell_shadow_bump('leg',$1,$2,$3,$4,$5,-1,0)")
        .bind(miscounted.tenant)
        .bind(VENUE)
        .bind(&miscounted.account)
        .bind(SYMBOL)
        .bind(miscounted.position)
        .execute(&pool)
        .await?;

    // Independently reasoned from the source rows written above.
    use Unresolved as U;
    let final_fee = FeeEvidence::FinalCommission;
    let cells = [
        // Own NEW STOP_LOSS, Q 10.01 B 0.01 X 10 consumed 0: G 10, trigger 90.
        ("leg_b_positive", &b, R1::Clear, protected(10, final_fee)),
        // Exact own stop plus own unresolved exit: two leg members, one other.
        ("leg_a_unresolved_exit", &a, R1::Held(Hold::LegSell), R3::Held(Hold::LegSell)),
        // The only cancel targets the candidate itself.
        ("target_cancel", &cancelled, R1::Held(Hold::TargetCancel), R3::TargetCancel),
        ("pending_new", &pending, R1::Held(Hold::CandidateStop), R3::Pending),
        ("partially_filled", &executing, R1::Held(Hold::CandidateStop), R3::Executing),
        ("unknown_status", &unknown, R1::Unresolved, R3::Unresolved(U::Phase)),
        // consumed 1 from the native fill exceeds X 0.5: no G exists.
        ("deficit", &deficit, R1::Unresolved, R3::Deficit { excess: Decimal::new(5, 1) }),
        // Discharged member; terminal status is descriptive only.
        (
            "terminal",
            &terminal,
            R1::Clear,
            R3::Terminal {
                status: "CANCELED".into(),
                executed: Decimal::ZERO,
            },
        ),
        (
            "terminal_with_execution",
            &terminal_fill,
            R1::Clear,
            R3::Terminal {
                status: "CANCELED".into(),
                executed: Decimal::ONE,
            },
        ),
        (
            "unrecorded_quantities",
            &unrecorded,
            R1::Unresolved,
            R3::Unresolved(U::Quantity),
        ),
        // G 9 differs from the stop's remaining 10.
        ("short_remainder", &short, R1::Held(Hold::CandidateStop), R3::ShapeMismatch),
        // A discovered live SELL without a leg in the same account and symbol.
        ("symbol_sell", &symbol, R1::Held(Hold::Symbol), R3::Held(Hold::Symbol)),
        (
            "closed_leg",
            &closed,
            R1::Unresolved,
            R3::Unresolved(U::Lifecycle(Lifecycle::Closed)),
        ),
        (
            "provisional_fee",
            &provisional,
            R1::Clear,
            protected(10, FeeEvidence::ProvisionalBound),
        ),
        // Aggregate of one, but the counted member is the exit journal.
        (
            "wrong_one_in_count",
            &miscounted,
            R1::Held(Hold::LegSell),
            R3::Unresolved(U::Membership),
        ),
        // A live phase with a terminal status.
        ("contradicted_phase", &contradicted, R1::Unresolved, R3::Unresolved(U::Phase)),
    ];
    let cell_count = cells.len() + 1;
    for (name, leg, r1, r3) in cells {
        let facts = observe(&ready, &pool, leg).await?;
        assert_eq!((facts.r1, &facts.r3), (r1, &r3), "{name}");
        assert_eq!(facts.generation, ready.generation());
        assert_eq!(facts.accounting_version, leg.version(&pool).await?, "{name}");
        assert_eq!((facts.tenant, facts.position), (leg.tenant, leg.position));
        assert_eq!(facts.key.exchange_order_id.to_string(), leg.stop);
        if name == "wrong_one_in_count" {
            assert_eq!((facts.leg_sell, facts.member), (1, Membership::NotMember));
        }
        if name == "leg_b_positive" {
            assert_eq!(facts.owner, Some(leg.stop_link.owner.clone()));
            assert_eq!((facts.leg_sell, facts.member), (1, Membership::ExactLive));
        }
    }

    // Leg A's exact stop is not B's candidate, even in B's account and symbol.
    let foreign_stop = observe_as(&ready, &pool, &b, b.position, &a.stop).await?;
    assert_eq!(foreign_stop.r3, R3::Unresolved(U::Identity));
    assert_ne!(foreign_stop.r1, R1::Clear);
    eprintln!("SPOT_LOCAL_R1_R3 cells={cell_count} hot_sql=1 positive=leg_b wrong_one_in_count=unresolved");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn local_r1_r3_snapshot_is_bound_to_its_read_and_version(pool: PgPool) -> Result<()> {
    fixture(&pool).await?;
    let leg = ProtectedLeg::admit(&pool).await?;
    let foreign = ProtectedLeg::admit(&pool).await?;
    record_partition(&pool, &leg, Decimal::from(10), true, false).await?;
    let ready = initialize(&pool, ReplayOrder::TargetsFirst, None).await?;

    let first = observe(&ready, &pool, &leg).await?;
    assert_eq!(first.r3, protected(10, FeeEvidence::FinalCommission));
    let kept = first.clone();
    // A canonical source write moves the version; old facts keep theirs.
    leg.canonical_mutation(0, 0).await?;
    let second = observe(&ready, &pool, &leg).await?;
    assert!(second.accounting_version > first.accounting_version);
    assert_eq!(second.r3, R3::Held(Hold::LegSell));
    assert_eq!(first, kept);

    // Another accounting owner's backend and an uninitialized generation
    // produce no snapshot, never zeroes.
    let wire = Wire::new(&pool, foreign.tenant).await?;
    let refused =
        evaluate(&ready, &wire.store, &leg.account, leg.position, SYMBOL, &leg.stop).await;
    assert_eq!(refused, Err(NoSnapshot::Uninitialized));
    assert_trace(&wire, &[READ]);
    let sql = policy_statement();
    let keys = Keys {
        tenant: leg.tenant,
        venue: VENUE,
        account: &leg.account,
        position: leg.position,
        symbol: SYMBOL,
    };
    let absent = ready
        .foreign_query_as::<PolicyRead>(&sql, keys, Some(STOP))
        .fetch_optional(&pool)
        .await?;
    ensure!(absent.is_none(), "an uninitialized generation produced policy facts");
    for malformed in ["04100001", "", "-1"] {
        let wire = Wire::new(&pool, leg.tenant).await?;
        let refused =
            evaluate(&ready, &wire.store, &leg.account, leg.position, SYMBOL, malformed).await;
        assert_eq!(refused, Err(NoSnapshot::Input));
        assert_trace(&wire, &[]);
    }

    // A read whose completion is lost seals nothing; its owner retires.
    let lost = Wire::new(&pool, leg.tenant).await?;
    lost.proxy
        .set_mode(ProxyMode::DropAutocommitAckOf("spot_shadow_initialization"));
    let refused =
        evaluate(&ready, &lost.store, &leg.account, leg.position, SYMBOL, &leg.stop).await;
    assert_eq!(refused, Err(NoSnapshot::Read));
    assert_eq!(lost.proxy.autocommit_acks_dropped(), 1);
    assert_trace(&lost, &[READ]);
    lost.retires(&pool).await?;
    let after = observe(&ready, &pool, &leg).await?;
    assert_eq!(after, second);
    eprintln!("SPOT_LOCAL_R1_R3 stale_version=kept foreign_owner=refused lost_read=no_snapshot");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn local_r1_r3_typed_read_stays_keyed_at_history_and_membership_scale(
    pool: PgPool,
) -> Result<()> {
    fixture(&pool).await?;
    let leg = ProtectedLeg::admit(&pool).await?;
    let unrelated = ProtectedLeg::admit(&pool).await?;
    record_partition(&pool, &leg, Decimal::from(10), true, false).await?;
    for index in 0..512 {
        let target = (100_000 + index).to_string();
        let exit = leg.intent(IntentKind::Exit, None);
        leg.commit(&journal_batch(exit.clone())).await?;
        let link = OrderLink {
            venue: VenueId::new(VENUE),
            account_scope: leg.account.clone(),
            owner: OrderOwner::Leg {
                intent_id: exit.intent_id,
                position_id: leg.position,
                kind: LegOrderKind::Sell,
            },
            client_order_id: exit.client_order_id,
        };
        let mut terminal = order(&target, &link.client_order_id, "CANCELED", OrderPhase::Terminal);
        terminal.order_type = "MARKET".into();
        terminal.protection_kind = None;
        terminal.stop_price = None;
        leg.record(&terminal, &link, 20 + i64::try_from(index)?).await?;
        let cancel = leg.intent(IntentKind::Cancel, Some(&target));
        leg.commit(&journal_batch(cancel)).await?;
        unrelated.canonical_mutation(2, index).await?;
    }
    let ready = initialize(&pool, ReplayOrder::TargetsFirst, None).await?;
    sqlx::raw_sql("ANALYZE venue_orders, order_intents, sell_shadow_orders, cancel_shadow_groups")
        .execute(&pool)
        .await?;
    let (venue_rows, journal_rows, member_rows, cancel_groups): (i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT (SELECT count(*) FROM venue_orders),
                    (SELECT count(*) FROM order_intents),
                    (SELECT count(*) FROM sell_shadow_orders),
                    (SELECT count(*) FROM cancel_shadow_groups)",
        )
        .fetch_one(&pool)
        .await?;
    ensure!(
        venue_rows >= 1024 && journal_rows >= 1024 && member_rows >= 512 && cancel_groups >= 512
    );
    let sql = policy_statement();
    let keys = Keys {
        tenant: leg.tenant,
        venue: VENUE,
        account: &leg.account,
        position: leg.position,
        symbol: SYMBOL,
    };
    for (candidate, present) in [(STOP, true), ("92000001", false)] {
        let wire = Wire::new(&pool, leg.tenant).await?;
        let observed = evaluate(&ready, &wire.store, &leg.account, leg.position, SYMBOL, candidate)
            .await
            .map_err(|error| anyhow!("dense policy snapshot: {error:?}"))?;
        assert_trace(&wire, &[READ]);
        assert_eq!(wire.proxy.query_trace(), [sql.clone()]);
        if present {
            assert_eq!(observed.r3, protected(10, FeeEvidence::FinalCommission));
        } else {
            assert_eq!(observed.r3, R3::Unresolved(Unresolved::Candidate));
            assert_ne!(observed.r1, R1::Clear);
        }
        let physical = ready.explain(&pool, &sql, keys, candidate).await?;
        let plan = &physical[0]["Plan"];
        ensure!(plan["Actual Rows"].as_u64() == Some(1));
        for (relation, expected_probes) in [
            ("venue_orders", 2),
            ("order_intents", 2),
            ("sell_shadow_orders", 1),
            ("cancel_shadow_groups", 1),
        ] {
            let mut probes = Vec::new();
            relation_nodes(plan, relation, &mut probes);
            ensure!(!probes.is_empty() && probes.len() <= expected_probes, "{relation}: {plan}");
            for node in probes {
                let loops =
                    node["Actual Loops"].as_u64().ok_or_else(|| anyhow!("missing loops"))?;
                let rows = node["Actual Rows"].as_u64().ok_or_else(|| anyhow!("missing rows"))?;
                ensure!(loops <= 1 && rows <= 1, "{relation}: {node}");
                if loops == 1 {
                    ensure!(node["Index Cond"].is_string(), "{relation}: {node}");
                    ensure!(node["Rows Removed by Filter"].as_u64().unwrap_or(0) == 0, "{node}");
                    ensure!(
                        node["Rows Removed by Index Recheck"].as_u64().unwrap_or(0) == 0,
                        "{node}"
                    );
                }
            }
        }
        for relation in [
            "cancel_shadow_members",
            "sell_shadow_intents",
            "submit_attempts",
        ] {
            let mut probes = Vec::new();
            relation_nodes(plan, relation, &mut probes);
            ensure!(probes.is_empty(), "historical read of {relation}: {plan}");
        }
        eprintln!(
            "SPOT_R1_R3_TYPED_WORK present={present} wire=1 venue_rows={venue_rows}
             journal_rows={journal_rows} member_rows={member_rows} cancel_groups={cancel_groups}
             plan={physical}"
        );
    }
    Ok(())
}

//! EP-1 in isolated PostgreSQL: the real record_order writer first-sets a
//! leg's entry partition once, at its terminal entry. Fixture amounts are
//! exchange metadata examples, never policy.
use std::time::Duration;

use anyhow::anyhow;
use robson_exec::{
    balance_liability::{partition_facts, FeeEvidence, RecordedPartition, Remainder},
    order_journal::{JournalBackend, OutcomeRow, VenueOutcome},
    venue_ledger::LegPartition,
};
use robson_testkit::db_proxy::DbProxy;
use rust_decimal_macros::dec;
use sqlx::{Connection as _, Row as _};

use super::{support::through_proxy, *};
use crate::order_journal_store::PgJournalBackend;

const ENTRY: u64 = 41;
const SELL: u64 = 42;

fn entry(f: &Fixture, executed: Decimal, quote: Decimal, terminal: bool) -> VenueOrderRecord {
    let orig_qty = dec!(1.00000000);
    VenueOrderRecord {
        client_order_id: f.entry.intent.client_order_id.clone(),
        side: OrderSide::Buy,
        orig_qty,
        cumulative_quote_qty: Some(quote),
        status: match (terminal, executed == orig_qty) {
            (false, _) => "PARTIALLY_FILLED",
            (true, true) => "FILLED",
            (true, false) => "EXPIRED",
        }
        .into(),
        ..f.record(
            ENTRY,
            executed,
            if terminal {
                OrderPhase::Terminal
            } else {
                OrderPhase::Live
            },
        )
    }
}

fn entry_link(f: &Fixture) -> OrderLink {
    OrderLink {
        client_order_id: f.entry.intent.client_order_id.clone(),
        ..f.link(ENTRY, OrderOwner::Leg {
            intent_id: f.entry.intent.intent_id,
            position_id: f.position,
            kind: LegOrderKind::Entry,
        })
    }
}

fn expected(gross: Decimal, cost: Decimal, base_fee: Decimal, executable: Decimal) -> LegPartition {
    LegPartition {
        gross,
        cost,
        base_fee,
        base_fee_final: false,
        executable,
    }
}

/// (partition, cohort set, consumed) read back from the actual row.
async fn leg_row(pool: &PgPool, f: &Fixture) -> Result<(Option<LegPartition>, bool, Decimal)> {
    let row = sqlx::query(
        "SELECT gross_qty,acquisition_cost,base_commission,base_commission_final,executable_qty,
                    cohort_month IS NOT NULL AS cohort,consumed_cum
             FROM balance_legs WHERE position_id=$1",
    )
    .bind(f.position)
    .fetch_one(pool)
    .await?;
    let amount = |column: &str| row.try_get::<Option<Decimal>, _>(column);
    let partition = match (
        amount("gross_qty")?,
        amount("acquisition_cost")?,
        amount("base_commission")?,
        amount("executable_qty")?,
    ) {
        (Some(gross), Some(cost), Some(base_fee), Some(executable)) => Some(LegPartition {
            gross,
            cost,
            base_fee,
            base_fee_final: row.try_get("base_commission_final")?,
            executable,
        }),
        (None, None, None, None) => None,
        _ => return Err(anyhow!("partial partition row")),
    };
    Ok((partition, row.try_get("cohort")?, row.try_get("consumed_cum")?))
}

/// The journal's handed-off admission sets the cohort before any order row.
async fn hand_off(pool: &PgPool, f: &Fixture) -> Result<()> {
    let mut batch = f.entry.clone();
    batch.outcome = Some(OutcomeRow {
        venue_outcome: Some(VenueOutcome::Success),
        ..OutcomeRow::default()
    });
    batch.balance.as_mut().context("admission batch")?.handed_off = true;
    PgJournalBackend::new(pool, f.tenant)
        .commit(&batch)
        .await
        .map_err(|error| anyhow!("handed-off admission: {error:?}"))
}

/// One write measured by the store counter and independently on the wire.
async fn measured(
    ledger: &PgVenueLedger,
    proxy: &DbProxy,
    record: &VenueOrderRecord,
    link: &OrderLink,
    seen: SeenAt,
) -> Result<(bool, u64)> {
    let (count, wire) = (ledger.statements_total(), proxy.stats().statements);
    let changed = ledger.record_order(record, link, seen).await?.changed;
    let count = ledger.statements_total() - count;
    ensure!(proxy.stats().statements - wire == count, "wire differs from counter {count}");
    Ok((changed, count))
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn entry_partition_first_set_writes_once_with_measured_statements(
    pool: PgPool,
) -> Result<()> {
    setup_tables(&pool).await?;
    let mut marker = Vec::new();
    for projected in [false, true] {
        for handed_off in [false, true] {
            for live_first in [false, true] {
                let case = format!(
                    "projected={projected} handed_off={handed_off} live_first={live_first}"
                );
                let f = Fixture::new(&pool).await;
                if handed_off {
                    hand_off(&pool, &f).await?;
                }
                ensure!(leg_row(&pool, &f).await?.1 == handed_off, "{case}: initial cohort");
                let generation = if projected {
                    Some(generation(&pool, &f).await?)
                } else {
                    None
                };
                let (proxy, proxied) = through_proxy(&pool, 1).await;
                let mut ledger = f.ledger(&proxied);
                if let Some(b) = &generation {
                    ledger = ledger.with_native_order_set_projection(b.clone());
                }
                let projection = u64::from(projected);
                let link = entry_link(&f);
                let mut counts = Vec::new();
                if live_first {
                    let version = f.version(&pool).await;
                    let live = entry(&f, dec!(0.10000000), dec!(10000.00), false);
                    let (changed, count) =
                        measured(&ledger, &proxy, &live, &link, later(1)).await?;
                    // The live write runs the leg UPDATE only to set a NULL cohort.
                    ensure!(
                        changed && count == 7 + u64::from(!handed_off) + projection,
                        "{case}: live {count}"
                    );
                    ensure!(f.version(&pool).await == version + 1);
                    ensure!(
                        leg_row(&pool, &f).await? == (None, true, Decimal::ZERO),
                        "{case}: live leg"
                    );
                    counts.push(count);
                }
                // Q as the venue prints it. Q=1 makes the dust exactly zero.
                let (gross, cost, set) = if live_first {
                    (
                        dec!(0.30000000),
                        dec!(30000.00),
                        expected(dec!(0.3), dec!(30000), dec!(0.0003), dec!(0.299)),
                    )
                } else {
                    (
                        dec!(1.00000000),
                        dec!(100000.00),
                        expected(dec!(1), dec!(100000), dec!(0.001), dec!(0.999)),
                    )
                };
                let terminal = entry(&f, gross, cost, true);
                let version = f.version(&pool).await;
                let (changed, count) =
                    measured(&ledger, &proxy, &terminal, &link, later(2)).await?;
                ensure!(changed && count == 8 + projection, "{case}: first set {count}");
                ensure!(f.version(&pool).await == version + 1, "{case}: version");
                ensure!(
                    leg_row(&pool, &f).await? == (Some(set), true, Decimal::ZERO),
                    "{case}: set"
                );
                counts.push(count);
                if let Some(b) = &generation {
                    ensure!(super::version(&pool, b).await? == version + 1);
                    exact_reference(&pool, b).await?;
                }
                // (c) The identical terminal replay is one read and no write.
                let before = snapshot(&pool).await;
                let (changed, count) =
                    measured(&ledger, &proxy, &terminal, &link, later(3)).await?;
                ensure!(!changed && count == 1, "{case}: replay {count}");
                ensure!(snapshot(&pool).await == before, "{case}: replay changed rows");
                counts.push(count);
                proxied.close().await;
                marker.push(format!(
                    "{}{}{}={}",
                    u8::from(projected),
                    u8::from(handed_off),
                    u8::from(live_first),
                    counts.iter().map(u64::to_string).collect::<Vec<_>>().join("/")
                ));
            }
        }
    }
    println!(
        "EP1_FIRST_SET cases={} counts[projected,handed_off,live_first]={}",
        marker.len(),
        marker.join(" ")
    );
    Ok(())
}

/// Seed a recorded partition directly, under the accounting row, as the
/// R1/R3 fixture does; M36 permits only this one set from NULL.
async fn seed_partition(pool: &PgPool, f: &Fixture, p: LegPartition) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id='binance:spot'")
        .bind(f.tenant)
        .execute(&mut *tx)
        .await?;
    let updated = sqlx::query(
        "UPDATE balance_legs SET gross_qty=$2,acquisition_cost=$3,base_commission=$4,executable_qty=$5
         WHERE position_id=$1",
    )
    .bind(f.position)
    .bind(p.gross)
    .bind(p.cost)
    .bind(p.base_fee)
    .bind(p.executable)
    .execute(&mut *tx)
    .await?;
    ensure!(updated.rows_affected() == 1);
    tx.commit().await?;
    Ok(())
}

/// Leave the leg pointing at an admission that is no longer current.
async fn supersede_in_place(pool: &PgPool, f: &Fixture) -> Result<()> {
    let successor = Uuid::now_v7();
    let admission = f.entry.intent.intent_id;
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id='binance:spot'")
        .bind(f.tenant)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE balance_leg_admissions SET superseded_at=now(),superseded_by=$2 WHERE entry_intent_id=$1")
        .bind(admission)
        .bind(successor)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO balance_leg_admissions (entry_intent_id,position_id,tenant_id,venue_id,account_scope,symbol,
            decided_month,taker_bps,gap_bps,notional_margin_bps,min_notional,lot_step,fee_asset_policy,
            envelope_source,envelope_as_of,envelope_hash,planned_qty,requested_qty,reference_price,
            remainder_reserve,admission_charge,initial_trigger,created_at)
         SELECT $2,position_id,tenant_id,venue_id,account_scope,symbol,decided_month,taker_bps,gap_bps,
            notional_margin_bps,min_notional,lot_step,fee_asset_policy,envelope_source,envelope_as_of,
            envelope_hash,planned_qty,requested_qty,reference_price,remainder_reserve,admission_charge,
            initial_trigger,created_at
         FROM balance_leg_admissions WHERE entry_intent_id=$1",
    )
    .bind(admission)
    .bind(successor)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn entry_partition_refusals_leave_every_row_unchanged(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    let set = expected(dec!(0.3), dec!(30000), dec!(0.0003), dec!(0.299));
    // (d1) A terminal regression keeps the existing TradeConflict.
    let f = Fixture::new(&pool).await;
    let ledger = f.ledger(&pool);
    let link = entry_link(&f);
    ensure!(
        ledger
            .record_order(&entry(&f, dec!(0.3), dec!(30000), true), &link, later(1))
            .await?
            .changed
    );
    let before = snapshot(&pool).await;
    let regressed = ledger
        .record_order(&entry(&f, dec!(0.2), dec!(20000), true), &link, later(2))
        .await;
    ensure!(matches!(regressed, Err(LedgerError::TradeConflict)), "d1: {regressed:?}");
    ensure!(snapshot(&pool).await == before, "d1 changed rows");

    // (d2) A recorded partition must equal a consistent terminal entry.
    let f = Fixture::new(&pool).await;
    seed_partition(&pool, &f, expected(dec!(0.2), dec!(20000), dec!(0.0002), dec!(0.199))).await?;
    let before = snapshot(&pool).await;
    let conflict = f
        .ledger(&pool)
        .record_order(&entry(&f, dec!(0.3), dec!(30000), true), &entry_link(&f), later(1))
        .await;
    ensure!(matches!(conflict, Err(LedgerError::IdentityConflict)), "d2: {conflict:?}");
    ensure!(snapshot(&pool).await == before, "d2 changed rows");

    // (e) A superseded admission supplies no terms: refusal, NULL partition.
    let f = Fixture::new(&pool).await;
    supersede_in_place(&pool, &f).await?;
    let before = snapshot(&pool).await;
    let refused = f
        .ledger(&pool)
        .record_order(&entry(&f, dec!(0.3), dec!(30000), true), &entry_link(&f), later(1))
        .await;
    ensure!(
        matches!(&refused, Err(LedgerError::Invariant(reason)) if reason.contains("current admission terms")),
        "e: {refused:?}"
    );
    ensure!(
        snapshot(&pool).await == before && leg_row(&pool, &f).await?.0.is_none(),
        "e changed rows"
    );

    // (f) Once set, M36 refuses a changed Q or C written directly.
    let f = Fixture::new(&pool).await;
    ensure!(
        f.ledger(&pool)
            .record_order(&entry(&f, dec!(0.3), dec!(30000), true), &entry_link(&f), later(1))
            .await?
            .changed
    );
    ensure!(leg_row(&pool, &f).await?.0 == Some(set));
    let before = snapshot(&pool).await;
    for sql in [
        "UPDATE balance_legs SET gross_qty=gross_qty+0.00000001 WHERE position_id=$1",
        "UPDATE balance_legs SET acquisition_cost=acquisition_cost+0.01 WHERE position_id=$1",
    ] {
        let error = sqlx::query(sql).bind(f.position).execute(&pool).await.err().context(sql)?;
        let code = error.as_database_error().and_then(|database| database.code());
        ensure!(code.as_deref() == Some("23514"), "f: {error}");
    }
    ensure!(snapshot(&pool).await == before, "f changed rows");
    println!(
        "EP1_REFUSALS d1=TradeConflict d2=IdentityConflict e=Invariant f=23514x2 rows=unchanged"
    );
    Ok(())
}

/// Wait until `writers` backends queue on the held accounting row; well
/// inside the writers' 300 ms lock timeout.
async fn queued_on_accounting(pool: &PgPool, writers: i64) -> Result<()> {
    tokio::time::timeout(Duration::from_millis(250), async {
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
                 AND wait_event_type='Lock' AND query LIKE 'UPDATE accounting_state%'",
            )
            .fetch_one(pool)
            .await?;
            if waiting == writers {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("writers never queued on the accounting row"))?
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn competing_entry_partition_first_sets_serialize(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    for same in [true, false] {
        let f = Fixture::new(&pool).await;
        let (_proxy, writers) = through_proxy(&pool, 2).await;
        let (left, right) = (f.ledger(&writers), f.ledger(&writers));
        let link = entry_link(&f);
        let first = entry(&f, dec!(0.3), dec!(30000), true);
        let second = if same {
            first.clone()
        } else {
            entry(&f, dec!(0.4), dec!(40000), true)
        };
        let version = f.version(&pool).await;
        // Both writers pass their unlocked pre-read before either can commit.
        let mut holder = pool.begin().await?;
        sqlx::query("SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot' FOR UPDATE")
            .bind(f.tenant)
            .fetch_one(&mut *holder)
            .await?;
        let release = async {
            queued_on_accounting(&pool, 2).await?;
            holder.rollback().await?;
            anyhow::Ok(())
        };
        let (a, b, released) = tokio::join!(
            left.record_order(&first, &link, later(1)),
            right.record_order(&second, &link, later(1)),
            release
        );
        released?;
        let (winner, other) = match (&a, &b) {
            (Ok(result), _) if result.changed => (&first, &b),
            (_, Ok(result)) if result.changed => (&second, &a),
            _ => return Err(anyhow!("no first set: {a:?} {b:?}")),
        };
        if same {
            ensure!(matches!(other, Ok(result) if !result.changed), "same: {other:?}");
        } else {
            ensure!(
                matches!(other, Err(LedgerError::TradeConflict | LedgerError::IdentityConflict)),
                "different: {other:?}"
            );
        }
        ensure!(f.version(&pool).await == version + 1, "one changed commit");
        let (partition, cohort, _) = leg_row(&pool, &f).await?;
        let partition = partition.context("winner partition")?;
        ensure!(
            cohort
                && (Some(partition.gross), Some(partition.cost))
                    == (Some(winner.executed_qty), winner.cumulative_quote_qty)
        );
        writers.close().await;
    }
    println!("EP1_COMPETING same=one_changed_one_unchanged different=one_set_one_refused version_delta=1");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn a_later_sell_consumes_against_the_recorded_partition(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    let f = Fixture::new(&pool).await;
    let ledger = f.ledger(&pool);
    ensure!(
        ledger
            .record_order(&entry(&f, dec!(0.3), dec!(30000), true), &entry_link(&f), later(1))
            .await?
            .changed
    );
    let set = expected(dec!(0.3), dec!(30000), dec!(0.0003), dec!(0.299));
    let sell = f.link(SELL, OrderOwner::Leg {
        intent_id: Uuid::now_v7(),
        position_id: f.position,
        kind: LegOrderKind::Sell,
    });
    let count = ledger.statements_total();
    ensure!(
        ledger
            .record_order(&f.record(SELL, dec!(0.1), OrderPhase::Terminal), &sell, later(2))
            .await?
            .changed
    );
    ensure!(ledger.statements_total() - count == 8);
    let (partition, _, consumed) = leg_row(&pool, &f).await?;
    ensure!(partition == Some(set) && consumed == dec!(0.1));
    // The R1/R3 evaluator's own calculation over the actual row.
    let facts = partition_facts(RecordedPartition {
        gross: set.gross,
        base_fee: set.base_fee,
        owned: set.executable,
        consumed,
        fee_evidence: FeeEvidence::ProvisionalBound,
    })
    .map_err(|error| anyhow!("{error}"))?;
    ensure!(facts.remainder() == Remainder::Quantity(dec!(0.199)));
    ensure!(facts.dust() == dec!(0.0007));
    println!("EP1_SELL consumed=0.1 G=0.199 dust=0.0007 partition=unchanged sql=8");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn entry_partition_guard_refuses_a_stale_identical_set(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    let f = Fixture::new(&pool).await;
    let ledger = f.ledger(&pool);
    ensure!(
        ledger
            .record_order(&entry(&f, dec!(0.3), dec!(30000), true), &entry_link(&f), later(1))
            .await?
            .changed
    );
    let recorded = leg_row(&pool, &f).await?.0.context("recorded partition")?;
    let before = snapshot(&pool).await;
    // Identical values pass M36 by themselves (rolled back here).
    let mut tx = pool.begin().await?;
    let identical = sqlx::query(
        "UPDATE balance_legs SET gross_qty=$2,acquisition_cost=$3,base_commission=$4,executable_qty=$5
         WHERE position_id=$1",
    )
    .bind(f.position)
    .bind(recorded.gross)
    .bind(recorded.cost)
    .bind(recorded.base_fee)
    .bind(recorded.executable)
    .execute(&mut *tx)
    .await?;
    ensure!(identical.rows_affected() == 1);
    tx.rollback().await?;
    // The production statement under the accounting row: only the guard refuses.
    let mut connection = pool.acquire().await?;
    let mut tx = connection.begin().await?;
    ledger.timeouts(&mut tx).await?;
    let bumped = sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id='binance:spot'")
        .bind(f.tenant)
        .execute(&mut *tx)
        .await?;
    ensure!(bumped.rows_affected() == 1);
    let count = ledger.statements_total();
    let refused = ledger
        .update_leg(&mut tx, f.position, &support::venue(), Decimal::ZERO, false, Some(&recorded))
        .await;
    ensure!(
        matches!(&refused, Err(LedgerError::Invariant(reason)) if reason.contains("partition is already set")),
        "i: {refused:?}"
    );
    ensure!(ledger.statements_total() - count == 1);
    tx.rollback().await?;
    ensure!(snapshot(&pool).await == before, "i changed rows");
    println!("EP1_GUARD identical_m36=accepted guarded_update=zero_rows rows=unchanged");
    Ok(())
}

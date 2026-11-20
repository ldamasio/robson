//! EP-2 in isolated PostgreSQL: the real entry-fee writer finalizes B once,
//! after real record_order and record_trades writes. Fixture amounts are
//! exchange metadata examples, never policy.
use robson_exec::{
    ports::TradeFill,
    venue::TradePage,
    venue_ledger::{EntryFeeHold, EntryFeePending, EntryFeePlan},
};

use super::*;

const BASE: &str = "BTC";

fn fill(id: u64, qty: Decimal, quote: Decimal, commission: Decimal, asset: &str) -> TradeFill {
    TradeFill {
        trade_id: id.to_string(),
        price: dec!(100000),
        qty,
        quote_qty: quote,
        commission,
        commission_asset: asset.into(),
        time: Some(at()),
    }
}

fn page(trades: Vec<TradeFill>) -> TradePage {
    TradePage { trades, next_from_id: None }
}

fn finalize(base_fee: Decimal, executable: Decimal, reduced: bool, deficit: bool) -> EntryFeePlan {
    EntryFeePlan::Finalize { base_fee, executable, reduced, deficit }
}

/// Q = 1 BTC for 100,000 USDT in two base-charged fills.
async fn complete_entry(
    ledger: &PgVenueLedger,
    f: &Fixture,
    first_fee: Decimal,
    second_fee: Decimal,
) -> Result<()> {
    let terminal = entry(f, dec!(1.00000000), dec!(100000.00), true);
    ensure!(ledger.record_order(&terminal, &entry_link(f), later(1)).await?.changed);
    let fills = page(vec![
        fill(1, dec!(0.6), dec!(60000.00), first_fee, BASE),
        fill(2, dec!(0.4), dec!(40000.00), second_fee, BASE),
    ]);
    ensure!(ledger.record_trades(&f.key(ENTRY), &fills, at()).await?.changed);
    Ok(())
}

/// One finalization measured by the store counter and independently on the
/// wire.
async fn fee_measured(
    ledger: &PgVenueLedger,
    proxy: &DbProxy,
    f: &Fixture,
) -> Result<(EntryFeePlan, u64)> {
    let (count, wire) = (ledger.statements_total(), proxy.stats().statements);
    let plan = ledger.finalize_entry_fee(f.position, BASE).await?;
    let count = ledger.statements_total() - count;
    ensure!(proxy.stats().statements - wire == count, "wire differs from counter {count}");
    Ok((plan, count))
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn entry_fee_finalizes_once_with_measured_statements(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    let mut marker = Vec::new();
    // (a) B < B̂ keeps X and grows D; (a′) the same on an opted ledger.
    for projected in [false, true] {
        let f = Fixture::new(&pool).await;
        let generation = if projected {
            Some(generation(&pool, &f).await?)
        } else {
            None
        };
        let (proxy, proxied) = through_proxy(&pool, 1).await;
        let mut ledger = f.ledger(&proxied);
        if let Some(b) = &generation {
            ledger = ledger
                .with_native_order_set_projection(b.clone())
                .with_trade_order_set_projection(b.clone());
        }
        let terminal = VenueOrderRecord {
            orig_qty: dec!(2),
            status: "EXPIRED".into(),
            ..entry(&f, dec!(1.0005), dec!(100050.00), true)
        };
        ensure!(ledger.record_order(&terminal, &entry_link(&f), later(1)).await?.changed);
        // B̂ = 1.0005 × 0.001 = 0.0010005; X = floor_step(0.9994995) = 0.999.
        let provisional = expected(dec!(1.0005), dec!(100050), dec!(0.0010005), dec!(0.999));
        ensure!(leg_row(&pool, &f).await?.0 == Some(provisional));
        let fills = page(vec![
            fill(1, dec!(0.6005), dec!(60050.00), dec!(0.0002), BASE),
            fill(2, dec!(0.4), dec!(40000.00), dec!(0.0002), BASE),
        ]);
        ensure!(ledger.record_trades(&f.key(ENTRY), &fills, at()).await?.changed);
        let before = f.version(&pool).await;
        let (plan, count) = fee_measured(&ledger, &proxy, &f).await?;
        // floor_step(1.0001) = 1 would raise X: X never rises, so it stays.
        ensure!(plan == finalize(dec!(0.0004), dec!(0.999), false, false), "a: {plan:?}");
        ensure!(count == 7 + u64::from(projected), "a: {count} statements");
        ensure!(f.version(&pool).await == before + 1);
        let done = LegPartition {
            base_fee: dec!(0.0004),
            base_fee_final: true,
            ..provisional
        };
        ensure!(leg_row(&pool, &f).await?.0 == Some(done));
        // D = Q − B − X grows from 0.0004995 to 0.0011.
        ensure!(done.gross - done.base_fee - done.executable == dec!(0.0011));
        if let Some(b) = &generation {
            ensure!(version(&pool, b).await? == before + 1);
            exact_reference(&pool, b).await?;
        }
        let rows = snapshot(&pool).await;
        let (replay, replayed) = fee_measured(&ledger, &proxy, &f).await?;
        ensure!(
            replay == EntryFeePlan::Unchanged && replayed == 1,
            "replay {replay:?} {replayed}"
        );
        ensure!(snapshot(&pool).await == rows, "replay changed rows");
        marker.push(format!("{}={count}/{replayed}", if projected { "opted" } else { "ordinary" }));
        proxied.close().await;
    }

    // (b) B > B̂ lowers X; the R1/R3 calculation reads it as final.
    let f = Fixture::new(&pool).await;
    let ledger = f.ledger(&pool);
    complete_entry(&ledger, &f, dec!(0.0009), dec!(0.0006)).await?;
    let plan = ledger.finalize_entry_fee(f.position, BASE).await?;
    ensure!(plan == finalize(dec!(0.0015), dec!(0.998), true, false), "b: {plan:?}");
    let (partition, _, consumed) = leg_row(&pool, &f).await?;
    let partition = partition.context("b partition")?;
    ensure!(partition.base_fee_final && partition.executable == dec!(0.998));
    let facts = partition_facts(RecordedPartition {
        gross: partition.gross,
        base_fee: partition.base_fee,
        owned: partition.executable,
        consumed,
        fee_evidence: FeeEvidence::FinalCommission,
    })
    .map_err(|error| anyhow!("{error}"))?;
    ensure!(facts.remainder() == Remainder::Quantity(dec!(0.998)) && facts.dust() == dec!(0.0005));

    // (c) A sale before finalization leaves a deficit, reported and accepted.
    let f = Fixture::new(&pool).await;
    let ledger = f.ledger(&pool);
    complete_entry(&ledger, &f, dec!(0.0009), dec!(0.0006)).await?;
    let sell = f.link(SELL, OrderOwner::Leg {
        intent_id: Uuid::now_v7(),
        position_id: f.position,
        kind: LegOrderKind::Sell,
    });
    let sold = f.record(SELL, dec!(0.999), OrderPhase::Terminal);
    ensure!(ledger.record_order(&sold, &sell, later(2)).await?.changed);
    let plan = ledger.finalize_entry_fee(f.position, BASE).await?;
    ensure!(plan == finalize(dec!(0.0015), dec!(0.998), true, true), "c: {plan:?}");
    let (partition, _, consumed) = leg_row(&pool, &f).await?;
    let partition = partition.context("c partition")?;
    ensure!(partition.base_fee_final && consumed == dec!(0.999));
    let facts = partition_facts(RecordedPartition {
        gross: partition.gross,
        base_fee: partition.base_fee,
        owned: partition.executable,
        consumed,
        fee_evidence: FeeEvidence::FinalCommission,
    })
    .map_err(|error| anyhow!("{error}"))?;
    ensure!(facts.remainder() == Remainder::Deficit { excess: dec!(0.001) });
    println!(
        "EP2_FINALIZE {} b=reduced:X=0.998:G=0.998:dust=0.0005 c=deficit:0.001",
        marker.join(" ")
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn entry_fee_waits_or_holds_without_writing(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    // (d) Incomplete fills wait; the completing page then finalizes.
    let f = Fixture::new(&pool).await;
    let (proxy, proxied) = through_proxy(&pool, 1).await;
    let ledger = f.ledger(&proxied);
    let terminal = entry(&f, dec!(1.00000000), dec!(100000.00), true);
    ensure!(ledger.record_order(&terminal, &entry_link(&f), later(1)).await?.changed);
    let first = page(vec![fill(1, dec!(0.6), dec!(60000.00), dec!(0.0006), BASE)]);
    ensure!(ledger.record_trades(&f.key(ENTRY), &first, at()).await?.changed);
    let before = snapshot(&pool).await;
    let (plan, count) = fee_measured(&ledger, &proxy, &f).await?;
    ensure!(
        plan == EntryFeePlan::NotReady(EntryFeePending::FillsIncomplete) && count == 1,
        "d: {plan:?} {count}"
    );
    ensure!(snapshot(&pool).await == before, "d changed rows");
    let second = page(vec![fill(2, dec!(0.4), dec!(40000.00), dec!(0.0004), BASE)]);
    ensure!(ledger.record_trades(&f.key(ENTRY), &second, at()).await?.changed);
    let (plan, count) = fee_measured(&ledger, &proxy, &f).await?;
    ensure!(
        plan == finalize(dec!(0.001), dec!(0.999), false, false) && count == 7,
        "d: {plan:?}"
    );
    proxied.close().await;

    // (e) quote mismatch, (f) a non-base commission, (g) an unfilled entry.
    for case in ["e", "f", "g"] {
        let f = Fixture::new(&pool).await;
        let (proxy, proxied) = through_proxy(&pool, 1).await;
        let ledger = f.ledger(&proxied);
        let held = match case {
            "e" | "f" => {
                let terminal = entry(&f, dec!(1.00000000), dec!(100000.00), true);
                ensure!(ledger.record_order(&terminal, &entry_link(&f), later(1)).await?.changed);
                let (quote, asset) = if case == "e" {
                    (dec!(39999.00), BASE)
                } else {
                    (dec!(40000.00), "BNB")
                };
                let fills = page(vec![
                    fill(1, dec!(0.6), dec!(60000.00), dec!(0.0006), BASE),
                    fill(2, dec!(0.4), quote, dec!(0.00001), asset),
                ]);
                ensure!(ledger.record_trades(&f.key(ENTRY), &fills, at()).await?.changed);
                if case == "e" {
                    EntryFeePlan::Held(EntryFeeHold::QuoteMismatch {
                        fills: dec!(99999.00),
                        cost: dec!(100000.00),
                    })
                } else {
                    EntryFeePlan::Held(EntryFeeHold::NonBaseCommission { fills: 1 })
                }
            },
            _ => {
                let expired = entry(&f, Decimal::ZERO, Decimal::ZERO, true);
                ensure!(ledger.record_order(&expired, &entry_link(&f), later(1)).await?.changed);
                EntryFeePlan::NotReady(EntryFeePending::NoPartition)
            },
        };
        let before = snapshot(&pool).await;
        let (plan, count) = fee_measured(&ledger, &proxy, &f).await?;
        ensure!(plan == held && count == 1, "{case}: {plan:?} {count}");
        ensure!(snapshot(&pool).await == before, "{case} changed rows");
        proxied.close().await;
    }

    // (l) Each amount is exact but the sum is not: refused before planning.
    for case in ["quote", "fee"] {
        let f = Fixture::new(&pool).await;
        let (proxy, proxied) = through_proxy(&pool, 1).await;
        let ledger = f.ledger(&proxied);
        let tiny = Decimal::from_i128_with_scale(1, 28);
        let (cost, fills) = if case == "quote" {
            (dec!(10.00), vec![
                fill(1, dec!(0.5), dec!(10), dec!(0.0005), BASE),
                fill(2, dec!(0.5), tiny, dec!(0.0005), BASE),
            ])
        } else {
            (dec!(100000.00), vec![
                fill(1, dec!(0.6), dec!(60000.00), dec!(10), BASE),
                fill(2, dec!(0.4), dec!(40000.00), tiny, BASE),
            ])
        };
        let terminal = entry(&f, dec!(1.00000000), cost, true);
        ensure!(ledger.record_order(&terminal, &entry_link(&f), later(1)).await?.changed);
        ensure!(ledger.record_trades(&f.key(ENTRY), &page(fills), at()).await?.changed);
        let before = snapshot(&pool).await;
        let (count, wire) = (ledger.statements_total(), proxy.stats().statements);
        let refused = ledger.finalize_entry_fee(f.position, BASE).await;
        ensure!(matches!(refused, Err(LedgerError::Unrepresentable)), "l {case}: {refused:?}");
        ensure!(ledger.statements_total() - count == 1 && proxy.stats().statements - wire == 1);
        ensure!(snapshot(&pool).await == before, "l {case} changed rows");
        proxied.close().await;
    }
    println!(
        "EP2_WAIT_HOLD d=FillsIncomplete/1>Finalize/7 e=QuoteMismatch/1 f=NonBaseCommission/1 \
         g=NoPartition/1 l_quote=Unrepresentable/1 l_fee=Unrepresentable/1 rows=unchanged"
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn entry_fee_is_independent_of_arrival_order(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    let first_fill = fill(1, dec!(0.6), dec!(60000.00), dec!(0.0009), BASE);
    let second_fill = fill(2, dec!(0.4), dec!(40000.00), dec!(0.0006), BASE);
    let incomplete = EntryFeePlan::NotReady(EntryFeePending::FillsIncomplete);
    let done = finalize(dec!(0.0015), dec!(0.998), true, false);

    // The terminal record first, then every fill.
    let terminal_first = Fixture::new(&pool).await;
    let ledger = terminal_first.ledger(&pool);
    let terminal = entry(&terminal_first, dec!(1.00000000), dec!(100000.00), true);
    ensure!(
        ledger
            .record_order(&terminal, &entry_link(&terminal_first), later(1))
            .await?
            .changed
    );
    ensure!(ledger.finalize_entry_fee(terminal_first.position, BASE).await? == incomplete);
    let both = page(vec![first_fill.clone(), second_fill.clone()]);
    ensure!(ledger.record_trades(&terminal_first.key(ENTRY), &both, at()).await?.changed);
    ensure!(ledger.finalize_entry_fee(terminal_first.position, BASE).await? == done);

    // A live entry's first fill, the terminal record, then the last fill.
    let fills_first = Fixture::new(&pool).await;
    let ledger = fills_first.ledger(&pool);
    let link = entry_link(&fills_first);
    let live = entry(&fills_first, dec!(0.6), dec!(60000.00), false);
    ensure!(ledger.record_order(&live, &link, later(1)).await?.changed);
    let key = fills_first.key(ENTRY);
    ensure!(ledger.record_trades(&key, &page(vec![first_fill]), at()).await?.changed);
    let unfilled = EntryFeePlan::NotReady(EntryFeePending::NoPartition);
    ensure!(ledger.finalize_entry_fee(fills_first.position, BASE).await? == unfilled);
    let terminal = entry(&fills_first, dec!(1.00000000), dec!(100000.00), true);
    ensure!(ledger.record_order(&terminal, &link, later(2)).await?.changed);
    ensure!(ledger.finalize_entry_fee(fills_first.position, BASE).await? == incomplete);
    ensure!(ledger.record_trades(&key, &page(vec![second_fill]), at()).await?.changed);
    ensure!(ledger.finalize_entry_fee(fills_first.position, BASE).await? == done);

    let one = leg_row(&pool, &terminal_first).await?;
    let other = leg_row(&pool, &fills_first).await?;
    ensure!(one == other && one.0.is_some_and(|p| p.base_fee_final), "{one:?} {other:?}");
    println!("EP2_ORDER terminal_first=fills_first final=B0.0015:X0.998");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn competing_entry_fee_finalizers_serialize(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    let f = Fixture::new(&pool).await;
    complete_entry(&f.ledger(&pool), &f, dec!(0.0009), dec!(0.0006)).await?;
    let (_proxy, writers) = through_proxy(&pool, 2).await;
    let (left, right) = (f.ledger(&writers), f.ledger(&writers));
    let version = f.version(&pool).await;
    // Both finalizers pass their unlocked pre-read before either can commit.
    let mut holder = pool.begin().await?;
    sqlx::query(
        "SELECT version FROM accounting_state WHERE tenant_id=$1 AND venue_id='binance:spot' FOR UPDATE",
    )
    .bind(f.tenant)
    .fetch_one(&mut *holder)
    .await?;
    let release = async {
        queued_on_accounting(&pool, 2).await?;
        holder.rollback().await?;
        anyhow::Ok(())
    };
    let (a, b, released) = tokio::join!(
        left.finalize_entry_fee(f.position, BASE),
        right.finalize_entry_fee(f.position, BASE),
        release
    );
    released?;
    let plans = [a?, b?];
    let counts = [left.statements_total(), right.statements_total()];
    let won = plans.iter().position(|plan| matches!(plan, EntryFeePlan::Finalize { .. }));
    let won = won.context("no finalization")?;
    ensure!(plans[1 - won] == EntryFeePlan::Unchanged, "{plans:?}");
    ensure!(counts[won] == 7 && counts[1 - won] == 6, "{counts:?}");
    ensure!(f.version(&pool).await == version + 1, "one changed commit");
    ensure!(leg_row(&pool, &f).await?.0.is_some_and(|p| p.base_fee_final));
    writers.close().await;
    println!("EP2_COMPETING one_finalized_one_unchanged sql=7/6 version_delta=1");
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn entry_fee_is_frozen_and_guarded(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    let f = Fixture::new(&pool).await;
    let ledger = f.ledger(&pool);
    complete_entry(&ledger, &f, dec!(0.0009), dec!(0.0006)).await?;
    let provisional = leg_row(&pool, &f).await?.0.context("provisional partition")?;
    let plan = ledger.finalize_entry_fee(f.position, BASE).await?;
    ensure!(plan == finalize(dec!(0.0015), dec!(0.998), true, false), "{plan:?}");
    let before = snapshot(&pool).await;
    // (j) M36 refuses a changed final B and a raised X written directly.
    for sql in [
        "UPDATE balance_legs SET base_commission=base_commission+0.0001 WHERE position_id=$1",
        "UPDATE balance_legs SET executable_qty=executable_qty+0.001 WHERE position_id=$1",
    ] {
        let error = sqlx::query(sql).bind(f.position).execute(&pool).await.err().context(sql)?;
        let code = error.as_database_error().and_then(|database| database.code());
        ensure!(code.as_deref() == Some("23514"), "j: {error}");
    }
    ensure!(snapshot(&pool).await == before, "j changed rows");
    // (k) The production statement with a stale request, under the accounting
    // row: identical values would pass M36, so only the compare-and-set refuses.
    let mut connection = pool.acquire().await?;
    let mut tx = connection.begin().await?;
    ledger.timeouts(&mut tx).await?;
    let bumped = sqlx::query(
        "UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id='binance:spot'",
    )
    .bind(f.tenant)
    .execute(&mut *tx)
    .await?;
    ensure!(bumped.rows_affected() == 1);
    let count = ledger.statements_total();
    let venue = support::venue();
    let refused = ledger
        .write_entry_fee(
            &mut tx,
            f.position,
            venue.as_str(),
            &provisional,
            dec!(0.0015),
            dec!(0.998),
        )
        .await;
    ensure!(
        matches!(&refused, Err(LedgerError::Invariant(reason)) if reason.contains("compare-and-set")),
        "k: {refused:?}"
    );
    ensure!(ledger.statements_total() - count == 1);
    tx.rollback().await?;
    ensure!(snapshot(&pool).await == before, "k changed rows");
    println!("EP2_FROZEN j=23514x2 k=zero_rows rows=unchanged");
    Ok(())
}

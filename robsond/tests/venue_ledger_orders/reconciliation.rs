//! 8k-2 in isolated PostgreSQL: the fixture-only detector composition over
//! the native ClockOwner and the loopback triple, on ledgers seeded through
//! the real journal and venue writers. Amounts are exchange metadata
//! examples, never policy; nothing here is a production caller.
use std::{sync::Arc, time::Duration};

use anyhow::anyhow;
use robson_connectors::inventory_loopback::LOOPBACK_UID;
use robson_exec::{
    clock_records::RawClockCapture,
    gate::{IncidentReason, PendingExecutionOwner, PendingExecutionRegistration},
    order_journal::{
        BalanceBatch, IntentKind, IntentRow, JournalBackend, JournalBatch, OrderJournal,
        OutcomeRow, VenueOutcome,
    },
    ports::TradeFill,
    venue::{ProtectionKind, TradePage, VenueId},
    venue_ledger::EntryFeePlan,
};
use robson_testkit::db_proxy::DbProxy;
use rust_decimal_macros::dec;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::time::{timeout, Instant};

use super::{support::through_proxy, *};
use crate::{
    balance_reconciliation::{
        consequence, AttemptOutcome, BaseVerdict, Consequence, DetectorScope, Finding,
        IncompleteReason, ObservationInterval, ScanCost, UnqualifiedReason,
    },
    order_journal_store::PgJournalBackend,
    spot_order_set_store::ExpectedGeneration,
    venue_clock_owner::{AccountSeen, Hook, Listener, ReconciliationRig, ScanInputs, Scanned},
};

const ENTRY: u64 = 40;
const STOP: u64 = 41;

struct Seen(i64);

impl Seen {
    fn next(&mut self) -> SeenAt {
        self.0 += 1;
        later(self.0)
    }
}

/// Every case unless RBX_8K2_ONLY names a subset: a mutant run builds the
/// same seeds and runs only the case named to kill it.
fn wanted(case: &str) -> bool {
    std::env::var("RBX_8K2_ONLY").map_or(true, |only| only.split(',').any(|c| c == case))
}

fn intent(f: &Fixture, kind: IntentKind, cancels: Option<String>) -> IntentRow {
    IntentRow {
        intent_id: Uuid::now_v7(),
        venue_id: VenueId::BINANCE_SPOT.into(),
        account_scope: f.scope.clone(),
        client_order_id: Uuid::now_v7().simple().to_string(),
        position_id: Some(f.position),
        kind,
        cancels_order_id: cancels,
        params: serde_json::json!({}),
        created_at: at(),
    }
}

/// One intent through the journal writer, with its outcome if any.
async fn commit(
    pool: &PgPool,
    f: &Fixture,
    intent: &IntentRow,
    outcome: Option<OutcomeRow>,
) -> Result<()> {
    let batch = JournalBatch {
        intent: intent.clone(),
        attempts: Vec::new(),
        outcome,
        balance: Some(BalanceBatch {
            admission: None,
            handed_off: false,
            commit_bound: Duration::from_millis(500),
        }),
    };
    PgJournalBackend::new(pool, f.tenant)
        .commit(&batch)
        .await
        .map_err(|error| anyhow!("journal: {error:?}"))
}

/// The leg's entry order through record_order; EP-1 first-sets the
/// partition at its terminal record (Q 1, C 100000: B̂ 0.001, X 0.999).
async fn entry(
    pool: &PgPool,
    f: &Fixture,
    executed: Decimal,
    terminal: bool,
    seen: SeenAt,
) -> Result<()> {
    let record = VenueOrderRecord {
        client_order_id: f.entry.intent.client_order_id.clone(),
        side: OrderSide::Buy,
        orig_qty: dec!(1.00000000),
        cumulative_quote_qty: Some(executed * dec!(100000)),
        status: match (terminal, executed == dec!(1)) {
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
    };
    let link = OrderLink {
        client_order_id: f.entry.intent.client_order_id.clone(),
        ..f.link(ENTRY, OrderOwner::Leg {
            intent_id: f.entry.intent.intent_id,
            position_id: f.position,
            kind: LegOrderKind::Entry,
        })
    };
    f.ledger(pool).record_order(&record, &link, seen).await?;
    Ok(())
}

/// A protection stop's venue record.
fn stop(
    f: &Fixture,
    id: u64,
    client: &str,
    executed: Decimal,
    phase: OrderPhase,
) -> VenueOrderRecord {
    VenueOrderRecord {
        client_order_id: client.into(),
        order_type: "STOP_LOSS".into(),
        protection_kind: Some(ProtectionKind::MarketOnTrigger),
        status: match (phase, executed.is_zero()) {
            (OrderPhase::Terminal, _) => "CANCELED",
            (_, true) => "NEW",
            (_, false) => "PARTIALLY_FILLED",
        }
        .into(),
        orig_qty: dec!(0.999),
        cumulative_quote_qty: Some(executed * dec!(90)),
        stop_price: Some(dec!(90)),
        ..f.record(id, executed, phase)
    }
}

/// A protection intent and its live stop over X, ledger-backed with the NULL
/// outcome the projected writers leave.
async fn protect(pool: &PgPool, f: &Fixture, id: u64, seen: SeenAt) -> Result<OrderLink> {
    let protection = intent(f, IntentKind::Protection, None);
    commit(pool, f, &protection, None).await?;
    let link = OrderLink {
        client_order_id: protection.client_order_id.clone(),
        ..f.link(id, OrderOwner::Leg {
            position_id: f.position,
            intent_id: protection.intent_id,
            kind: LegOrderKind::Stop,
        })
    };
    let record = stop(f, id, &protection.client_order_id, Decimal::ZERO, OrderPhase::Live);
    f.ledger(pool).record_order(&record, &link, seen).await?;
    Ok(link)
}

/// A filled leg protected by one live stop.
/// A filled leg whose entry commission EP-2 finalized from its base-charged
/// fills (B = 0.001, so X stays 0.999), protected by one live stop.
async fn protected(pool: &PgPool, tenant: Uuid, seen: &mut Seen) -> Result<(Fixture, OrderLink)> {
    let f = Fixture::for_tenant(pool, tenant).await;
    entry(pool, &f, dec!(1.00000000), true, seen.next()).await?;
    let fill = |id: u64, qty: Decimal| TradeFill {
        trade_id: id.to_string(),
        price: dec!(100000),
        qty,
        quote_qty: qty * dec!(100000),
        commission: qty / dec!(1000),
        commission_asset: "BTC".into(),
        time: Some(at()),
    };
    let fills = TradePage {
        trades: vec![fill(1, dec!(0.6)), fill(2, dec!(0.4))],
        next_from_id: None,
    };
    let ledger = f.ledger(pool);
    ledger.record_trades(&f.key(ENTRY), &fills, at()).await?;
    let plan = ledger.finalize_entry_fee(f.position, "BTC").await?;
    ensure!(
        matches!(plan, EntryFeePlan::Finalize { base_fee, executable, reduced: false, deficit: false }
            if base_fee == dec!(0.001) && executable == dec!(0.999)),
        "EP-2: {plan:?}"
    );
    let link = protect(pool, &f, STOP, seen.next()).await?;
    Ok((f, link))
}

/// The pre statement under EXPLAIN (ANALYZE, BUFFERS) on the fixture: the
/// intent probe's plan nodes and the rows they examined.
async fn explain_pre(pool: &PgPool, b: &JournalGeneration) -> Result<(Vec<String>, u64)> {
    let sql = format!(
        "EXPLAIN (ANALYZE, BUFFERS)\n{}",
        include_str!("../../src/balance_reconciliation/btc_partition.sql")
    );
    let plan: Vec<String> = sqlx::query_scalar(&sql)
        .bind(b.tenant_id)
        .bind(VenueId::BINANCE_SPOT)
        .bind(&b.account_scope)
        .bind(b.generation_id)
        .bind(b.process_epoch)
        .bind(b.source_audit_id)
        .bind(LOOPBACK_UID.to_string())
        .bind(SYMBOL)
        .fetch_all(pool)
        .await?;
    let number = |line: &str, key: &str| -> Option<u64> {
        let rest = &line[line.find(key)? + key.len()..];
        rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
    };
    let (mut nodes, mut examined) = (Vec::new(), 0);
    for (i, line) in plan.iter().enumerate() {
        if !line.contains(" on order_intents") {
            continue;
        }
        nodes.push(line.trim().to_string());
        // A node the executor never reached examined nothing.
        let Some(at) = line.find("(actual time") else {
            continue;
        };
        let actual = &line[at..];
        let rows = number(actual, "rows=").context("actual rows")?;
        let loops = number(actual, "loops=").context("loops")?;
        let removed = plan[i + 1..]
            .iter()
            .take_while(|l| !l.trim_start().starts_with("->"))
            .find_map(|l| number(l, "Rows Removed by Filter: "))
            .unwrap_or(0);
        examined += (rows + removed) * loops;
    }
    Ok((nodes, examined))
}

/// The scope's generation at the current version on the rig's epoch, its
/// venue UID bound. The hints flag is the accepted fixture UPDATE.
async fn install(pool: &PgPool, f: &Fixture, epoch: Uuid, seed: bool) -> Result<JournalGeneration> {
    let b = JournalGeneration {
        tenant_id: f.tenant,
        account_scope: f.scope.clone(),
        generation_id: Uuid::now_v7(),
        source_audit_id: Uuid::now_v7(),
        process_epoch: epoch,
    };
    sqlx::query(
        "INSERT INTO public.venue_accounts (venue_id,account_scope,tenant_id,uid,bound_at,cutover_id)
         VALUES ('binance:spot',$1,$2,$3,now(),$4) ON CONFLICT DO NOTHING",
    )
    .bind(&f.scope).bind(f.tenant).bind(LOOPBACK_UID.to_string()).bind(Uuid::now_v7())
    .execute(pool).await?;
    sqlx::query(
        "INSERT INTO public.spot_order_set_generation (
            tenant_id,venue_id,account_scope,generation_id,source_audit_id,process_epoch,accounting_version)
         VALUES ($1,'binance:spot',$2,$3,$4,$5,$6)
         ON CONFLICT (tenant_id,venue_id,account_scope) DO UPDATE SET
            generation_id=EXCLUDED.generation_id, source_audit_id=EXCLUDED.source_audit_id,
            process_epoch=EXCLUDED.process_epoch, accounting_version=EXCLUDED.accounting_version",
    )
    .bind(f.tenant).bind(&f.scope).bind(b.generation_id).bind(b.source_audit_id)
    .bind(b.process_epoch).bind(f.version(pool).await).execute(pool).await?;
    if seed {
        let receipt: Uuid = sqlx::query_scalar(include_str!("oracle_seed.sql"))
            .bind(b.tenant_id)
            .bind(&b.account_scope)
            .bind(b.generation_id)
            .fetch_one(pool)
            .await?;
        ensure!(receipt == b.generation_id);
    }
    sqlx::query("UPDATE public.spot_order_set_generation SET position_hints_complete = true WHERE generation_id = $1")
        .bind(b.generation_id).execute(pool).await?;
    Ok(b)
}

fn scope<'a>(venue: &'a VenueId, b: &'a JournalGeneration, uid: &'a str) -> DetectorScope<'a> {
    DetectorScope {
        expected: ExpectedGeneration {
            tenant_id: b.tenant_id,
            venue,
            account_scope: &b.account_scope,
            generation_id: b.generation_id,
            process_epoch: b.process_epoch,
            source_audit_id: b.source_audit_id,
        },
        uid,
        operated: SYMBOL,
        base_asset: "BTC",
    }
}

fn account(balances: &[(&str, Decimal, Decimal)]) -> String {
    let balances: Vec<_> = balances
        .iter()
        .map(|(asset, free, locked)| {
            format!(r#"{{"asset":"{asset}","free":"{free}","locked":"{locked}"}}"#)
        })
        .collect();
    format!(
        r#"{{"canTrade":true,"accountType":"SPOT","permissions":["SPOT"],"balances":[{}],"uid":{LOOPBACK_UID}}}"#,
        balances.join(",")
    )
}

fn btc(free: Decimal, locked: Decimal) -> String {
    account(&[("USDT", dec!(1805), dec!(0)), ("BTC", free, locked)])
}

fn order(
    symbol: &str,
    id: u64,
    list: i64,
    client: &str,
    kind: &str,
    side: &str,
    executed: Decimal,
) -> String {
    format!(
        r#"{{"symbol":"{symbol}","orderId":{id},"orderListId":{list},"clientOrderId":"{client}","price":"0","origQty":"0.999","executedQty":"{executed}","cummulativeQuoteQty":"0","status":"NEW","timeInForce":"GTC","type":"{kind}","side":"{side}","stopPrice":"90","time":1,"updateTime":2,"workingTime":-1,"selfTradePreventionMode":"EXPIRE_MAKER"}}"#
    )
}

/// The listed Robson stop, as the venue shows it.
fn listed(link: &OrderLink, id: u64, executed: Decimal) -> String {
    order(SYMBOL, id, -1, &link.client_order_id, "STOP_LOSS", "SELL", executed)
}

fn list(symbol: &str, id: i64, client: &str, member: u64) -> String {
    format!(
        r#"{{"orderListId":{id},"contingencyType":"OCO","listStatusType":"EXEC_STARTED","listOrderStatus":"EXECUTING","listClientOrderId":"{client}","transactionTime":3,"symbol":"{symbol}","orders":[{{"symbol":"{symbol}","orderId":{member},"clientOrderId":"{client}-m"}}]}}"#
    )
}

fn triple(account: String, orders: &[String], lists: &[String]) -> [String; 3] {
    [
        account,
        format!("[{}]", orders.join(",")),
        format!("[{}]", lists.join(",")),
    ]
}

/// What every scan on one rig shares: the counting proxy and read-only
/// options, the bound UID, and the gate with its pending-execution owner.
struct Ctx {
    proxy: DbProxy,
    proxied: PgPool,
    options: PgConnectOptions,
    venue: VenueId,
    uid: String,
    journal: OrderJournal,
    pending: PendingExecutionOwner,
}

impl Ctx {
    async fn new(pool: &PgPool, rig: &ReconciliationRig) -> Result<Self> {
        let (proxy, proxied) = through_proxy(pool, 2).await;
        let options = proxied.connect_options().as_ref().clone().options([
            ("default_transaction_read_only", "on"),
            ("lock_timeout", "300ms"),
            ("statement_timeout", "1s"),
        ]);
        let (journal, pending) = rig.gate()?;
        Ok(Self {
            proxy,
            proxied,
            options,
            venue: VenueId::new(VenueId::BINANCE_SPOT),
            uid: LOOPBACK_UID.to_string(),
            journal,
            pending,
        })
    }

    fn inputs<'a>(&'a self, b: &'a JournalGeneration) -> ScanInputs<'a> {
        self.with_uid(b, &self.uid)
    }

    fn with_uid<'a>(&'a self, b: &'a JournalGeneration, uid: &'a str) -> ScanInputs<'a> {
        ScanInputs {
            wire: &self.proxy,
            options: &self.options,
            scope: scope(&self.venue, b, uid),
            gate: self.journal.gate(),
            pending: &self.pending,
        }
    }
}

/// Settles capacity for `attempts` triples, then one scan over `answers`.
async fn scan(
    rig: &mut ReconciliationRig,
    inputs: &ScanInputs<'_>,
    attempts: usize,
    answers: &[[String; 3]],
    between: Option<Hook<'_>>,
) -> Result<Scanned> {
    rig.settle(attempts).await?;
    let listener = Listener::triples(Instant::now() + Duration::from_secs(60), answers).await?;
    let scanned = rig.scan(inputs, &listener, between).await;
    let finished = listener.finish(Instant::now() + Duration::from_secs(5)).await;
    let scanned = scanned?;
    finished?;
    Ok(scanned)
}

fn wall(capture: &RawClockCapture) -> i128 {
    i128::from(capture.wall_seconds) * 1_000_000_000 + i128::from(capture.wall_nanoseconds)
}

/// Measured SQL, physical connections and handoffs equal the cost model.
fn costs(case: &str, s: &Scanned, attempts: u8, sql: u32, http: u32, weight: u32) -> Result<()> {
    let cost = ScanCost { sql, http, weight };
    ensure!(
        s.outcome.attempts == attempts
            && s.outcome.cost == cost
            && s.sql == u64::from(sql)
            && s.connections == http as usize
            && s.handoffs == u64::from(http),
        "{case}: attempts {} cost {:?} sql {} connections {} handoffs {}",
        s.outcome.attempts,
        s.outcome.cost,
        s.sql,
        s.connections,
        s.handoffs
    );
    println!(
        "RECON_8K2 case={case} attempts={attempts} sql={sql} http={http} weight={weight} outcomes={:?}",
        s.attempts
    );
    Ok(())
}

fn interval(s: &Scanned) -> Result<ObservationInterval> {
    match &s.verdict {
        BaseVerdict::Clean { interval }
        | BaseVerdict::Discrepancy { interval, .. }
        | BaseVerdict::Incomplete { interval, .. } => Ok(*interval),
        BaseVerdict::Unqualified(reason) => Err(anyhow!("unqualified: {reason:?}")),
    }
}

fn findings(case: &str, s: &Scanned, expected: &[Finding]) -> Result<()> {
    ensure!(
        matches!(&s.verdict, BaseVerdict::Discrepancy { findings, .. } if findings == expected),
        "{case}: {:?}",
        s.verdict
    );
    Ok(())
}

fn incomplete(case: &str, s: &Scanned, reason: IncompleteReason) -> Result<()> {
    ensure!(
        matches!(&s.verdict, BaseVerdict::Incomplete { reason: r, .. } if *r == reason),
        "{case}: {:?}",
        s.verdict
    );
    Ok(())
}

fn clean(case: &str, s: &Scanned) -> Result<()> {
    ensure!(matches!(s.verdict, BaseVerdict::Clean { .. }), "{case}: {:?}", s.verdict);
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn btc_base_detector_cases(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    let tenant = Uuid::now_v7();
    let mut rig = ReconciliationRig::open(&pool, tenant).await?;
    let cases = base_cases(&pool, tenant, &mut rig).await;
    let closed = rig.close().await;
    cases?;
    closed?;
    Ok(())
}

async fn base_cases(pool: &PgPool, tenant: Uuid, rig: &mut ReconciliationRig) -> Result<()> {
    let mut seen = Seen(0);
    // Every ledger is seeded through the real writers while the barrier runs.
    let (a, a_stop) = protected(pool, tenant, &mut seen).await?;
    let closed = Fixture::for_tenant(pool, tenant).await;
    entry(pool, &closed, dec!(1.00000000), true, seen.next()).await?;
    // No writer closes a leg before the W record slice: a fixture UPDATE.
    sqlx::query("UPDATE balance_legs SET closed_at = now() WHERE position_id = $1")
        .bind(closed.position)
        .execute(pool)
        .await?;
    let live = Fixture::for_tenant(pool, tenant).await;
    entry(pool, &live, dec!(0.1), false, seen.next()).await?;
    let sent = Fixture::for_tenant(pool, tenant).await;
    let unprotected = Fixture::for_tenant(pool, tenant).await;
    entry(pool, &unprotected, dec!(1.00000000), true, seen.next()).await?;
    commit(pool, &unprotected, &intent(&unprotected, IntentKind::Protection, None), None).await?;
    let succeeded = Fixture::for_tenant(pool, tenant).await;
    let mut handed = succeeded.entry.clone();
    handed.outcome = Some(OutcomeRow {
        venue_outcome: Some(VenueOutcome::Success),
        ..OutcomeRow::default()
    });
    handed.balance.as_mut().context("admission batch")?.handed_off = true;
    PgJournalBackend::new(pool, tenant)
        .commit(&handed)
        .await
        .map_err(|e| anyhow!("{e:?}"))?;
    let unfilled = Fixture::for_tenant(pool, tenant).await;
    entry(pool, &unfilled, Decimal::ZERO, true, seen.next()).await?;
    // A cancelled stop with a definite cancel outcome and its replacement.
    let (replaced, first) = protected(pool, tenant, &mut seen).await?;
    let cancel = intent(&replaced, IntentKind::Cancel, Some(STOP.to_string()));
    commit(
        pool,
        &replaced,
        &cancel,
        Some(OutcomeRow {
            venue_outcome: Some(VenueOutcome::Success),
            cancel_class: Some("cancelled"),
            ..OutcomeRow::default()
        }),
    )
    .await?;
    let cancelled =
        stop(&replaced, STOP, &first.client_order_id, Decimal::ZERO, OrderPhase::Terminal);
    replaced.ledger(pool).record_order(&cancelled, &first, seen.next()).await?;
    let replacement = protect(pool, &replaced, STOP + 2, seen.next()).await?;
    // An uncertain cancel whose target stop is still live.
    let (pending_cancel, uncertain_stop) = protected(pool, tenant, &mut seen).await?;
    let uncertain = intent(&pending_cancel, IntentKind::Cancel, Some(STOP.to_string()));
    commit(pool, &pending_cancel, &uncertain, None).await?;

    rig.ready().await?;
    let epoch = rig.start.epoch.epoch_id;
    let ctx = Ctx::new(pool, rig).await?;
    let robson = listed(&a_stop, STOP, Decimal::ZERO);
    let protected_scope = install(pool, &a, epoch, false).await?;
    let b = &protected_scope;
    let mut cut = None;
    if wanted("n1") {
        // Ledger-backed entry and protection intents keep a NULL outcome.
        let n1 = scan(
            rig,
            &ctx.inputs(b),
            1,
            &[triple(btc(dec!(0), dec!(0.999)), &[robson.clone()], &[])],
            None,
        )
        .await?;
        clean("n1", &n1)?;
        costs("n1", &n1, 1, 2, 3, 106)?;
        let account = n1.account.context("n1 ACCOUNT receipt")?;
        let i = interval(&n1)?;
        let u = (account.u2 + 1) / 2;
        ensure!(
            i.start_ns == wall(&account.send) - u
                && i.cut_ns == i.start_ns
                && i.end_ns == wall(&account.receive) + u
                && i.end_ns < wall(&account.composite_end) + u,
            "n1: the interval and cut are the ACCOUNT receipt's: {i:?}"
        );
        cut = Some(i);
        // Plan item 2: the intent probe's examined rows, measured.
        let (nodes, examined) = explain_pre(pool, b).await?;
        ensure!(!nodes.is_empty(), "n1: the intent probe in the plan: {nodes:?}");
        println!(
            "RECON_8K2_EXPLAIN intent_nodes={} examined_rows={examined} plan={nodes:?}",
            nodes.len()
        );
    }
    if wanted("n2") {
        // Operator excess: fenced and paged, never sold or sent anywhere.
        let n2 = scan(
            rig,
            &ctx.inputs(b),
            1,
            &[triple(btc(dec!(0.5), dec!(0.999)), &[robson.clone()], &[])],
            None,
        )
        .await?;
        let excess = Finding::BaseExcess { holdings: dec!(1.499), upper: dec!(1) };
        findings("n2", &n2, std::slice::from_ref(&excess))?;
        ensure!(consequence(&excess) == Consequence::FenceAndPage, "n2: consequence");
        costs("n2", &n2, 1, 2, 3, 106)?;
    }
    if wanted("n3") {
        let n3 = scan(
            rig,
            &ctx.inputs(b),
            1,
            &[triple(btc(dec!(0), dec!(0.5)), &[robson.clone()], &[])],
            None,
        )
        .await?;
        findings("n3", &n3, &[
            Finding::BaseDeficit { holdings: dec!(0.5), lower: dec!(0.999) },
            Finding::LockedShort {
                locked: dec!(0.5),
                robson: dec!(0.999),
                stops: vec![STOP.to_string()],
            },
        ])?;
    }
    if wanted("n4") {
        // A foreign BTC lock from another pair; its list is ignored.
        let foreign = order("ETHBTC", 77, 5, "foreign-buy", "LIMIT", "BUY", Decimal::ZERO);
        let n4 = scan(
            rig,
            &ctx.inputs(b),
            1,
            &[triple(
                btc(dec!(0), dec!(1)),
                &[robson.clone(), foreign],
                &[list("ETHBTC", 5, "foreign-oco", 77)],
            )],
            None,
        )
        .await?;
        findings("n4", &n4, &[Finding::LockedForeign { locked: dec!(1), robson: dec!(0.999) }])?;
    }
    if wanted("n5") {
        let n5 = scan(
            rig,
            &ctx.inputs(b),
            1,
            &[triple(btc(dec!(0), dec!(0.999)), &[robson.clone()], &[
                list(SYMBOL, 9, "operated-oco", 901),
            ])],
            None,
        )
        .await?;
        let operated = Finding::OperatedOrderList { order_list_id: 9 };
        findings("n5", &n5, std::slice::from_ref(&operated))?;
        ensure!(consequence(&operated) == Consequence::FenceAndPage, "n5: consequence");
    }
    if wanted("n6") {
        // An unlinked operated order: strict recovery, never a sweep.
        let unlinked = order(SYMBOL, 900, -1, "operator-order", "LIMIT", "SELL", Decimal::ZERO);
        let n6 = scan(
            rig,
            &ctx.inputs(b),
            1,
            &[triple(
                btc(dec!(0), dec!(0.999)),
                &[robson.clone(), unlinked],
                &[],
            )],
            None,
        )
        .await?;
        let strict = Finding::UnlinkedOperatedOrder { order_id: 900 };
        findings("n6", &n6, std::slice::from_ref(&strict))?;
        ensure!(consequence(&strict) == Consequence::StrictRecovery, "n6: consequence");
    }
    if wanted("n13") {
        // A UID other than the bound one: pre refuses and nothing is sent.
        let n13 = scan(rig, &ctx.with_uid(b, "1"), 1, &[], None).await?;
        ensure!(
            matches!(n13.verdict, BaseVerdict::Unqualified(UnqualifiedReason::Scope)),
            "n13: {:?}",
            n13.verdict
        );
        costs("n13", &n13, 1, 1, 0, 0)?;
    }
    let claim = |holdings| Finding::WithinClaims {
        holdings,
        lower: dec!(0),
        upper: dec!(10001),
    };
    if wanted("n10") {
        // A closed leg without a write-off record.
        let b = install(pool, &closed, epoch, false).await?;
        let n10 =
            scan(rig, &ctx.inputs(&b), 1, &[triple(btc(dec!(0.999), dec!(0)), &[], &[])], None)
                .await?;
        incomplete("n10", &n10, IncompleteReason::ClosedLegWithoutWriteOff)?;
    }
    if wanted("n11") {
        // A live ledger-backed entry: a claim [0, requested].
        let b = install(pool, &live, epoch, false).await?;
        let entry = order(
            SYMBOL,
            ENTRY,
            -1,
            &live.entry.intent.client_order_id,
            "MARKET",
            "BUY",
            dec!(0.1),
        );
        let n11 =
            scan(rig, &ctx.inputs(&b), 1, &[triple(btc(dec!(0.1), dec!(0)), &[entry], &[])], None)
                .await?;
        findings("n11", &n11, &[claim(dec!(0.1))])?;
    }
    if wanted("n11b") {
        // Sent with no order row, the entry intent uncertain: a claim.
        let b = install(pool, &sent, epoch, false).await?;
        let n11b =
            scan(rig, &ctx.inputs(&b), 1, &[triple(btc(dec!(0), dec!(0)), &[], &[])], None).await?;
        findings("n11b", &n11b, &[claim(dec!(0))])?;
    }
    if wanted("n11c") {
        // An uncertain protection intent is not a claim.
        let b = install(pool, &unprotected, epoch, false).await?;
        let n11c =
            scan(rig, &ctx.inputs(&b), 1, &[triple(btc(dec!(0.999), dec!(0)), &[], &[])], None)
                .await?;
        incomplete("n11c", &n11c, IncompleteReason::UncertainIntent)?;
    }
    if wanted("n11d") {
        // A 'success' entry intent with no order row yet: a claim.
        let b = install(pool, &succeeded, epoch, false).await?;
        let n11d =
            scan(rig, &ctx.inputs(&b), 1, &[triple(btc(dec!(0), dec!(0)), &[], &[])], None).await?;
        findings("n11d", &n11d, &[claim(dec!(0))])?;
    }
    if wanted("n11e") {
        // The EP-1 unfilled terminal entry contributes [0, 0].
        let b = install(pool, &unfilled, epoch, false).await?;
        let n11e =
            scan(rig, &ctx.inputs(&b), 1, &[triple(btc(dec!(0), dec!(0)), &[], &[])], None).await?;
        clean("n11e", &n11e)?;
    }
    if wanted("n14") {
        // A settled cancel and a live ledger-backed replacement stop.
        let b = install(pool, &replaced, epoch, false).await?;
        let n14 = scan(
            rig,
            &ctx.inputs(&b),
            1,
            &[triple(
                btc(dec!(0), dec!(0.999)),
                &[listed(&replacement, STOP + 2, Decimal::ZERO)],
                &[],
            )],
            None,
        )
        .await?;
        clean("n14", &n14)?;
    }
    if wanted("n14b") {
        // An uncertain cancel whose target stop is still live.
        let b = install(pool, &pending_cancel, epoch, false).await?;
        let n14b = scan(
            rig,
            &ctx.inputs(&b),
            1,
            &[triple(
                btc(dec!(0), dec!(0.999)),
                &[listed(&uncertain_stop, STOP, Decimal::ZERO)],
                &[],
            )],
            None,
        )
        .await?;
        incomplete("n14b", &n14b, IncompleteReason::UncertainCancel)?;
    }
    ctx.proxied.close().await;
    println!(
        "RECON_8K2_BASE only={:?} n1_interval={cut:?}",
        std::env::var("RBX_8K2_ONLY").ok()
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn btc_detector_qualification_and_restart(pool: PgPool) -> Result<()> {
    setup_tables(&pool).await?;
    let tenant = Uuid::now_v7();
    let mut rig = ReconciliationRig::open(&pool, tenant).await?;
    let first = qualification_cases(&pool, tenant, &mut rig).await;
    let closed = rig.close().await;
    let restart = first?;
    let reopen = closed?;
    let Some((restarted, before)) = restart else {
        return Ok(());
    };
    // (n12) A restart: a new epoch, owner, budget and pool recompute.
    let fresh = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(pool.connect_options().as_ref().clone())
        .await?;
    let mut rig = ReconciliationRig::reopen(&fresh, tenant, reopen).await?;
    let second = restart_case(&fresh, &restarted, before, &mut rig).await;
    let closed = rig.close().await;
    fresh.close().await;
    second?;
    closed?;
    Ok(())
}

struct Restarted {
    f: Fixture,
    stop: OrderLink,
    stale: JournalGeneration,
}

async fn qualification_cases(
    pool: &PgPool,
    tenant: Uuid,
    rig: &mut ReconciliationRig,
) -> Result<Option<(Restarted, AccountSeen)>> {
    let mut seen = Seen(0);
    let (q, q_stop) = protected(pool, tenant, &mut seen).await?;
    let (r, r_stop) = protected(pool, tenant, &mut seen).await?;
    rig.ready().await?;
    let epoch = rig.start.epoch.epoch_id;
    let ctx = Ctx::new(pool, rig).await?;
    let pending = &ctx.pending;
    let b = install(pool, &q, epoch, true).await?;
    let inputs = ctx.inputs(&b);
    // The projected writer advances the generation with the version.
    let opted = q.ledger(pool).with_native_order_set_projection(b.clone());
    let filled =
        |executed: Decimal| stop(&q, STOP, &q_stop.client_order_id, executed, OrderPhase::Live);
    let answer = |executed: Decimal| {
        triple(btc(dec!(0), dec!(0.999) - executed), &[listed(&q_stop, STOP, executed)], &[])
    };
    // The stop's executed quantity as the ledger and the venue both hold it.
    let mut executed = Decimal::ZERO;

    if wanted("n7") {
        // A stop fill committed between the triple and post.
        let fill = filled(executed + dec!(0.1));
        let write: Hook<'_> = Box::pin(async {
            opted.record_order(&fill, &q_stop, seen.next()).await?;
            Ok(())
        });
        let n7 =
            scan(rig, &inputs, 2, &[answer(executed), answer(executed + dec!(0.1))], Some(write))
                .await?;
        executed += dec!(0.1);
        ensure!(
            n7.attempts[0] == AttemptOutcome::Unqualified(UnqualifiedReason::VersionChanged),
            "n7: {:?}",
            n7.attempts
        );
        clean("n7", &n7)?;
        costs("n7", &n7, 2, 4, 6, 212)?;
    }
    if wanted("nI1") {
        // A record_order commit while held at OPEN_ORDERS.
        rig.settle(2).await?;
        let (listener, held) = Listener::held(
            Instant::now() + Duration::from_secs(60),
            &[answer(executed), answer(executed + dec!(0.1))],
            1,
        )
        .await?;
        let fill = filled(executed + dec!(0.1));
        let at = seen.next();
        let write = timeout(Duration::from_secs(20), async {
            held.seen.await?;
            opted.record_order(&fill, &q_stop, at).await?;
            held.release.send(()).map_err(|_| anyhow!("held body released"))?;
            Ok::<_, anyhow::Error>(())
        });
        let (ni1, wrote) = tokio::join!(rig.scan(&inputs, &listener, None), write);
        listener.finish(Instant::now() + Duration::from_secs(5)).await?;
        let ni1 = ni1?;
        wrote??;
        executed += dec!(0.1);
        ensure!(
            ni1.attempts[0] == AttemptOutcome::Unqualified(UnqualifiedReason::VersionChanged),
            "nI1: {:?}",
            ni1.attempts
        );
        clean("nI1", &ni1)?;
        costs("nI1", &ni1, 2, 4, 6, 212)?;
    }
    if wanted("nI3") {
        // A gate generation bump after the triple, before post.
        let gate = ctx.journal.gate();
        let bump: Hook<'_> = Box::pin(async {
            let _ = gate.record(IncidentReason::UncertainOutcome);
            Ok(())
        });
        let ni3 = scan(rig, &inputs, 2, &[answer(executed), answer(executed)], Some(bump)).await?;
        ensure!(
            ni3.attempts[0] == AttemptOutcome::Unqualified(UnqualifiedReason::GenerationChanged),
            "nI3: {:?}",
            ni3.attempts
        );
        clean("nI3", &ni3)?;
        costs("nI3", &ni3, 2, 4, 6, 212)?;
    }
    if wanted("n9") {
        // A clock fault between the triple and post fails the revalidated
        // ACCOUNT anchor; a stale sample refuses before any request or post.
        let trip = rig.clock_trip()?;
        let fault: Hook<'_> = Box::pin(async move { trip.trip() });
        let n9 = scan(rig, &inputs, 2, &[answer(executed), answer(executed)], Some(fault)).await?;
        ensure!(
            n9.attempts[0] == AttemptOutcome::Unqualified(UnqualifiedReason::Clock),
            "n9: {:?}",
            n9.attempts
        );
        clean("n9-fault", &n9)?;
        costs("n9-fault", &n9, 2, 4, 6, 212)?;
        rig.stale().await?;
        let listener = Listener::triples(Instant::now() + Duration::from_secs(60), &[]).await?;
        let stale = rig.scan(&inputs, &listener, None).await;
        listener.finish(Instant::now() + Duration::from_secs(5)).await?;
        let stale = stale?;
        ensure!(
            matches!(stale.verdict, BaseVerdict::Unqualified(UnqualifiedReason::Clock)),
            "n9: {:?}",
            stale.verdict
        );
        costs("n9-stale", &stale, 3, 3, 0, 0)?;
    }
    if wanted("n8") {
        // Pending execution evidence before the scan.
        let evidence = rig.evidence(executed + dec!(0.1)).await?;
        let PendingExecutionRegistration::Issued(ticket) =
            pending.register(evidence, Arc::new(filled(executed + dec!(0.1))))
        else {
            return Err(anyhow!("pending evidence not issued"));
        };
        let n8 =
            scan(rig, &inputs, 3, &[answer(executed), answer(executed), answer(executed)], None)
                .await?;
        ensure!(
            matches!(n8.verdict, BaseVerdict::Unqualified(UnqualifiedReason::PendingEvidence)),
            "n8: {:?}",
            n8.verdict
        );
        costs("n8", &n8, 3, 6, 9, 318)?;
        ensure!(pending.acknowledge(ticket) && pending.pending().is_none(), "n8: acknowledged");
    }
    if wanted("nI2") {
        // Pending evidence registered while held at OPEN_ORDER_LIST.
        rig.settle(3).await?;
        let (listener, held) = Listener::held(
            Instant::now() + Duration::from_secs(60),
            &[answer(executed), answer(executed), answer(executed)],
            2,
        )
        .await?;
        let evidence = rig.evidence(executed + dec!(0.1)).await?;
        let record = Arc::new(filled(executed + dec!(0.1)));
        let register = timeout(Duration::from_secs(20), async {
            held.seen.await?;
            let registered = pending.register(evidence, record);
            held.release.send(()).map_err(|_| anyhow!("held body released"))?;
            Ok::<_, anyhow::Error>(registered)
        });
        let (ni2, registered) = tokio::join!(rig.scan(&inputs, &listener, None), register);
        listener.finish(Instant::now() + Duration::from_secs(5)).await?;
        let ni2 = ni2?;
        let PendingExecutionRegistration::Issued(ticket) = registered?? else {
            return Err(anyhow!("held pending evidence not issued"));
        };
        ensure!(
            ni2.attempts == [AttemptOutcome::Unqualified(UnqualifiedReason::PendingEvidence); 3],
            "nI2: {:?}",
            ni2.attempts
        );
        costs("nI2", &ni2, 3, 6, 9, 318)?;
        ensure!(pending.acknowledge(ticket) && pending.pending().is_none(), "nI2: acknowledged");
    }
    if wanted("nI4") {
        // A ledger write after post leaves the attempt qualified; it is late
        // evidence, begun after the interval end.
        let ni4 = scan(rig, &inputs, 1, &[answer(executed)], None).await?;
        clean("nI4", &ni4)?;
        let end = interval(&ni4)?.end_ns;
        let wall = || {
            let now = chrono::Utc::now();
            i128::from(now.timestamp()) * 1_000_000_000 + i128::from(now.timestamp_subsec_nanos())
        };
        // I ends u after the ACCOUNT receive: the write starts once it has.
        if let Ok(rest) = u64::try_from(end - wall()) {
            tokio::time::sleep(Duration::from_nanos(rest) + Duration::from_millis(1)).await;
        }
        let begun_ns = wall();
        opted.record_order(&filled(executed + dec!(0.1)), &q_stop, seen.next()).await?;
        ensure!(begun_ns > end, "nI4: write began at {begun_ns}, interval end {end}");
        println!("RECON_8K2 case=nI4 interval_end_ns={end} write_begun_ns={begun_ns}");
    }
    let restart = if wanted("n12") {
        // The restart scope, scanned on this epoch first.
        let stale = install(pool, &r, epoch, false).await?;
        let before = scan(
            rig,
            &ctx.inputs(&stale),
            1,
            &[triple(
                btc(dec!(0), dec!(0.999)),
                &[listed(&r_stop, STOP, Decimal::ZERO)],
                &[],
            )],
            None,
        )
        .await?;
        clean("n12-before", &before)?;
        let seen = before.account.context("n12 ACCOUNT receipt before")?;
        Some((Restarted { f: r, stop: r_stop, stale }, seen))
    } else {
        None
    };
    ctx.proxied.close().await;
    Ok(restart)
}

async fn restart_case(
    pool: &PgPool,
    r: &Restarted,
    before: AccountSeen,
    rig: &mut ReconciliationRig,
) -> Result<()> {
    rig.ready().await?;
    let ctx = Ctx::new(pool, rig).await?;
    // The old binding names a superseded epoch: refused, nothing reused.
    let refused = scan(rig, &ctx.inputs(&r.stale), 1, &[], None).await?;
    ensure!(
        matches!(refused.verdict, BaseVerdict::Unqualified(UnqualifiedReason::Scope)),
        "n12: {:?}",
        refused.verdict
    );
    let b = install(pool, &r.f, rig.start.epoch.epoch_id, false).await?;
    let after = scan(
        rig,
        &ctx.inputs(&b),
        1,
        &[triple(
            btc(dec!(0), dec!(0.999)),
            &[listed(&r.stop, STOP, Decimal::ZERO)],
            &[],
        )],
        None,
    )
    .await?;
    clean("n12", &after)?;
    costs("n12", &after, 1, 2, 3, 106)?;
    let account = after.account.context("n12 ACCOUNT receipt")?;
    let recomputed = interval(&after)?;
    // Captures carry the new epoch and follow the old receipt; the widened
    // intervals may overlap, the observations do not.
    ensure!(
        account.send.epoch == rig.start.epoch.epoch_id
            && before.send.epoch == r.stale.process_epoch
            && account.send.epoch != before.send.epoch,
        "n12: epochs {} after {}",
        before.send.epoch,
        account.send.epoch
    );
    ensure!(
        wall(&account.send) > wall(&before.receive),
        "n12: the new ACCOUNT request follows the old receipt"
    );
    ctx.proxied.close().await;
    println!(
        "RECON_8K2 case=n12 epoch={} before_receive_ns={} after_send_ns={} after_start_ns={}",
        rig.start.epoch.epoch_id,
        wall(&before.receive),
        wall(&account.send),
        recomputed.start_ns
    );
    Ok(())
}

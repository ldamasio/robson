use super::{
    audit, batch,
    fixture::{backend_pid, retired, through_proxy_limit, unowned},
    linked_sell,
    reader::{Report, Scope},
    scope, seen, zero_execution_order, Fixture, VENUE,
};
use robson_exec::{
    order_journal::{AttemptRow, IntentKind, JournalBackend, OutcomeRow, VenueOutcome},
    ports::TradeFill,
    venue::{OrderPhase, ProtectionKind, TradePage},
    venue_ledger::{LedgerError, LegOrderKind, OrderKey, OrderOwner},
};
use robson_testkit::db_proxy::ProxyMode;
use robsond::{order_journal_store::PgJournalBackend, venue_ledger_store::PgVenueLedger};
use rust_decimal::Decimal;
use sqlx::{postgres::PgArguments, query::QueryAs, PgPool, Postgres};
use uuid::Uuid;

#[path = "../venue_ledger_schema/empty_budget.rs"]
mod empty_budget;

const HOT: &str = include_str!("bounded_order_set.sql");

#[derive(Debug, sqlx::FromRow)]
struct Hot {
    position_id: Uuid,
    accounting_version: i64,
    gross_qty: Option<Decimal>,
    acquisition_cost: Option<Decimal>,
    base_commission: Option<Decimal>,
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
    stop_id: Option<String>,
    stop_phase: Option<String>,
    stop_status: Option<String>,
    exact_stop_identity: bool,
}

fn hot_query<'a>(s: Scope, candidate: Option<&'a str>) -> QueryAs<'a, Postgres, Hot, PgArguments> {
    sqlx::query_as(HOT)
        .bind(s.tenant)
        .bind(s.venue)
        .bind(s.account)
        .bind(s.position)
        .bind(s.symbol)
        .bind(candidate)
}

fn terminal(phase: Option<&str>, status: Option<&str>) -> bool {
    phase == Some("terminal")
        && matches!(
            status,
            Some("FILLED" | "CANCELED" | "EXPIRED" | "EXPIRED_IN_MATCH" | "REJECTED")
        )
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Liability {
    active: i64,
    conflicts: i64,
}

impl Liability {
    fn observe(&mut self, active: bool, conflict: bool) {
        self.active += i64::from(active);
        self.conflicts += i64::from(conflict);
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
struct Sells {
    leg: Liability,
    symbol: Liability,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct Cancels {
    grouped: Liability,
    ungroupable: Liability,
    unbound: Liability,
}

impl Hot {
    fn sells(&self) -> Sells {
        Sells {
            leg: Liability {
                active: self.leg_sell,
                conflicts: self.leg_sell_conflicts,
            },
            symbol: Liability {
                active: self.symbol_sell,
                conflicts: self.symbol_sell_conflicts,
            },
        }
    }

    fn cancels(&self) -> Cancels {
        Cancels {
            grouped: Liability {
                active: self.grouped_cancel,
                conflicts: self.grouped_cancel_conflicts,
            },
            ungroupable: Liability {
                active: self.ungroupable_cancel,
                conflicts: self.ungroupable_cancel_conflicts,
            },
            unbound: Liability {
                active: self.unbound_cancel,
                conflicts: self.unbound_cancel_conflicts,
            },
        }
    }
}

fn cold_sells(report: &Report) -> Sells {
    let mut counts = Sells::default();
    let mut handled = Vec::new();
    for row in report
        .rows
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "exit" | "protection"))
    {
        let exact_sell = row.named_identity_matches && row.named_side.as_deref() == Some("SELL");
        if exact_sell {
            handled.push(row.named_id.as_deref().unwrap());
        }
        let contradictory_outcome =
            row.not_sent || row.venue_outcome.as_deref() == Some("rejected");
        let proved_terminal = exact_sell
            && !contradictory_outcome
            && terminal(row.named_phase.as_deref(), row.named_status.as_deref());
        if row.no_send || row.has_durable_rejection() || proved_terminal {
            continue;
        }
        // An exact pair is one member. Mismatches stay on both native and journal paths.
        let conflict = contradictory_outcome
            || (row.named_id.is_some() && !exact_sell)
            || row.named_phase.as_deref() == Some("terminal");
        counts.leg.observe(true, conflict);
    }
    for row in &report.venue_complement {
        if handled.contains(&row.exchange_order_id.as_str()) {
            continue;
        }
        if row.side != "SELL"
            && !report.rows.iter().any(|journal| {
                Some(journal.intent_id) == row.intent_id
                    && matches!(journal.kind.as_str(), "exit" | "protection")
            })
        {
            continue;
        }
        // No entry in the complement supplies exact terminal evidence for a SELL.
        // Owning-leg complements carry missing or contradictory linkage. An
        // unowned observation is an active hold even without a contradiction.
        if row.position_id.is_some() {
            counts.leg.observe(true, true);
        } else {
            counts.symbol.observe(true, row.phase == "terminal");
        }
    }
    counts
}

fn cancel_conflict(named: bool, not_sent: bool, has_outcome: bool, has_evidence: bool) -> bool {
    named || (not_sent && (has_outcome || has_evidence))
}

fn cold_cancels(report: &Report) -> Cancels {
    let mut counts = Cancels::default();
    for row in report.rows.iter().filter(|row| row.kind == "cancel") {
        let contradiction = cancel_conflict(
            row.named_id.is_some(),
            row.not_sent,
            row.venue_outcome.is_some(),
            row.venue_evidence.is_some(),
        );
        if row.original_target.is_none() {
            counts.ungroupable.observe(!row.no_send, contradiction);
            continue;
        }
        let exact_terminal = !row.target_conflict
            && row.target_id == row.original_target
            && terminal(row.target_phase.as_deref(), row.target_status.as_deref());
        counts.grouped.observe(!row.no_send && !exact_terminal, contradiction);
    }
    for row in &report.unbound_cancels {
        counts.unbound.observe(
            !row.no_send,
            cancel_conflict(
                row.named_id.is_some(),
                row.not_sent,
                row.venue_outcome.is_some(),
                row.venue_evidence.is_some(),
            ),
        );
    }
    counts
}

async fn checkpoint(
    pool: &PgPool,
    s: Scope,
    label: &str,
    expected_sells: (i64, i64),
    candidate: Option<&str>,
) -> Hot {
    let hot = hot_query(s, candidate).fetch_one(pool).await.unwrap();
    let cold = audit(pool, s).await.unwrap();
    assert_eq!(hot.position_id, s.position, "{label}");
    assert_eq!(hot.accounting_version, cold.version, "{label}: accounting version");
    let sells = cold_sells(&cold);
    let cancels = cold_cancels(&cold);
    assert_eq!((sells.leg.active, sells.symbol.active), expected_sells, "{label}: cold SELL");
    assert_eq!(hot.sells(), sells, "{label}: SELL liabilities");
    assert_eq!(hot.cancels(), cancels, "{label}: cancellation liabilities");
    println!(
        "order-set event={label} version={} sells={sells:?} cancels={cancels:?}",
        hot.accounting_version
    );
    hot
}

async fn install(pool: &PgPool) {
    sqlx::raw_sql(include_str!("cancel_shadow.sql")).execute(pool).await.unwrap();
    sqlx::raw_sql(include_str!("sell_shadow.sql")).execute(pool).await.unwrap();
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn mismatched_terminal_links_preserve_each_scoped_sell_member(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let sibling = Fixture::for_tenant(&pool, f.tenant).await;
    let s = scope(&f);
    let sibling_scope = scope(&sibling);
    let journal = PgJournalBackend::new(&pool, s.tenant);
    install(&pool).await;

    let original = batch(s, IntentKind::Exit, None);
    journal.commit(&original).await.unwrap();
    checkpoint(&pool, s, "unresolved-own-journal", (1, 0), None).await;

    let wrong_owner = linked_sell(&sibling, &original.intent);
    let mut observed = zero_execution_order("301", "CANCELED", OrderPhase::Terminal);
    observed.client_order_id = wrong_owner.client_order_id.clone();
    assert!(
        sibling
            .ledger
            .record_order(&observed, &wrong_owner, seen(1))
            .await
            .unwrap()
            .changed
    );
    let original_hold =
        checkpoint(&pool, s, "foreign-leg-terminal-retains-journal", (1, 0), None).await;
    assert_eq!(original_hold.leg_sell_conflicts, 1);
    let sibling_hold =
        checkpoint(&pool, sibling_scope, "native-terminal-conflict", (1, 0), None).await;
    assert_eq!(sibling_hold.leg_sell_conflicts, 1);

    let wrong_kind = batch(s, IntentKind::Protection, None);
    journal.commit(&wrong_kind).await.unwrap();
    let link = linked_sell(&f, &wrong_kind.intent);
    observed.exchange_order_id = "302".to_owned();
    observed.client_order_id = link.client_order_id.clone();
    assert!(f.ledger.record_order(&observed, &link, seen(2)).await.unwrap().changed);
    let result = checkpoint(&pool, s, "wrong-kind-is-two-unresolved-members", (3, 0), None).await;
    assert_eq!(result.leg_sell_conflicts, 3);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn sell_projection_matches_cold_after_flagged_and_native_writes(pool: PgPool) {
    empty_budget::initialize(&pool, seen(0).at).await.unwrap();
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let journal = PgJournalBackend::new(&pool, s.tenant);
    // The only preexisting intent is the fixture's entry. This fixture does
    // not test startup backfill for source rows which can hold SELL/cancel.
    install(&pool).await;

    let initial = checkpoint(&pool, s, "empty", (0, 0), None).await;
    assert_eq!(
        (
            initial.gross_qty,
            initial.acquisition_cost,
            initial.base_commission,
            initial.executable_qty
        ),
        (None, None, None, None)
    );
    assert_eq!(initial.consumed_cum, Decimal::ZERO);
    assert_eq!(initial.target_trigger, Decimal::from(90));

    let mut rejected = batch(s, IntentKind::Exit, None);
    journal.commit(&rejected).await.unwrap();
    checkpoint(&pool, s, "journal-before-attempt", (1, 0), None).await;

    rejected.outcome = Some(OutcomeRow {
        venue_outcome: Some(VenueOutcome::Rejected),
        ..OutcomeRow::default()
    });
    journal.commit(&rejected).await.unwrap();
    checkpoint(&pool, s, "bare-rejection", (1, 0), None).await;

    rejected.attempts.push(AttemptRow {
        attempt_id: Uuid::now_v7(),
        created_at: seen(1).at,
        rejected_at: None,
    });
    journal.commit(&rejected).await.unwrap();
    checkpoint(&pool, s, "rejection-with-started-attempt", (1, 0), None).await;

    // Outcome already persisted, so only the changed attempt can reconcile
    // the liability. An equal replay cannot subtract it a second time.
    rejected.attempts[0].rejected_at = Some(seen(2).at);
    for label in ["durable-exact-rejection", "equal-rejection-replay"] {
        journal.commit(&rejected).await.unwrap();
        checkpoint(&pool, s, label, (0, 0), None).await;
    }

    let rejected_link = linked_sell(&f, &rejected.intent);
    let mut contradicted = zero_execution_order("101", "NEW", OrderPhase::Live);
    contradicted.client_order_id = rejected_link.client_order_id.clone();
    assert!(
        f.ledger
            .record_order(&contradicted, &rejected_link, seen(3))
            .await
            .unwrap()
            .changed
    );
    let hot = checkpoint(&pool, s, "rejection-with-exact-live-order", (1, 0), None).await;
    assert_eq!(hot.leg_sell_conflicts, 1, "exact contradictory pair is one member");

    let mut no_send = batch(s, IntentKind::Protection, None);
    no_send.attempts.push(AttemptRow {
        attempt_id: Uuid::now_v7(),
        created_at: seen(3).at,
        rejected_at: None,
    });
    journal.commit(&no_send).await.unwrap();
    checkpoint(&pool, s, "started-before-no-send", (2, 0), None).await;
    no_send.outcome = Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() });
    journal.commit(&no_send).await.unwrap();
    checkpoint(&pool, s, "positive-no-send-with-started-attempt", (1, 0), None).await;

    let adopted = batch(s, IntentKind::Exit, None);
    let link = linked_sell(&f, &adopted.intent);
    let mut discovered_link = unowned();
    discovered_link.client_order_id = link.client_order_id.clone();
    let mut order = zero_execution_order("201", "NEW", OrderPhase::Live);
    order.client_order_id = link.client_order_id.clone();

    assert!(f.ledger.record_order(&order, &discovered_link, seen(4)).await.unwrap().changed);
    checkpoint(&pool, s, "unowned-native-sell-before-journal", (1, 1), None).await;
    journal.commit(&adopted).await.unwrap();
    checkpoint(&pool, s, "journal-before-adoption", (2, 1), None).await;
    assert!(f.ledger.record_order(&order, &link, seen(5)).await.unwrap().changed);
    let adopted_hot = checkpoint(&pool, s, "exact-adoption-moves-symbol-hold", (2, 0), None).await;

    let key = OrderKey::from_record(&order, &link).unwrap();
    let fill = TradeFill {
        trade_id: "41".into(),
        price: Decimal::from(100),
        qty: Decimal::from(2),
        quote_qty: Decimal::from(200),
        commission: Decimal::ZERO,
        commission_asset: "USDT".into(),
        time: Some(seen(6).at),
    };
    let fills = TradePage { trades: vec![fill], next_from_id: None };
    assert!(f.ledger.record_trades(&key, &fills, seen(6).at).await.unwrap().changed);
    let after_fill = checkpoint(&pool, s, "trade-writes-final-consumption", (2, 0), None).await;
    assert_eq!(after_fill.accounting_version, adopted_hot.accounting_version + 1);
    assert_eq!(after_fill.consumed_cum, Decimal::from(2));
    assert_eq!(after_fill.executable_qty, None);
    assert!(!f.ledger.record_trades(&key, &fills, seen(6).at).await.unwrap().changed);
    let replay = checkpoint(&pool, s, "trade-replay", (2, 0), None).await;
    assert_eq!(replay.accounting_version, after_fill.accounting_version);

    let target_cancel = batch(s, IntentKind::Cancel, Some("201"));
    journal.commit(&target_cancel).await.unwrap();
    let queued = checkpoint(&pool, s, "exact-cancel-pending", (2, 0), None).await;
    assert_eq!(queued.grouped_cancel, 1);
    let invalid_target = batch(s, IntentKind::Cancel, Some("not-a-canonical-id"));
    journal.commit(&invalid_target).await.unwrap();
    let invalid = checkpoint(&pool, s, "known-leg-cancel-invalid-target", (2, 0), None).await;
    assert_eq!(invalid.ungroupable_cancel, 1);
    let mut orphan = batch(s, IntentKind::Cancel, Some("201"));
    orphan.intent.position_id = None;
    journal.commit(&orphan).await.unwrap();
    let unbound = checkpoint(&pool, s, "account-cancel-without-position", (2, 0), None).await;
    assert_eq!(unbound.unbound_cancel, 1);

    order.status = "CANCELED".to_owned();
    order.phase = OrderPhase::Terminal;
    order.executed_qty = Decimal::from(2);
    order.cumulative_quote_qty = Some(Decimal::from(200));
    sqlx::query("INSERT INTO cancel_shadow_fail_terminal VALUES('201')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(f.ledger.record_order(&order, &link, seen(7)).await.is_err());
    let rolled_back = checkpoint(&pool, s, "native-and-projection-rollback", (2, 0), None).await;
    assert_eq!(rolled_back.accounting_version, unbound.accounting_version);
    assert_eq!(rolled_back.grouped_cancel, 1);
    sqlx::query("DELETE FROM cancel_shadow_fail_terminal WHERE target_id='201'")
        .execute(&pool)
        .await
        .unwrap();

    let (proxy, proxied) = through_proxy_limit(&pool, 1).await;
    let pid = backend_pid(&proxied).await;
    let proxied_ledger = PgVenueLedger::new(&proxied, s.tenant);
    proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE venue_orders"));
    assert!(matches!(
        proxied_ledger.record_order(&order, &link, seen(8)).await,
        Err(LedgerError::Uncertain(_))
    ));
    retired(&pool, pid).await;
    assert_eq!(proxy.stats().commit_acks_dropped, 1);
    let terminal = checkpoint(&pool, s, "lost-ack-positive-terminal-fate", (1, 0), None).await;
    assert_eq!(terminal.accounting_version, unbound.accounting_version + 1);
    assert_eq!(terminal.grouped_cancel, 0);
    assert_eq!((terminal.ungroupable_cancel, terminal.unbound_cancel), (1, 1));
    assert!(!f.ledger.record_order(&order, &link, seen(9)).await.unwrap().changed);
    let replay = checkpoint(&pool, s, "terminal-replay", (1, 0), None).await;
    assert_eq!(replay.accounting_version, terminal.accounting_version);

    let protection = batch(s, IntentKind::Protection, None);
    journal.commit(&protection).await.unwrap();
    checkpoint(&pool, s, "protection-intent", (2, 0), None).await;
    let mut stop_link = linked_sell(&f, &protection.intent);
    stop_link.owner = OrderOwner::Leg {
        position_id: s.position,
        intent_id: protection.intent.intent_id,
        kind: LegOrderKind::Stop,
    };
    let mut stop = zero_execution_order("202", "NEW", OrderPhase::Live);
    stop.client_order_id = stop_link.client_order_id.clone();
    stop.order_type = "STOP_LOSS".to_owned();
    stop.protection_kind = Some(ProtectionKind::MarketOnTrigger);
    stop.stop_price = Some(Decimal::from(90));
    assert!(f.ledger.record_order(&stop, &stop_link, seen(10)).await.unwrap().changed);

    let exact = checkpoint(&pool, s, "explicit-stop-key", (2, 0), Some("202")).await;
    assert_eq!(exact.stop_id.as_deref(), Some("202"));
    assert_eq!(exact.stop_phase.as_deref(), Some("live"));
    assert_eq!(exact.stop_status.as_deref(), Some("NEW"));
    assert!(exact.exact_stop_identity);
    let decoy = checkpoint(&pool, s, "explicit-non-stop-key", (2, 0), Some("201")).await;
    assert_eq!(decoy.stop_id.as_deref(), Some("201"));
    assert!(!decoy.exact_stop_identity);

    proxy.set_mode(ProxyMode::Pass);
    let mut connection = proxied.acquire().await.unwrap();
    proxy.enable_exact_query_trace(4);
    let start = proxy.stats().statements;
    let absent = hot_query(s, Some("999999")).fetch_one(&mut *connection).await.unwrap();
    assert!(absent.stop_id.is_none() && !absent.exact_stop_identity);
    assert_eq!(proxy.stats().statements - start, 1);
    assert_eq!(proxy.query_trace().len(), 1);
    assert_eq!(proxy.query_trace_dropped(), 0);
    let wrong = Scope { symbol: "ETHUSDT", ..s };
    assert!(hot_query(wrong, Some("202"))
        .fetch_optional(&mut *connection)
        .await
        .unwrap()
        .is_none());
    assert_eq!(proxy.stats().statements - start, 2);
    assert_eq!(proxy.query_trace().len(), 2);
    assert_eq!(proxy.query_trace_dropped(), 0);
    println!("order-set exact/absent controls execute one statement each");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn terminal_masks_active_cancel_but_preserves_conflict_in_every_scope(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let sibling = Fixture::for_tenant(&pool, f.tenant).await;
    let s = scope(&f);
    let other = scope(&sibling);
    let journal = PgJournalBackend::new(&pool, s.tenant);
    install(&pool).await;
    checkpoint(&pool, s, "before-conflicts", (0, 0), None).await;

    let contradictory = OutcomeRow {
        not_sent: true,
        venue_evidence: Some(serde_json::json!({"opaque_fixture_evidence": "retained"})),
        ..OutcomeRow::default()
    };
    for (label, target, bound) in [
        ("grouped-contradicted-no-send", Some("701"), true),
        ("ungroupable-contradicted-no-send", Some("invalid-id"), true),
        ("unbound-contradicted-no-send", Some("701"), false),
    ] {
        let mut cancel = batch(s, IntentKind::Cancel, target);
        cancel.intent.position_id = bound.then_some(s.position);
        cancel.outcome = Some(contradictory.clone());
        journal.commit(&cancel).await.unwrap();
        checkpoint(&pool, s, label, (0, 0), None).await;
    }
    let all_conflicting = Liability { active: 1, conflicts: 1 };
    let live = checkpoint(&pool, s, "all-cancel-channels-positive", (0, 0), None).await;
    assert_eq!(
        live.cancels(),
        Cancels {
            grouped: all_conflicting,
            ungroupable: all_conflicting,
            unbound: all_conflicting
        }
    );

    let original = batch(s, IntentKind::Exit, None);
    journal.commit(&original).await.unwrap();
    checkpoint(&pool, s, "conflicting-cancel-target-journal", (1, 0), None).await;
    let link = linked_sell(&f, &original.intent);
    let mut own_terminal = zero_execution_order("701", "CANCELED", OrderPhase::Terminal);
    own_terminal.client_order_id = link.client_order_id.clone();
    assert!(f.ledger.record_order(&own_terminal, &link, seen(1)).await.unwrap().changed);
    let masked = checkpoint(&pool, s, "terminal-masks-active-retains-conflict", (0, 0), None).await;
    assert_eq!(
        masked.cancels(),
        Cancels {
            grouped: Liability { active: 0, conflicts: 1 },
            ungroupable: all_conflicting,
            unbound: all_conflicting,
        }
    );

    // Even recognized terminal evidence on an unowned SELL cannot be
    // attributed to either leg. Its unresolved conflict is shared by symbol.
    let unowned_terminal = zero_execution_order("702", "CANCELED", OrderPhase::Terminal);
    assert!(
        f.ledger
            .record_order(&unowned_terminal, &unowned(), seen(2))
            .await
            .unwrap()
            .changed
    );
    let native = checkpoint(&pool, s, "symbol-terminal-conflict", (0, 1), None).await;
    assert_eq!(native.sells().symbol, all_conflicting);
    let sibling_hot =
        checkpoint(&pool, other, "sibling-shares-account-and-symbol", (0, 1), None).await;
    assert_eq!(sibling_hot.sells().symbol, all_conflicting);
    assert_eq!(
        sibling_hot.cancels(),
        Cancels {
            unbound: all_conflicting,
            ..Cancels::default()
        }
    );
}

#[path = "sell_scale_tests.rs"]
mod sell_scale_tests;

#[path = "sell_nested_work_tests.rs"]
mod sell_nested_work_tests;

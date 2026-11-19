use super::{
    audit, batch,
    fixture::{backend_pid, retired, through_proxy_limit, unowned},
    linked_sell,
    reader::Scope,
    scope, seen, zero_execution_order, Fixture,
};
use robson_exec::{
    order_journal::{IntentKind, JournalBackend, JournalBatch, OutcomeRow, VenueOutcome},
    venue::OrderPhase,
    venue_ledger::LedgerError,
};
use robson_testkit::db_proxy::ProxyMode;
use robsond::{order_journal_store::PgJournalBackend, venue_ledger_store::PgVenueLedger};
use sqlx::PgPool;
use uuid::Uuid;

async fn install(pool: &PgPool) {
    sqlx::raw_sql(include_str!("cancel_shadow.sql")).execute(pool).await.unwrap();
}

async fn pending_cancels(
    journal: &PgJournalBackend,
    scope: Scope,
    target: &str,
    count: usize,
) -> Vec<JournalBatch> {
    let mut cancels = Vec::with_capacity(count);
    for _ in 0..count {
        let cancel = batch(scope, IntentKind::Cancel, Some(target));
        journal.commit(&cancel).await.unwrap();
        cancels.push(cancel);
    }
    cancels
}

async fn group(pool: &PgPool, s: Scope, target: &str) -> (i64, i64, bool) {
    sqlx::query_as(
        "SELECT pending_count, conflict_count, terminal_mask FROM cancel_shadow_groups
         WHERE tenant_id=$1 AND venue_id=$2 AND account_scope=$3 AND symbol=$4
           AND position_id=$5 AND target_id=$6",
    )
    .bind(s.tenant)
    .bind(s.venue)
    .bind(s.account)
    .bind(s.symbol)
    .bind(s.position)
    .bind(target)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn members(pool: &PgPool, s: Scope, target: &str) -> Vec<(Uuid, bool, bool)> {
    sqlx::query_as(
        "SELECT intent_id, pending, conflict FROM cancel_shadow_members
         WHERE tenant_id=$1 AND venue_id=$2 AND account_scope=$3 AND symbol=$4
           AND position_id=$5 AND target_id=$6 ORDER BY intent_id",
    )
    .bind(s.tenant)
    .bind(s.venue)
    .bind(s.account)
    .bind(s.symbol)
    .bind(s.position)
    .bind(target)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn terminal_work(pool: &PgPool, s: Scope, target: &str) -> Vec<(i64, i64, i64)> {
    sqlx::query_as(
        "SELECT group_rows, leg_rows, member_rows FROM cancel_shadow_terminal_work
         WHERE tenant_id=$1 AND venue_id=$2 AND account_scope=$3 AND symbol=$4
           AND position_id=$5 AND target_id=$6",
    )
    .bind(s.tenant)
    .bind(s.venue)
    .bind(s.account)
    .bind(s.symbol)
    .bind(s.position)
    .bind(target)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn assert_leg(pool: &PgPool, s: Scope, expected: (i64, i64)) {
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT active_count, conflict_count FROM cancel_shadow_legs
         WHERE tenant_id=$1 AND venue_id=$2 AND account_scope=$3 AND symbol=$4
           AND position_id=$5",
    )
    .bind(s.tenant)
    .bind(s.venue)
    .bind(s.account)
    .bind(s.symbol)
    .bind(s.position)
    .fetch_one(pool)
    .await
    .unwrap();
    let reference = audit(pool, s).await.unwrap();
    let mut active = 0;
    let mut conflicts = 0;
    for row in reference.rows.iter().filter(|row| row.kind == "cancel") {
        let exact_terminal = !row.target_conflict
            && row.original_target.is_some()
            && row.target_id == row.original_target
            && row.target_phase.as_deref() == Some("terminal")
            && matches!(
                row.target_status.as_deref(),
                Some("FILLED" | "CANCELED" | "EXPIRED" | "EXPIRED_IN_MATCH" | "REJECTED")
            );
        active += i64::from(!row.no_send && !exact_terminal);
        conflicts += i64::from(row.named_conflict);
    }
    assert_eq!(counts, expected, "shadow counts for leg {}", s.position);
    assert_eq!((active, conflicts), expected, "cold counts for leg {}", s.position);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cancel_shadow_masks_exact_target_with_bounded_native_row_work(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let sibling = Fixture::for_tenant(&pool, f.tenant).await;
    let s = scope(&f);
    let other = scope(&sibling);
    let journal = PgJournalBackend::new(&pool, s.tenant);
    install(&pool).await;

    pending_cancels(&journal, other, "300", 1).await;
    assert_leg(&pool, other, (1, 0)).await;

    for (target, count) in [("100", 2), ("200", 48)] {
        let mut cancels = pending_cancels(&journal, s, target, count).await;
        let expected = i64::try_from(count).unwrap();
        assert_eq!(group(&pool, s, target).await, (expected, 0, false));
        assert_leg(&pool, s, (expected, 0)).await;

        journal.commit(&cancels[0]).await.unwrap();
        assert_eq!(group(&pool, s, target).await, (expected, 0, false));

        // A positively recorded no-send removes this intent only. Replay must
        // leave the adjustment idempotent.
        cancels[0].outcome = Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() });
        for _ in 0..2 {
            journal.commit(&cancels[0]).await.unwrap();
            assert_eq!(group(&pool, s, target).await, (expected - 1, 0, false));
            assert_leg(&pool, s, (expected - 1, 0)).await;
        }
        let before = members(&pool, s, target).await;
        assert_eq!(before.len(), count);
        assert!(before.contains(&(cancels[0].intent.intent_id, false, false)));
        assert!(before.contains(&(cancels[1].intent.intent_id, true, false)));

        let original = batch(s, IntentKind::Exit, None);
        journal.commit(&original).await.unwrap();
        let link = linked_sell(&f, &original.intent);
        let mut terminal = zero_execution_order(target, "CANCELED", OrderPhase::Terminal);
        terminal.client_order_id = link.client_order_id.clone();
        assert!(f.ledger.record_order(&terminal, &link, seen(1)).await.unwrap().changed);

        assert_eq!(group(&pool, s, target).await, (expected - 1, 0, true));
        assert_leg(&pool, s, (0, 0)).await;
        assert_leg(&pool, other, (1, 0)).await;
        assert_eq!(members(&pool, s, target).await, before);
        let work = terminal_work(&pool, s, target).await;
        assert_eq!(work, [(1, 1, 0)], "exact group, leg and member row work");
        println!("Cancel target with {count} members, terminal row work {work:?}");

        assert!(!f.ledger.record_order(&terminal, &link, seen(2)).await.unwrap().changed);
        assert_eq!(terminal_work(&pool, s, target).await, work);
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cancel_shadow_keeps_wrong_targets_and_contradictions_unresolved(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let sibling = Fixture::for_tenant(&pool, f.tenant).await;
    let s = scope(&f);
    let other = scope(&sibling);
    let journal = PgJournalBackend::new(&pool, s.tenant);

    let mut historical = batch(s, IntentKind::Cancel, Some("100"));
    historical.intent.position_id = None;
    journal.commit(&historical).await.unwrap();
    install(&pool).await;

    // These four members are distinct liabilities under one leg.
    for target in ["100", "200", "300", "400"] {
        pending_cancels(&journal, s, target, 1).await;
    }
    let mut current_unbound = batch(s, IntentKind::Cancel, Some("100"));
    current_unbound.intent.position_id = None;
    journal.commit(&current_unbound).await.unwrap();

    let unbound: (Option<Uuid>, Option<String>, bool) = sqlx::query_as(
        "SELECT position_id, symbol, pending FROM cancel_shadow_members WHERE intent_id=$1",
    )
    .bind(current_unbound.intent.intent_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unbound, (None, None, true));
    assert!(audit(&pool, s).await.unwrap().ambiguous_unbound);

    // 100 matches the venue key but lacks the target's own journal row.
    let missing_link = f.link(robson_exec::venue_ledger::LegOrderKind::Sell);
    let missing = zero_execution_order("100", "CANCELED", OrderPhase::Terminal);
    f.ledger.record_order(&missing, &missing_link, seen(1)).await.unwrap();

    // 200 has the same numeric order ID on a different symbol.
    let mut another_symbol = zero_execution_order("200", "CANCELED", OrderPhase::Terminal);
    another_symbol.symbol = "ETHUSDT".into();
    f.ledger.record_order(&another_symbol, &unowned(), seen(1)).await.unwrap();

    // 300 has a valid terminal linkage owned by the other leg in this route.
    let sibling_original = batch(other, IntentKind::Exit, None);
    journal.commit(&sibling_original).await.unwrap();
    let sibling_link = linked_sell(&sibling, &sibling_original.intent);
    let mut sibling_terminal = zero_execution_order("300", "CANCELED", OrderPhase::Terminal);
    sibling_terminal.client_order_id = sibling_link.client_order_id.clone();
    sibling
        .ledger
        .record_order(&sibling_terminal, &sibling_link, seen(1))
        .await
        .unwrap();

    // 400 is reused on the same symbol in a different account.
    let mut another_account = unowned();
    another_account.account_scope = "other-account".into();
    let other_account_order = zero_execution_order("400", "CANCELED", OrderPhase::Terminal);
    f.ledger
        .record_order(&other_account_order, &another_account, seen(1))
        .await
        .unwrap();

    for target in ["100", "200", "300", "400"] {
        assert_eq!(group(&pool, s, target).await, (1, 0, false));
        assert!(terminal_work(&pool, s, target).await.is_empty());
    }
    let report = audit(&pool, s).await.unwrap();
    for target in ["100", "300"] {
        let row = report
            .rows
            .iter()
            .find(|row| row.original_target.as_deref() == Some(target))
            .unwrap();
        assert_eq!(row.target_id.as_deref(), Some(target));
        assert!(row.target_conflict);
    }
    for target in ["200", "400"] {
        let row = report
            .rows
            .iter()
            .find(|row| row.original_target.as_deref() == Some(target))
            .unwrap();
        assert!(row.target_id.is_none());
    }
    assert_leg(&pool, s, (4, 0)).await;

    // A known venue outcome and a no-send claim cannot release the member.
    // An exact target can mask pending work but must retain the contradiction.
    let mut contradictions = pending_cancels(&journal, s, "500", 1).await;
    contradictions[0].outcome = Some(OutcomeRow {
        not_sent: true,
        venue_outcome: Some(VenueOutcome::Rejected),
        ..OutcomeRow::default()
    });
    journal.commit(&contradictions[0]).await.unwrap();
    assert_eq!(group(&pool, s, "500").await, (1, 1, false));
    assert_leg(&pool, s, (5, 1)).await;
    let original = batch(s, IntentKind::Exit, None);
    journal.commit(&original).await.unwrap();
    let link = linked_sell(&f, &original.intent);
    let mut terminal = zero_execution_order("500", "CANCELED", OrderPhase::Terminal);
    terminal.client_order_id = link.client_order_id.clone();
    f.ledger.record_order(&terminal, &link, seen(1)).await.unwrap();
    assert_eq!(group(&pool, s, "500").await, (1, 1, true));
    assert_eq!(
        members(&pool, s, "500").await,
        [(contradictions[0].intent.intent_id, true, true)]
    );
    assert_leg(&pool, s, (4, 1)).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cancel_shadow_keeps_evidence_only_no_send_conflicts(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let journal = PgJournalBackend::new(&pool, s.tenant);
    install(&pool).await;

    let mut cancels = pending_cancels(&journal, s, "700", 1).await;
    let contradictory = OutcomeRow {
        not_sent: true,
        venue_evidence: Some(serde_json::json!({"opaque_fixture_evidence": "retained"})),
        ..OutcomeRow::default()
    };
    cancels[0].outcome = Some(contradictory.clone());
    for _ in 0..2 {
        journal.commit(&cancels[0]).await.unwrap();
        assert_eq!(group(&pool, s, "700").await, (1, 1, false));
        assert_leg(&pool, s, (1, 1)).await;
    }
    let id = cancels[0].intent.intent_id;
    let raw: bool = sqlx::query_scalar(
        "SELECT not_sent AND venue_outcome IS NULL AND venue_evidence IS NOT NULL
         FROM order_intents WHERE intent_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(raw);

    let original = batch(s, IntentKind::Exit, None);
    journal.commit(&original).await.unwrap();
    let link = linked_sell(&f, &original.intent);
    let mut terminal = zero_execution_order("700", "CANCELED", OrderPhase::Terminal);
    terminal.client_order_id = link.client_order_id.clone();
    f.ledger.record_order(&terminal, &link, seen(1)).await.unwrap();
    assert_eq!(group(&pool, s, "700").await, (1, 1, true));
    assert_eq!(members(&pool, s, "700").await, [(id, true, true)]);
    assert_leg(&pool, s, (0, 1)).await;

    let mut unbound = batch(s, IntentKind::Cancel, Some("700"));
    unbound.intent.position_id = None;
    unbound.outcome = Some(OutcomeRow { not_sent: true, ..OutcomeRow::default() });
    journal.commit(&unbound).await.unwrap();
    assert!(!audit(&pool, s).await.unwrap().ambiguous_unbound);
    unbound.outcome = Some(contradictory);
    journal.commit(&unbound).await.unwrap();
    assert!(audit(&pool, s).await.unwrap().ambiguous_unbound);
    let unresolved: (Option<Uuid>, Option<String>, bool, bool) = sqlx::query_as(
        "SELECT position_id, symbol, pending, conflict
         FROM cancel_shadow_members WHERE intent_id=$1",
    )
    .bind(unbound.intent.intent_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unresolved, (None, None, true, true));
    assert_leg(&pool, s, (0, 1)).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn cancel_shadow_rolls_back_and_preserves_uncertain_commit_semantics(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let journal = PgJournalBackend::new(&pool, s.tenant);
    install(&pool).await;
    pending_cancels(&journal, s, "100", 2).await;

    let original = batch(s, IntentKind::Exit, None);
    journal.commit(&original).await.unwrap();
    let link = linked_sell(&f, &original.intent);
    let mut live = zero_execution_order("100", "NEW", OrderPhase::Live);
    live.client_order_id = link.client_order_id.clone();
    f.ledger.record_order(&live, &link, seen(1)).await.unwrap();
    let before = members(&pool, s, "100").await;
    let version = f.version(&pool).await;

    sqlx::query("INSERT INTO cancel_shadow_fail_terminal(target_id) VALUES ('100')")
        .execute(&pool)
        .await
        .unwrap();
    let mut terminal = live;
    terminal.phase = OrderPhase::Terminal;
    terminal.status = "CANCELED".into();
    assert!(matches!(
        f.ledger.record_order(&terminal, &link, seen(2)).await,
        Err(LedgerError::Backend(_))
    ));
    assert_eq!(f.version(&pool).await, version);
    assert_eq!(group(&pool, s, "100").await, (2, 0, false));
    assert_eq!(members(&pool, s, "100").await, before);
    assert!(terminal_work(&pool, s, "100").await.is_empty());
    assert_leg(&pool, s, (2, 0)).await;
    let cold = audit(&pool, s).await.unwrap();
    assert!(cold.rows.iter().filter(|row| row.kind == "cancel").all(|row| {
        row.target_phase.as_deref() == Some("live") && row.target_status.as_deref() == Some("NEW")
    }));

    sqlx::query("DELETE FROM cancel_shadow_fail_terminal WHERE target_id='100'")
        .execute(&pool)
        .await
        .unwrap();
    let (proxy, proxied) = through_proxy_limit(&pool, 1).await;
    let ledger = PgVenueLedger::new(&proxied, s.tenant);
    let pid = backend_pid(&proxied).await;
    proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE venue_orders"));
    assert!(matches!(
        ledger.record_order(&terminal, &link, seen(3)).await,
        Err(LedgerError::Uncertain(_))
    ));
    retired(&pool, pid).await;
    assert_eq!(proxy.stats().commit_acks_dropped, 1);

    // An independent read establishes the actual fate after the lost reply.
    assert_eq!(f.version(&pool).await, version + 1);
    assert_eq!(group(&pool, s, "100").await, (2, 0, true));
    assert_eq!(members(&pool, s, "100").await, before);
    assert_eq!(terminal_work(&pool, s, "100").await, [(1, 1, 0)]);
    assert_leg(&pool, s, (0, 0)).await;
    assert!(!f.ledger.record_order(&terminal, &link, seen(4)).await.unwrap().changed);
}

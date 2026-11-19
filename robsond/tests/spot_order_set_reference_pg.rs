//! Disposable PostgreSQL order-set reference. This test target owns no runtime
//! authority.
#![cfg(feature = "postgres")]

#[path = "spot_order_set_reference_pg/cancel_shadow_tests.rs"]
mod cancel_shadow_tests;
#[allow(
    dead_code,
    reason = "shared ledger fixture includes controls for other targets"
)]
#[path = "venue_ledger_orders/fixture.rs"]
mod fixture;
#[path = "spot_order_set_reference_pg/reader.rs"]
mod reader;
#[path = "spot_order_set_reference_pg/sell_projection_tests.rs"]
mod sell_projection_tests;

use std::time::Duration;

use fixture::{record, seen, unowned, Fixture, VENUE};
use proptest::{
    collection::vec,
    prelude::*,
    test_runner::{Config, RngSeed, TestCaseResult, TestRunner},
};
use reader::{Report, Scope, Snapshot, VenueLinkage};
use robson_exec::{
    order_journal::{
        BalanceBatch, IntentKind, IntentRow, JournalBackend, JournalBatch, OutcomeRow, VenueOutcome,
    },
    venue::{OrderPhase, VenueId, VenueOrderRecord},
    venue_ledger::{LegOrderKind, OrderLink, OrderOwner},
};
use robsond::{order_journal_store::PgJournalBackend, process_epoch::start_epoch};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

const CEILING: Duration = Duration::from_secs(5);

fn scope(f: &Fixture) -> Scope {
    Scope {
        tenant: f.tenant,
        venue: VENUE,
        account: "scope",
        position: f.position,
        symbol: "BTCUSDT",
    }
}

fn batch(s: Scope, kind: IntentKind, target: Option<&str>) -> JournalBatch {
    let id = Uuid::now_v7();
    JournalBatch {
        intent: IntentRow {
            intent_id: id,
            venue_id: s.venue.to_owned(),
            account_scope: s.account.to_owned(),
            client_order_id: format!("reference-{id}"),
            position_id: Some(s.position),
            kind,
            cancels_order_id: target.map(str::to_owned),
            params: serde_json::json!({ "quantity": "10" }),
            created_at: seen(0).at,
        },
        attempts: Vec::new(),
        outcome: None,
        balance: Some(BalanceBatch {
            admission: None,
            handed_off: false,
            commit_bound: Duration::from_millis(500),
        }),
    }
}

async fn audit(pool: &PgPool, s: Scope) -> Result<Report, reader::Refusal> {
    tokio::time::timeout(CEILING, async { Snapshot::open(pool, s).await?.finish().await })
        .await
        .expect("local reference read exceeded its fixture ceiling")
}

fn ids(report: &Report) -> Vec<Uuid> {
    report.rows.iter().map(|row| row.intent_id).collect()
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn exhausts_all_pages_of_a_frozen_scope(pool: PgPool) {
    let empty = Scope {
        tenant: Uuid::now_v7(),
        venue: VENUE,
        account: "scope",
        position: Uuid::now_v7(),
        symbol: "BTCUSDT",
    };
    tokio::time::timeout(CEILING, async {
        let mut conn = reader::begin(&pool).await.unwrap();
        assert!(reader::page(&mut conn, empty, None).await.unwrap().is_empty());
        sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
    })
    .await
    .unwrap();

    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let j = PgJournalBackend::new(&pool, f.tenant);
    let epoch = start_epoch(&pool, f.tenant, "reference-fixture").await.unwrap();
    let mut expected = vec![f.admission];
    for (index, lengths) in [vec![2, 0], vec![2, 1], vec![2, 2, 0]].into_iter().enumerate() {
        let mut row = batch(s, IntentKind::Exit, None);
        row.outcome = Some(OutcomeRow {
            not_sent: true,
            venue_outcome: (index == 1).then_some(VenueOutcome::Rejected),
            ..OutcomeRow::default()
        });
        j.commit(&row).await.unwrap();
        expected.push(row.intent.intent_id);
        expected.sort_unstable();

        let report = audit(&pool, s).await.unwrap();
        assert_eq!(ids(&report), expected);
        assert_eq!(report.page_lengths, lengths);
        assert_eq!(report.tenant, f.tenant);
        assert_eq!(report.epoch, Some(epoch.epoch.epoch_id));
        let observed = report.rows.iter().find(|r| r.intent_id == row.intent.intent_id).unwrap();
        assert_eq!((observed.no_send, observed.named_conflict), (index != 1, index == 1));
        assert!(!observed.has_disposition);
    }

    // Record the actual fixture plan separately from application page counts.
    // EXPLAIN without ANALYZE does not measure row work or wire statements.
    tokio::time::timeout(CEILING, async {
        let mut conn = reader::begin(&pool).await.unwrap();
        let plan = reader::explain(&mut conn, s).await.unwrap();
        assert!(!plan.is_empty());
        println!("REFERENCE_KEYSET_EXPLAIN");
        for line in plan {
            println!("{line}");
        }
        sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
    })
    .await
    .unwrap();

    let mut orphan = batch(s, IntentKind::Cancel, Some("777"));
    orphan.intent.position_id = None;
    j.commit(&orphan).await.unwrap();
    let report = audit(&pool, s).await.unwrap();
    assert!(report.ambiguous_unbound);
    assert_eq!(ids(&report), expected, "a null-leg request is reported as scoped ambiguity");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn refuses_invalid_admission_and_wrong_journal_route(pool: PgPool) {
    use reader::Refusal;

    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let no_leg = Uuid::now_v7();
    f.ledger
        .insert_route(no_leg, &VenueId::new(VENUE), "scope", seen(0).at)
        .await
        .unwrap();
    for wrong in [
        Scope { position: no_leg, ..s },
        Scope { tenant: Uuid::now_v7(), ..s },
        Scope { venue: "bad-venue", ..s },
        Scope { account: "bad-account", ..s },
        Scope { symbol: "ETHUSDT", ..s },
    ] {
        assert!(matches!(audit(&pool, wrong).await, Err(Refusal::Route)));
    }

    for wrong in [
        Scope { tenant: Uuid::now_v7(), ..s },
        Scope { venue: "other-venue", ..s },
        Scope { account: "other-account", ..s },
    ] {
        let row = batch(wrong, IntentKind::Exit, None).intent;
        // The current writer refuses unflagged Spot mutations. Seed the old
        // M31 shape directly in the fixture to exercise the reader's refusal.
        sqlx::query(
            "INSERT INTO order_intents
               (intent_id, tenant_id, venue_id, account_scope, client_order_id,
                position_id, kind, params, created_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(row.intent_id)
        .bind(wrong.tenant)
        .bind(&row.venue_id)
        .bind(&row.account_scope)
        .bind(&row.client_order_id)
        .bind(row.position_id)
        .bind(row.kind.as_str())
        .bind(&row.params)
        .bind(row.created_at)
        .execute(&pool)
        .await
        .unwrap();
        assert!(matches!(audit(&pool, s).await, Err(Refusal::JournalRoute)));
        sqlx::query("DELETE FROM order_intents WHERE intent_id=$1")
            .bind(row.intent_id)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM order_intents WHERE intent_id=$1")
        .bind(f.admission)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(audit(&pool, s).await, Err(Refusal::Admission)));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn original_cancel_target_survives_a_decoy(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let j = PgJournalBackend::new(&pool, f.tenant);
    let mut origin = None;
    for (exchange_id, status, phase) in [
        ("7", "PARTIALLY_FILLED", OrderPhase::Live),
        ("9", "PENDING_NEW", OrderPhase::Live),
    ] {
        let intent = batch(s, IntentKind::Exit, None);
        j.commit(&intent).await.unwrap();
        let mut link = f.link(LegOrderKind::Sell);
        link.owner = OrderOwner::Leg {
            intent_id: intent.intent.intent_id,
            position_id: f.position,
            kind: LegOrderKind::Sell,
        };
        link.client_order_id = intent.intent.client_order_id;
        let mut row = record();
        row.exchange_order_id = exchange_id.to_owned();
        row.client_order_id = link.client_order_id.clone();
        row.status = status.to_owned();
        row.phase = phase;
        if exchange_id == "9" {
            row.executed_qty = Decimal::ZERO;
            row.cumulative_quote_qty = Some(Decimal::ZERO);
        }
        f.ledger.record_order(&row, &link, seen(1)).await.unwrap();
        if exchange_id == "9" {
            origin = Some((row, link));
        }
    }

    let cancel = batch(s, IntentKind::Cancel, Some("9"));
    assert_ne!(cancel.intent.client_order_id, "9");
    j.commit(&cancel).await.unwrap();

    let live = audit(&pool, s).await.unwrap();
    let (mut order, link) = origin.unwrap();
    order.status = "CANCELED".to_owned();
    order.phase = OrderPhase::Terminal;
    f.ledger.record_order(&order, &link, seen(2)).await.unwrap();

    let final_report = audit(&pool, s).await.unwrap();
    let actual = final_report
        .rows
        .iter()
        .find(|r| r.intent_id == cancel.intent.intent_id)
        .unwrap();
    assert_eq!(actual.kind, "cancel");
    assert_eq!(actual.original_target.as_deref(), Some("9"));
    assert_eq!(
        (actual.target_id.as_deref(), actual.target_status.as_deref()),
        (Some("9"), Some("CANCELED")),
        "cancel follows its original target, independent of a lower-ID decoy"
    );
    let prior = live.rows.iter().find(|r| r.intent_id == cancel.intent.intent_id).unwrap();
    assert_eq!(prior.target_status.as_deref(), Some("PENDING_NEW"));
    assert_eq!(prior.target_phase.as_deref(), Some("live"));

    assert_eq!(actual.target_phase.as_deref(), Some("terminal"));
    assert!(!actual.target_conflict);
    let decoy = final_report.rows.iter().find(|r| r.named_id.as_deref() == Some("7")).unwrap();
    assert_eq!(decoy.named_status.as_deref(), Some("PARTIALLY_FILLED"));
    assert_eq!(decoy.named_phase.as_deref(), Some("live"));
    assert!(!decoy.named_conflict);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn another_leg_order_cannot_satisfy_named_or_cancel_evidence(pool: PgPool) {
    let selected = Fixture::new(&pool).await;
    let other = Fixture::for_tenant(&pool, selected.tenant).await;
    let s = scope(&selected);
    let journal = PgJournalBackend::new(&pool, selected.tenant);
    let intent = batch(s, IntentKind::Exit, None);
    journal.commit(&intent).await.unwrap();

    // M36 binds the ledger row to its actual leg route. The journal intent ID
    // remains unique but its position need not match the row's actual owner.
    let mut link = other.link(LegOrderKind::Sell);
    link.owner = OrderOwner::Leg {
        intent_id: intent.intent.intent_id,
        position_id: other.position,
        kind: LegOrderKind::Sell,
    };
    link.client_order_id = intent.intent.client_order_id;
    let mut order = record();
    order.exchange_order_id = "80".to_owned();
    order.client_order_id = link.client_order_id.clone();
    order.status = "CANCELED".to_owned();
    order.phase = OrderPhase::Terminal;
    order.executed_qty = Decimal::ZERO;
    order.cumulative_quote_qty = Some(Decimal::ZERO);
    other.ledger.record_order(&order, &link, seen(1)).await.unwrap();

    let cancel = batch(s, IntentKind::Cancel, Some("80"));
    journal.commit(&cancel).await.unwrap();
    let report = audit(&pool, s).await.unwrap();
    let named = report.rows.iter().find(|row| row.intent_id == intent.intent.intent_id).unwrap();
    assert_eq!(named.named_id.as_deref(), Some("80"));
    assert!(
        named.named_conflict,
        "an exact intent ID cannot hide the actual other-leg owner"
    );
    let target = report.rows.iter().find(|row| row.intent_id == cancel.intent.intent_id).unwrap();
    assert_eq!(target.target_id.as_deref(), Some("80"));
    assert_eq!(target.target_status.as_deref(), Some("CANCELED"));
    assert!(
        target.target_conflict,
        "a terminal target cannot discharge another leg's evidence"
    );
}

fn zero_execution_order(id: &str, status: &str, phase: OrderPhase) -> VenueOrderRecord {
    let mut observed = record();
    observed.exchange_order_id = id.to_owned();
    observed.status = status.to_owned();
    observed.phase = phase;
    observed.executed_qty = Decimal::ZERO;
    observed.cumulative_quote_qty = Some(Decimal::ZERO);
    observed
}

fn linked_sell(f: &Fixture, intent: &IntentRow) -> OrderLink {
    OrderLink {
        owner: OrderOwner::Leg {
            intent_id: intent.intent_id,
            position_id: f.position,
            kind: LegOrderKind::Sell,
        },
        client_order_id: intent.client_order_id.clone(),
        ..unowned()
    }
}

fn complement(report: &Report) -> Vec<(&str, VenueLinkage)> {
    report
        .venue_complement
        .iter()
        .map(|row| (row.exchange_order_id.as_str(), row.linkage()))
        .collect()
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn venue_complement_preserves_unlinked_orders_across_frozen_pages(pool: PgPool) {
    use VenueLinkage::{MissingJournal, RouteUnowned};

    let f = Fixture::new(&pool).await;
    let other = Fixture::for_tenant(&pool, f.tenant).await;
    let s = scope(&f);
    let j = PgJournalBackend::new(&pool, f.tenant);
    let live = zero_execution_order("10", "NEW", OrderPhase::Live);
    let live_link = f.link(LegOrderKind::Sell);
    f.ledger.record_order(&live, &live_link, seen(1)).await.unwrap();

    let unknown = zero_execution_order("20", "PENDING_CANCEL", OrderPhase::Unknown);
    f.ledger
        .record_order(&unknown, &f.link(LegOrderKind::Sell), seen(1))
        .await
        .unwrap();
    let unbound = zero_execution_order("30", "NEW", OrderPhase::Live);
    f.ledger.record_order(&unbound, &unowned(), seen(1)).await.unwrap();

    // The schema can retain an old terminal flag that disagrees with status.
    let historical = zero_execution_order("35", "PENDING_CANCEL", OrderPhase::Unknown);
    f.ledger
        .record_order(&historical, &f.link(LegOrderKind::Sell), seen(1))
        .await
        .unwrap();
    let fabricated = sqlx::query(
        "UPDATE venue_orders SET phase='terminal', terminal_seen_at=$6
         WHERE tenant_id=$1 AND venue_id=$2 AND account_scope=$3 AND symbol=$4
           AND exchange_order_id=$5",
    )
    .bind(f.tenant)
    .bind(s.venue)
    .bind(s.account)
    .bind(s.symbol)
    .bind(&historical.exchange_order_id)
    .bind(seen(2).at)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(fabricated.rows_affected(), 1);

    let named = batch(s, IntentKind::Exit, None);
    j.commit(&named).await.unwrap();
    let named_link = linked_sell(&f, &named.intent);
    let mut named_order = zero_execution_order("40", "PENDING_CANCEL", OrderPhase::Unknown);
    named_order.client_order_id = named_link.client_order_id.clone();
    f.ledger.record_order(&named_order, &named_link, seen(1)).await.unwrap();

    let separate_leg = zero_execution_order("60", "NEW", OrderPhase::Live);
    other
        .ledger
        .record_order(&separate_leg, &other.link(LegOrderKind::Sell), seen(1))
        .await
        .unwrap();
    let mut separate_symbol = zero_execution_order("70", "NEW", OrderPhase::Live);
    separate_symbol.symbol = "ETHUSDT".into();
    f.ledger.record_order(&separate_symbol, &unowned(), seen(1)).await.unwrap();
    let mut separate_account = unowned();
    separate_account.account_scope = "other-account".into();
    let outside_account = zero_execution_order("80", "NEW", OrderPhase::Live);
    f.ledger
        .record_order(&outside_account, &separate_account, seen(1))
        .await
        .unwrap();

    let mut frozen = Snapshot::open(&pool, s).await.unwrap();
    frozen.step().await.unwrap();
    let late_journal = batch(s, IntentKind::Protection, None);
    j.commit(&late_journal).await.unwrap();
    let late_order = zero_execution_order("50", "NEW", OrderPhase::Live);
    f.ledger
        .record_order(&late_order, &f.link(LegOrderKind::Sell), seen(3))
        .await
        .unwrap();

    let frozen = frozen.finish().await.unwrap();
    // The J-only projection sees the matched row and none of the orphans.
    let named_ids: Vec<_> = frozen.rows.iter().filter_map(|row| row.named_id.as_deref()).collect();
    assert_eq!(named_ids, ["40"]);
    let named = frozen.rows.iter().find(|row| row.intent_id == named.intent.intent_id).unwrap();
    assert_eq!(
        (named.named_phase.as_deref(), named.named_status.as_deref()),
        (Some("unknown"), Some("PENDING_CANCEL"))
    );
    assert!(!named.named_conflict);
    assert_eq!(
        complement(&frozen),
        [
            ("10", MissingJournal),
            ("20", MissingJournal),
            ("30", RouteUnowned),
            ("35", MissingJournal)
        ],
        "J-only observation lost venue orders without an exact journal link"
    );
    assert_eq!(frozen.venue_page_lengths, [2, 2, 1]);

    let rows = &frozen.venue_complement;
    let OrderOwner::Leg { intent_id, .. } = live_link.owner else {
        unreachable!()
    };
    assert_eq!((rows[0].intent_id, rows[0].position_id), (Some(intent_id), Some(f.position)));
    assert_eq!(
        (
            rows[0].tenant_id,
            rows[0].venue_id.as_str(),
            rows[0].account_scope.as_str(),
            rows[0].symbol.as_str(),
            rows[0].client_order_id.as_str(),
            rows[0].owner.as_str(),
            rows[0].kind.as_deref(),
            rows[0].side.as_str(),
            rows[0].phase.as_str(),
            rows[0].status.as_str(),
            rows[0].terminal
        ),
        (
            f.tenant,
            s.venue,
            s.account,
            s.symbol,
            "original",
            "leg",
            Some("sell"),
            "SELL",
            "live",
            "NEW",
            false
        )
    );
    assert_eq!((rows[1].phase.as_str(), rows[1].status.as_str()), ("unknown", "PENDING_CANCEL"));
    assert_eq!(
        (rows[2].owner.as_str(), rows[2].position_id, rows[2].intent_id),
        ("discovered", None, None)
    );
    assert_eq!(
        (rows[3].phase.as_str(), rows[3].status.as_str(), rows[3].terminal),
        ("terminal", "PENDING_CANCEL", true)
    );
    assert!(!ids(&frozen).contains(&late_journal.intent.intent_id));

    let fresh = audit(&pool, s).await.unwrap();
    assert!(ids(&fresh).contains(&late_journal.intent.intent_id));
    assert_eq!(
        complement(&fresh),
        [
            ("10", MissingJournal),
            ("20", MissingJournal),
            ("30", RouteUnowned),
            ("35", MissingJournal),
            ("50", MissingJournal)
        ]
    );
    assert_eq!(fresh.venue_page_lengths, [2, 2, 2, 0]);
    assert!(fresh.version > frozen.version);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn terminal_client_and_kind_conflicts_survive_original_cancel_lookup(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let s = scope(&f);
    let j = PgJournalBackend::new(&pool, f.tenant);
    let mut cases = Vec::new();
    for (id, journal_kind, wrong_client) in [
        ("100", IntentKind::Exit, false),
        ("101", IntentKind::Exit, true),
        ("102", IntentKind::Protection, false),
    ] {
        let intent = batch(s, journal_kind, None);
        j.commit(&intent).await.unwrap();
        let mut link = linked_sell(&f, &intent.intent);
        if wrong_client {
            link.client_order_id = "wrong-native-client-101".into();
        }
        let mut observed = zero_execution_order(id, "CANCELED", OrderPhase::Terminal);
        observed.client_order_id = link.client_order_id.clone();
        f.ledger.record_order(&observed, &link, seen(1)).await.unwrap();

        let cancel = batch(s, IntentKind::Cancel, Some(id));
        j.commit(&cancel).await.unwrap();
        cases.push((
            id,
            intent.intent.intent_id,
            cancel.intent.intent_id,
            journal_kind,
            wrong_client,
        ));
    }

    let report = audit(&pool, s).await.unwrap();
    for (id, intent, cancel, journal_kind, wrong_client) in cases {
        let wrong_kind = journal_kind == IntentKind::Protection;
        let expected_conflict = wrong_client || wrong_kind;
        let named = report.rows.iter().find(|row| row.intent_id == intent).unwrap();
        assert_eq!(named.named_id.as_deref(), Some(id));
        assert_eq!(named.named_phase.as_deref(), Some("terminal"));
        assert_eq!(named.named_status.as_deref(), Some("CANCELED"));
        assert_eq!(named.named_conflict, expected_conflict, "named evidence for {id}");

        let target = report.rows.iter().find(|row| row.intent_id == cancel).unwrap();
        assert_eq!(
            (target.original_target.as_deref(), target.target_id.as_deref()),
            (Some(id), Some(id))
        );
        assert_eq!(target.target_phase.as_deref(), Some("terminal"));
        assert_eq!(target.target_status.as_deref(), Some("CANCELED"));
        assert_eq!(target.target_conflict, expected_conflict, "cancel target {id}");
    }
    assert_eq!(
        complement(&report),
        [
            ("101", VenueLinkage::JournalConflict),
            ("102", VenueLinkage::JournalConflict)
        ]
    );
}

async fn generated_case(
    pool: &PgPool,
    n: usize,
    ranks: Vec<u16>,
    barrier: u8,
    page: u8,
) -> TestCaseResult {
    let f = Fixture::new(pool).await;
    let s = scope(&f);
    let j = PgJournalBackend::new(pool, f.tenant);
    let seeded: Vec<_> = (1..n).map(|_| batch(s, IntentKind::Protection, None)).collect();
    let mut expected: Vec<_> = std::iter::once(f.admission)
        .chain(seeded.iter().map(|row| row.intent.intent_id))
        .collect();
    let mut permutation: Vec<_> = (0..seeded.len()).collect();
    permutation.sort_by_key(|&i| (ranks[i], i));
    for i in permutation {
        j.commit(&seeded[i]).await.unwrap();
    }
    let later = batch(s, IntentKind::Cancel, Some("44"));
    let later_id = later.intent.intent_id;
    let immediately_visible = barrier == 0;
    if immediately_visible {
        j.commit(&later).await.unwrap();
        expected.push(later_id);
    }
    let mut reader = Snapshot::open(pool, s).await.unwrap();
    if !immediately_visible {
        if barrier == 2 {
            let page_count = 1 + usize::from(page) % (n / 2 + 1);
            for _ in 0..page_count {
                reader.step().await.unwrap();
            }
        }
        j.commit(&later).await.unwrap();
    }

    let old = reader.finish().await.unwrap();
    let new = audit(pool, s).await.unwrap();
    expected.sort_unstable();
    prop_assert_eq!(ids(&old), expected.clone());
    prop_assert_eq!(old.page_lengths.len(), expected.len() / 2 + 1);
    prop_assert_eq!(old.tenant, f.tenant);
    if !immediately_visible {
        expected.push(later_id);
        expected.sort_unstable();
    }
    prop_assert_eq!(ids(&new), expected.clone());
    prop_assert_eq!(new.page_lengths.len(), expected.len() / 2 + 1);
    prop_assert_eq!(new.tenant, f.tenant);
    prop_assert_eq!(new.version, old.version + i64::from(!immediately_visible));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn snapshot_membership_is_stable_across_generated_commits(pool: PgPool) {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut runner = TestRunner::new(Config {
            cases: 12,
            rng_seed: RngSeed::Fixed(20261118),
            max_shrink_iters: 12,
            failure_persistence: None,
            ..Config::default()
        });
        runner
            .run(
                &(2usize..=6, vec(any::<u16>(), 5), 0u8..3, any::<u8>()),
                |(n, ranks, barrier, page)| {
                    runtime.block_on(async {
                        tokio::time::timeout(
                            CEILING,
                            generated_case(&pool, n, ranks, barrier, page),
                        )
                        .await
                        .expect("generated fixture exceeded its ceiling")
                    })
                },
            )
            .unwrap();
    })
    .await
    .unwrap();
}

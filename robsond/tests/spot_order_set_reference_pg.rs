//! Disposable PostgreSQL order-set reference. This test target owns no runtime
//! authority.
#![cfg(feature = "postgres")]

#[allow(
    dead_code,
    reason = "shared ledger fixture includes controls for other targets"
)]
#[path = "venue_ledger_orders/fixture.rs"]
mod fixture;
#[path = "spot_order_set_reference_pg/reader.rs"]
mod reader;

use std::time::Duration;

use fixture::{record, seen, Fixture, VENUE};
use proptest::{
    collection::vec,
    prelude::*,
    test_runner::{Config, RngSeed, TestCaseResult, TestRunner},
};
use reader::{Report, Scope, Snapshot};
use robson_exec::{
    order_journal::{
        BalanceBatch, IntentKind, IntentRow, JournalBackend, JournalBatch, OutcomeRow, VenueOutcome,
    },
    venue::{OrderPhase, VenueId},
    venue_ledger::{LegOrderKind, OrderOwner},
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
        let mut row = batch(wrong, IntentKind::Exit, None);
        // Deliberately create the historical/conflicting shape M31 permits.
        row.balance = None;
        PgJournalBackend::new(&pool, wrong.tenant).commit(&row).await.unwrap();
        assert!(matches!(audit(&pool, s).await, Err(Refusal::JournalRoute)));
        sqlx::query("DELETE FROM order_intents WHERE intent_id=$1")
            .bind(row.intent.intent_id)
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

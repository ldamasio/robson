#![cfg(test)]

//! Native witnesses for the dormant result sink and its exact persisted receipt.
use super::super::result::{
    confirm_result_once, publish_result, PreparedSnapshot, Publication, ResultRequest,
};
use super::{
    assert_claim_sql, assert_confirm_sql, assert_native_claim, claim, deadline, seed, snapshot,
    tick, unchanged_row, BudgetError, ClaimIdentity, ClaimObservation, ClaimOutcome, ClaimRequest,
    ClaimResult, ClockDomain, Measured, PriorClaim, PriorEligibility, ProxyMode, Trace, DAY,
};
use chrono::{DateTime, Utc};
use robson_connectors::binance::spot::SpotCommission;
use robson_domain::Symbol;
use robson_exec::{balance_costs::CostIdentity, venue::VenueId, venue_clock::TickCapture};
use serde_json::json;
use sqlx::{PgPool, Row};
use std::time::Duration;
use tokio::time::{sleep, timeout, Instant};
use uuid::Uuid;

const UID: u64 = u64::MAX;
const SCOPE: &str = "bounded-result-fixture";

fn seconds_since_boot(capture: TickCapture) -> i64 {
    i64::try_from(capture.nanoseconds() / 1_000_000_000).unwrap()
}

fn offset(capture: TickCapture, seconds: i64, nanos: i64) -> TickCapture {
    tick(capture.domain(), seconds_since_boot(capture) + seconds, nanos)
}

async fn stored_claim(pool: &PgPool, request: ClaimRequest) -> ClaimObservation {
    let Ok(ClaimOutcome::Stored(observation)) =
        claim(pool, request, deadline(), &Trace::default()).await
    else {
        panic!("given fixture claim must advance");
    };
    observation
}

async fn initial_result(pool: &PgPool) -> ResultRequest {
    let request = seed(pool, true).await;
    let key = request.key.clone();
    let claim = stored_claim(pool, request).await;
    ResultRequest {
        key,
        claim,
        handoff: None,
        conclusion: ClaimResult::NoSend,
        snapshot: None,
    }
}

fn commission_payload(symbol: &str, discount: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "symbol": symbol,
        "standardCommission": {
            "maker": "0.0003", "taker": "0.0005", "buyer": "0.00001", "seller": "0.00002"
        },
        "specialCommission": {
            "maker": "0", "taker": "0", "buyer": "0", "seller": "0"
        },
        "taxCommission": {
            "maker": "0", "taker": "0", "buyer": "0", "seller": "0"
        },
        "discount": {
            "enabledForAccount": false, "enabledForSymbol": true,
            "discountAsset": "BNB", "discount": discount
        }
    }))
    .unwrap()
}

fn prepare(
    symbol: &str,
    tenant: Uuid,
    as_of: DateTime<Utc>,
    observed: TickCapture,
    discount: &str,
) -> PreparedSnapshot {
    let payload = commission_payload(symbol, discount);
    let commission: SpotCommission = serde_json::from_slice(&payload).unwrap();
    let account = CostIdentity::new(
        tenant,
        VenueId::new(VenueId::BINANCE_SPOT),
        SCOPE.to_owned(),
        UID,
        Symbol::from_pair(symbol).unwrap(),
    )
    .unwrap();
    let candidate = commission.cost_candidate(account, as_of).unwrap();
    PreparedSnapshot {
        evidence: candidate.evidence().clone(),
        observed,
        payload,
    }
}

fn success(base: ResultRequest) -> ResultRequest {
    let start = base.claim.identity.captured;
    let as_of = DateTime::<Utc>::from_timestamp(1_795_123_400, 802_345_000).unwrap();
    ResultRequest {
        handoff: Some(offset(start, 60, 0)),
        conclusion: ClaimResult::Success,
        snapshot: Some(prepare(
            &base.key.symbol,
            Uuid::now_v7(),
            as_of,
            offset(start, 63, 19),
            "1.0",
        )),
        ..base
    }
}

async fn publish(measured: &Measured, request: ResultRequest) -> Result<Publication, BudgetError> {
    publish_result(&measured.pool, request, deadline(), &measured.trace).await
}

async fn assert_native_receipt(pool: &PgPool, request: &ResultRequest) {
    let prepared = request.snapshot.as_ref().unwrap();
    let account = prepared.evidence.account();
    let handoff = request.handoff.unwrap();
    let row = sqlx::query(
        "SELECT revision, claim_id, owner_process, result_class,
                (handoff_domain).kind AS handoff_kind,
                (handoff_domain).boot_uuid AS handoff_boot,
                (handoff_domain).offset_seconds AS handoff_seconds,
                (handoff_domain).offset_nanoseconds AS handoff_nanos,
                handoff_tick::text, snapshot_claim_id, snapshot_revision,
                snapshot_owner, snapshot_tenant, snapshot_scope, snapshot_uid::text,
                (snapshot_domain).kind AS snapshot_kind,
                (snapshot_domain).boot_uuid AS snapshot_boot,
                (snapshot_domain).offset_seconds AS snapshot_seconds,
                (snapshot_domain).offset_nanoseconds AS snapshot_nanos,
                snapshot_tick::text, snapshot_as_of, snapshot_validation_version,
                snapshot_hash, snapshot_payload
         FROM n5_refresh_state WHERE venue_id='binance:spot' AND symbol=$1 AND descriptor='commission'",
    )
    .bind(&request.key.symbol)
    .fetch_one(pool)
    .await
    .unwrap();
    let handoff_domain = handoff.domain();
    let (handoff_seconds, handoff_nanos) = handoff_domain.offset();
    let snapshot_domain = prepared.observed.domain();
    let (snapshot_seconds, snapshot_nanos) = snapshot_domain.offset();
    assert_eq!(row.get::<i64, _>("revision"), request.claim.revision);
    assert_eq!(row.get::<Uuid, _>("claim_id"), request.claim.identity.claim_id);
    assert_eq!(row.get::<Uuid, _>("owner_process"), request.claim.identity.owner);
    assert_eq!(row.get::<String, _>("result_class"), "success");
    assert_eq!(row.get::<String, _>("handoff_kind"), "boottime");
    assert_eq!(row.get::<Uuid, _>("handoff_boot"), handoff_domain.boot());
    assert_eq!(row.get::<i64, _>("handoff_seconds"), handoff_seconds);
    assert_eq!(row.get::<i32, _>("handoff_nanos"), i32::try_from(handoff_nanos).unwrap());
    assert_eq!(row.get::<String, _>("handoff_tick"), handoff.nanoseconds().to_string());
    assert_eq!(row.get::<Uuid, _>("snapshot_claim_id"), request.claim.identity.claim_id);
    assert_eq!(row.get::<i64, _>("snapshot_revision"), request.claim.revision);
    assert_eq!(row.get::<Uuid, _>("snapshot_owner"), request.claim.identity.owner);
    assert_eq!(row.get::<Uuid, _>("snapshot_tenant"), account.tenant());
    assert_eq!(row.get::<String, _>("snapshot_scope"), account.scope());
    assert_eq!(row.get::<String, _>("snapshot_uid"), account.uid().to_string());
    assert_eq!(row.get::<String, _>("snapshot_kind"), "boottime");
    assert_eq!(row.get::<Uuid, _>("snapshot_boot"), snapshot_domain.boot());
    assert_eq!(row.get::<i64, _>("snapshot_seconds"), snapshot_seconds);
    assert_eq!(row.get::<i32, _>("snapshot_nanos"), i32::try_from(snapshot_nanos).unwrap());
    assert_eq!(
        row.get::<String, _>("snapshot_tick"),
        prepared.observed.nanoseconds().to_string()
    );
    assert_eq!(row.get::<DateTime<Utc>, _>("snapshot_as_of"), prepared.evidence.as_of());
    assert_eq!(
        row.get::<i16, _>("snapshot_validation_version"),
        i16::try_from(prepared.evidence.validation_version()).unwrap()
    );
    assert_eq!(row.get::<Vec<u8>, _>("snapshot_hash"), prepared.evidence.hash().as_slice());
    assert_eq!(row.get::<Vec<u8>, _>("snapshot_payload"), prepared.payload);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn exact_success_replay_requires_original_bytes_even_at_equal_normalized_hash(pool: PgPool) {
    let request = success(initial_result(&pool).await);
    let measured = Measured::start(&pool).await;

    assert!(matches!(publish(&measured, request.clone()).await, Ok(Publication::Published)));
    assert_claim_sql(&measured.wire(4, 1, 1), true);
    assert_native_receipt(&pool, &request).await;
    let persisted = unchanged_row(&pool).await;

    assert!(matches!(publish(&measured, request.clone()).await, Ok(Publication::Replayed)));
    assert_claim_sql(&measured.wire(8, 2, 2)[4..], true);
    assert_eq!(unchanged_row(&pool).await, persisted);

    let original = request.snapshot.as_ref().unwrap();
    let alternate = |discount| {
        prepare(
            &request.key.symbol,
            original.evidence.account().tenant(),
            original.evidence.as_of(),
            original.observed,
            discount,
        )
    };
    let same_hash = alternate("1.00");
    assert_eq!(same_hash.evidence, original.evidence);
    assert_eq!(same_hash.observed, original.observed);
    assert_ne!(same_hash.payload, original.payload);
    let mut different_bytes = request.clone();
    different_bytes.snapshot = Some(same_hash);
    assert!(matches!(publish(&measured, different_bytes).await, Ok(Publication::Refused)));
    assert_claim_sql(&measured.wire(11, 3, 2)[8..], false);
    assert_eq!(unchanged_row(&pool).await, persisted);

    // Deliberately offer a different validated hash with the original bytes.
    // This isolates the hash comparison from the exact byte comparison above.
    let other_evidence = alternate("0.75").evidence;
    assert_ne!(other_evidence.hash(), original.evidence.hash());
    assert_eq!(other_evidence.account(), original.evidence.account());
    assert_eq!(other_evidence.as_of(), original.evidence.as_of());
    let mut different_hash = request.clone();
    different_hash.snapshot.as_mut().unwrap().evidence = other_evidence;
    assert!(matches!(publish(&measured, different_hash).await, Ok(Publication::Refused)));
    assert_eq!(unchanged_row(&pool).await, persisted);
    measured.retired(&pool, 4).await;
    assert_claim_sql(&measured.wire(14, 4, 2)[11..], false);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn success_replay_refuses_other_tenant_scope_or_uid(pool: PgPool) {
    let request = success(initial_result(&pool).await);
    let measured = Measured::start(&pool).await;
    assert!(matches!(publish(&measured, request.clone()).await, Ok(Publication::Published)));
    let persisted = unchanged_row(&pool).await;
    let original = request.snapshot.as_ref().unwrap();
    let account = original.evidence.account();
    let commission: SpotCommission = serde_json::from_slice(&original.payload).unwrap();
    for (index, (tenant, scope, uid)) in [
        (Uuid::now_v7(), account.scope(), account.uid()),
        (account.tenant(), "other-scope", account.uid()),
        (account.tenant(), account.scope(), account.uid() - 1),
    ]
    .into_iter()
    .enumerate()
    {
        let other = CostIdentity::new(
            tenant,
            account.venue().clone(),
            scope.to_owned(),
            uid,
            account.symbol().clone(),
        )
        .unwrap();
        let evidence = commission.cost_candidate(other, original.evidence.as_of()).unwrap();
        let mut wrong = request.clone();
        wrong.snapshot.as_mut().unwrap().evidence = evidence.evidence().clone();
        assert!(matches!(publish(&measured, wrong).await, Ok(Publication::Refused)));
        let count = 4 + 3 * (index as u64 + 1);
        let wire = measured.wire(count, index as u64 + 2, 1);
        assert_claim_sql(&wire[wire.len() - 3..], false);
        assert_eq!(unchanged_row(&pool).await, persisted);
    }
    measured.retired(&pool, 4).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn unknown_result_keeps_original_snapshot_and_replays_only_exactly(pool: PgPool) {
    let mut request = initial_result(&pool).await;
    request.conclusion = ClaimResult::Unknown;
    request.handoff = Some(offset(request.claim.identity.captured, 5, 123));
    let inherited = snapshot(&pool).await;
    let measured = Measured::start(&pool).await;
    assert!(matches!(publish(&measured, request.clone()).await, Ok(Publication::Published)));
    let (class, handoff): (String, String) = sqlx::query_as(
        "SELECT result_class, handoff_tick::text FROM n5_refresh_state WHERE symbol=$1",
    )
    .bind(&request.key.symbol)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(class, "unknown");
    assert_eq!(handoff, request.handoff.unwrap().nanoseconds().to_string());
    assert_eq!(snapshot(&pool).await, inherited);
    let persisted = unchanged_row(&pool).await;
    assert!(matches!(publish(&measured, request.clone()).await, Ok(Publication::Replayed)));
    request.handoff = None;
    assert!(matches!(publish(&measured, request).await, Ok(Publication::Refused)));
    measured.retired(&pool, 3).await;
    let wire = measured.wire(11, 3, 2);
    assert_claim_sql(&wire[..4], true);
    assert_claim_sql(&wire[4..8], true);
    assert_claim_sql(&wire[8..], false);
    assert_eq!(unchanged_row(&pool).await, persisted);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn terminal_non_success_preserves_snapshot_and_late_replay_cannot_overwrite(pool: PgPool) {
    let mut request = initial_result(&pool).await;
    let earlier_snapshot = snapshot(&pool).await;
    request.handoff = Some(offset(request.claim.identity.captured, 2, 0));
    request.conclusion = ClaimResult::VenueRejected;
    let measured = Measured::start(&pool).await;

    assert!(matches!(publish(&measured, request.clone()).await, Ok(Publication::Published)));
    assert_claim_sql(&measured.wire(4, 1, 1), true);
    let (result, handoff): (String, String) = sqlx::query_as(
        "SELECT result_class, handoff_tick::text FROM n5_refresh_state WHERE symbol=$1",
    )
    .bind(&request.key.symbol)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(result, "venue_rejected");
    assert_eq!(handoff, request.handoff.unwrap().nanoseconds().to_string());
    assert_eq!(snapshot(&pool).await, earlier_snapshot);

    let rejected = unchanged_row(&pool).await;
    assert!(matches!(publish(&measured, request.clone()).await, Ok(Publication::Replayed)));
    assert_claim_sql(&measured.wire(8, 2, 2)[4..], true);
    assert_eq!(unchanged_row(&pool).await, rejected);

    let mut different_class = request.clone();
    different_class.conclusion = ClaimResult::Unknown;
    assert!(matches!(publish(&measured, different_class).await, Ok(Publication::Refused)));
    assert_claim_sql(&measured.wire(11, 3, 2)[8..], false);
    assert_eq!(unchanged_row(&pool).await, rejected);

    let next = ClaimIdentity {
        claim_id: Uuid::now_v7(),
        owner: Uuid::now_v7(),
        digest: [0x81; 32],
        captured: offset(request.handoff.unwrap(), DAY, 0),
        audit_at: request.claim.identity.audit_at + chrono::Duration::days(1),
    };
    let claim = stored_claim(
        &pool,
        ClaimRequest {
            key: request.key.clone(),
            prior: PriorClaim {
                revision: request.claim.revision,
                identity: request.claim.identity.clone(),
                eligibility: PriorEligibility::SameDomain,
                handoff: request.handoff,
                result: Some(request.conclusion),
            },
            next,
        },
    )
    .await;
    let no_send = ResultRequest {
        key: request.key.clone(),
        claim,
        handoff: None,
        conclusion: ClaimResult::NoSend,
        snapshot: None,
    };
    assert!(matches!(publish(&measured, no_send.clone()).await, Ok(Publication::Published)));
    assert_claim_sql(&measured.wire(15, 4, 3)[11..], true);
    let (result, cleared): (String, i32) = sqlx::query_as(
        "SELECT result_class, num_nulls(handoff_domain, handoff_tick)
         FROM n5_refresh_state WHERE symbol=$1",
    )
    .bind(&request.key.symbol)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(result, "no_send");
    assert_eq!(cleared, 2);
    let winner = unchanged_row(&pool).await;
    assert!(matches!(publish(&measured, request).await, Ok(Publication::Refused)));
    assert_eq!(unchanged_row(&pool).await, winner);
    assert_eq!(snapshot(&pool).await, earlier_snapshot);
    measured.retired(&pool, 5).await;
    assert_claim_sql(&measured.wire(18, 5, 3)[15..], false);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn malformed_response_and_handoff_refuse_before_native_work(pool: PgPool) {
    let request = success(initial_result(&pool).await);
    let initial = unchanged_row(&pool).await;
    let start = request.claim.identity.captured;
    let measured = Measured::start(&pool).await;
    assert!(matches!(
        publish_result(&measured.pool, request.clone(), Instant::now(), &measured.trace).await,
        Err(BudgetError::Expired),
    ));

    let mut beyond_bound = request.clone();
    beyond_bound.handoff = Some(offset(start, 60, 1));
    let mut wrong_domain = request.clone();
    let (offset_seconds, offset_nanos) = start.domain().offset();
    let other_domain =
        ClockDomain::new(start.domain().boot(), offset_seconds, offset_nanos + 1).unwrap();
    wrong_domain.handoff = Some(tick(other_domain, seconds_since_boot(start), 0));
    let mut wrong_observation = request.clone();
    wrong_observation.snapshot.as_mut().unwrap().observed =
        tick(other_domain, seconds_since_boot(start) + 63, 19);
    let mut missing_handoff = request.clone();
    missing_handoff.handoff = None;
    let mut no_send_with_handoff = request.clone();
    no_send_with_handoff.conclusion = ClaimResult::NoSend;
    no_send_with_handoff.snapshot = None;
    let mut wrong_symbol = request.clone();
    let original = request.snapshot.as_ref().unwrap();
    wrong_symbol.snapshot = Some(prepare(
        "ETHUSDT",
        original.evidence.account().tenant(),
        original.evidence.as_of(),
        original.observed,
        "1.0",
    ));
    let mut submicrosecond = request.clone();
    submicrosecond.snapshot = Some(prepare(
        &request.key.symbol,
        original.evidence.account().tenant(),
        original.evidence.as_of() + chrono::Duration::nanoseconds(1),
        original.observed,
        "1.0",
    ));

    for (test_case, invalid) in [
        ("handoff exceeds the inclusive bound", beyond_bound),
        ("handoff changes BOOTTIME namespace", wrong_domain),
        ("response observation changes BOOTTIME namespace", wrong_observation),
        ("success needs a positive handoff", missing_handoff),
        ("no-send cannot carry a handoff", no_send_with_handoff),
        ("evidence belongs to a different symbol", wrong_symbol),
        ("native storage cannot preserve submicrosecond as_of", submicrosecond),
    ] {
        assert!(
            matches!(publish(&measured, invalid).await, Err(BudgetError::Invalid)),
            "{test_case}"
        );
    }
    measured.wire(0, 0, 0);
    assert_eq!(unchanged_row(&pool).await, initial);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn lost_result_commit_ack_allows_one_exact_positive_confirmation(pool: PgPool) {
    let request = success(initial_result(&pool).await);
    let measured = Measured::start(&pool).await;
    measured.proxy.set_mode(ProxyMode::DropCommitAck);
    let Ok(Publication::Uncertain(original)) = publish(&measured, request.clone()).await else {
        panic!("uncertain result expected after dropped COMMIT acknowledgement");
    };
    assert_eq!(measured.proxy.stats().commit_acks_dropped, 1);
    assert_claim_sql(&measured.wire(4, 1, 1), true);
    assert!(confirm_result_once(&measured.pool, original, &measured.trace).await.unwrap());
    assert_native_receipt(&pool, &request).await;
    measured.retired(&pool, 2).await;
    assert_confirm_sql(&measured.wire(5, 2, 1)[4]);
}

async fn defer_result_commit(pool: &PgPool) -> sqlx::Transaction<'_, sqlx::Postgres> {
    // A deferred trigger stalls the native COMMIT after the update receipt.
    sqlx::raw_sql(
        "CREATE FUNCTION n5_result_test_block_commit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(730062); RETURN NEW; END;
         $$;
         CREATE CONSTRAINT TRIGGER n5_result_test_block_commit AFTER UPDATE ON n5_refresh_state
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION n5_result_test_block_commit();",
    )
    .execute(pool)
    .await
    .unwrap();
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(730062)")
        .execute(&mut *blocker)
        .await
        .unwrap();

    blocker
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn absent_result_confirmation_can_precede_original_commit(pool: PgPool) {
    let request = success(initial_result(&pool).await);
    let unpublished = unchanged_row(&pool).await;
    let measured = Measured::start(&pool).await;
    let blocker = defer_result_commit(&pool).await;

    let sever_pending_commit = async {
        timeout(Duration::from_millis(900), async {
            loop {
                if measured.proxy.stats().commits == 1 {
                    let pids: Vec<i32> = measured
                        .proxy
                        .diagnostic_identities()
                        .unwrap()
                        .iter()
                        .filter_map(|identity| identity.pid)
                        .collect();
                    let waiting: bool = sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                         WHERE backend_type = 'client backend' AND pid = ANY($1)
                               AND wait_event_type = 'Lock' AND wait_event = 'advisory')",
                    )
                    .bind(pids)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                    if waiting {
                        measured.proxy.set_mode(ProxyMode::Sever);
                        break;
                    }
                }
                sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("the result COMMIT must be waiting in the native deferred trigger");
    };
    let (outcome, ()) = tokio::join!(publish(&measured, request.clone()), sever_pending_commit);
    let Ok(Publication::Uncertain(original)) = outcome else {
        panic!("COMMIT contributed no acknowledgement");
    };
    assert_claim_sql(&measured.wire(4, 1, 1), true);

    measured.proxy.set_mode(ProxyMode::Pass);
    assert!(!confirm_result_once(&measured.pool, original, &measured.trace).await.unwrap());
    assert_eq!(unchanged_row(&pool).await, unpublished);
    blocker.commit().await.unwrap();

    measured.retired(&pool, 2).await;
    assert_confirm_sql(&measured.wire(5, 2, 1)[4]);
    assert_native_receipt(&pool, &request).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn result_timeouts_retire_native_backends_without_finalizing(pool: PgPool) {
    let request = success(initial_result(&pool).await);
    let unpublished = unchanged_row(&pool).await;
    let measured = Measured::start(&pool).await;
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT revision FROM n5_refresh_state
         WHERE venue_id='binance:spot' AND symbol=$1 AND descriptor='commission' FOR UPDATE",
    )
    .bind(&request.key.symbol)
    .fetch_one(&mut *blocker)
    .await
    .unwrap();

    assert!(publish(&measured, request.clone()).await.is_err());
    measured.retired(&pool, 1).await;
    assert_claim_sql(&measured.wire(3, 1, 0), false);
    assert!(measured
        .proxy
        .diagnostics()
        .unwrap()
        .records
        .iter()
        .any(|record| { record.sqlstate == Some(*b"55P03") }));

    let caller_deadline = Instant::now() + Duration::from_millis(180);
    assert!(matches!(
        publish_result(&measured.pool, request.clone(), caller_deadline, &measured.trace).await,
        Err(BudgetError::Expired),
    ));
    measured.retired(&pool, 2).await;
    assert_claim_sql(&measured.wire(6, 2, 0)[3..], false);
    blocker.rollback().await.unwrap();

    sqlx::raw_sql(
        "CREATE FUNCTION n5_result_test_delay_statement() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_sleep(10); RETURN NEW; END;
         $$;
         CREATE TRIGGER n5_result_test_delay_statement BEFORE UPDATE ON n5_refresh_state
         FOR EACH ROW EXECUTE FUNCTION n5_result_test_delay_statement();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(publish(&measured, request).await.is_err());
    measured.retired(&pool, 3).await;
    assert_claim_sql(&measured.wire(9, 3, 0)[6..], false);
    assert!(measured
        .proxy
        .diagnostics()
        .unwrap()
        .records
        .iter()
        .any(|record| { record.sqlstate == Some(*b"57014") }));
    assert_eq!(unchanged_row(&pool).await, unpublished);
}

async fn observe_native_block(
    pool: &PgPool,
    measured: &Measured,
    blocking_pid: i32,
    at_commit: bool,
) -> (i32, String) {
    timeout(Duration::from_millis(250), async {
        loop {
            let pid = measured
                .proxy
                .diagnostic_identities()
                .unwrap()
                .iter()
                .find_map(|identity| identity.pid);
            if let Some(pid) = pid {
                let event: Option<String> = sqlx::query_scalar(
                    "SELECT wait_event FROM pg_stat_activity
                     WHERE pid = $1 AND datname = current_database() AND state = 'active'
                       AND $2 = ANY(pg_blocking_pids(pid)) AND wait_event_type = 'Lock'
                       AND (($3::boolean AND wait_event = 'advisory' AND query = 'COMMIT')
                            OR (NOT $3 AND wait_event IN ('transactionid', 'tuple')))",
                )
                .bind(pid)
                .bind(blocking_pid)
                .bind(at_commit)
                .fetch_optional(pool)
                .await
                .unwrap();
                if let Some(event) = event {
                    return (pid, event);
                }
            }
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("native lock wait not witnessed (blocking pid {blocking_pid})"))
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn precommit_snapshot_equal_loser_refuses_before_independent_replay(pool: PgPool) {
    let request = success(initial_result(&pool).await);
    let winner = Measured::start(&pool).await;
    let loser = Measured::start(&pool).await;
    let mut blocker = defer_result_commit(&pool).await;
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();

    let run_loser_after_winner_updates = async {
        let (winner_pid, winner_wait) =
            observe_native_block(&pool, &winner, blocker_pid, true).await;
        assert_claim_sql(&winner.wire(4, 1, 1), true);

        let release_after_loser_takes_its_snapshot = async {
            let (loser_pid, loser_wait) =
                observe_native_block(&pool, &loser, winner_pid, false).await;
            assert_claim_sql(&loser.wire(3, 1, 0), false);
            blocker.commit().await.unwrap();
            println!(
                "N5 result race winner_pid={winner_pid} winner_wait={winner_wait} \
                 loser_pid={loser_pid} loser_wait={loser_wait}"
            );
        };
        let (outcome, ()) =
            tokio::join!(publish(&loser, request.clone()), release_after_loser_takes_its_snapshot);
        outcome
    };

    let (first, concurrent_equal) =
        tokio::join!(publish(&winner, request.clone()), run_loser_after_winner_updates);
    assert!(matches!(first, Ok(Publication::Published)), "{first:?}");
    assert!(matches!(concurrent_equal, Ok(Publication::Refused)), "{concurrent_equal:?}");
    winner.retired(&pool, 1).await;
    loser.retired(&pool, 1).await;
    winner.wire(4, 1, 1);
    loser.wire(3, 1, 0);

    assert_native_receipt(&pool, &request).await;
    let persisted = unchanged_row(&pool).await;
    assert!(matches!(publish(&loser, request).await, Ok(Publication::Replayed)));
    loser.retired(&pool, 2).await;
    assert_claim_sql(&loser.wire(7, 2, 1)[3..], true);
    assert_eq!(unchanged_row(&pool).await, persisted);
}

fn advance_from(prior: &ResultRequest, settled: bool) -> ClaimRequest {
    let tick = prior.claim.identity.captured;
    let threshold = prior
        .handoff
        .map_or_else(|| offset(tick, DAY + 60, 0), |handoff| offset(handoff, DAY, 0));
    ClaimRequest {
        key: prior.key.clone(),
        prior: PriorClaim {
            revision: prior.claim.revision,
            identity: prior.claim.identity.clone(),
            eligibility: PriorEligibility::SameDomain,
            handoff: prior.handoff,
            result: settled.then_some(prior.conclusion),
        },
        next: ClaimIdentity {
            claim_id: Uuid::now_v7(),
            owner: Uuid::now_v7(),
            digest: [0x83; 32],
            captured: threshold,
            audit_at: prior.claim.identity.audit_at + chrono::Duration::days(1),
        },
    }
}

async fn scheduled_result(pool: &PgPool, request: ResultRequest, delay: u64) -> Publication {
    sleep(Duration::from_millis(delay)).await;
    publish_result(pool, request, deadline(), &Trace::default()).await.unwrap()
}

async fn assert_model_receipt(
    pool: &PgPool,
    winner: &ResultRequest,
    previous_snapshot: &mut String,
) {
    if winner.snapshot.is_some() {
        assert_native_receipt(pool, winner).await;
        *previous_snapshot = snapshot(pool).await;
    } else {
        let (revision, claim, result, cleared): (i64, Uuid, String, i32) = sqlx::query_as(
            "SELECT revision, claim_id, result_class, num_nulls(handoff_domain, handoff_tick)
             FROM n5_refresh_state
             WHERE venue_id='binance:spot' AND symbol=$1 AND descriptor='commission'",
        )
        .bind(&winner.key.symbol)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(revision, winner.claim.revision);
        assert_eq!(claim, winner.claim.identity.claim_id);
        assert_eq!(result, "no_send");
        assert_eq!(cleared, 2);
        assert_eq!(snapshot(pool).await, *previous_snapshot);
    }
}

async fn advance_past_unacknowledged_result(
    pool: &PgPool,
    current: &ResultRequest,
    previous_snapshot: &mut String,
) -> ClaimObservation {
    let unacknowledged = success(current.clone());
    let measured = Measured::start(pool).await;
    measured.proxy.set_mode(ProxyMode::DropCommitAck);
    let Ok(Publication::Uncertain(original)) = publish(&measured, unacknowledged.clone()).await
    else {
        panic!("scheduled result must lose the answer to its actual COMMIT");
    };
    assert_eq!(measured.proxy.stats().commit_acks_dropped, 1);
    assert_model_receipt(pool, &unacknowledged, previous_snapshot).await;

    let next = stored_claim(pool, advance_from(&unacknowledged, true)).await;
    let advanced = unchanged_row(pool).await;
    assert!(!confirm_result_once(&measured.pool, original, &measured.trace).await.unwrap());
    assert!(matches!(scheduled_result(pool, unacknowledged, 0).await, Publication::Refused));
    assert_eq!(unchanged_row(pool).await, advanced);
    assert_eq!(snapshot(pool).await, *previous_snapshot);
    assert_native_claim(pool, next.revision, &next.identity).await;
    measured.retired(pool, 2).await;
    assert_confirm_sql(&measured.wire(5, 2, 1)[4]);
    next
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn generated_interleavings_keep_one_terminal_result_per_claim(pool: PgPool) {
    use proptest::{
        array::uniform4,
        prelude::any,
        strategy::{Strategy, ValueTree},
        test_runner::TestRunner,
    };

    let mut current = initial_result(&pool).await;
    let mut previous_snapshot = snapshot(&pool).await;
    let mut runner = TestRunner::deterministic();
    let schedules = (uniform4(0_u64..8), any::<bool>(), any::<bool>());
    for case in 0..16 {
        let (
            [delay_first, delay_duplicate, delay_different, delay_advance],
            different_success,
            allow_advance,
        ) = schedules.new_tree(&mut runner).unwrap().current();
        let first = success(current.clone());
        let different = if different_success {
            success(current.clone())
        } else {
            current.clone()
        };
        let race_advance = async {
            if !allow_advance {
                return ClaimOutcome::Refused;
            }
            sleep(Duration::from_millis(delay_advance)).await;
            claim(&pool, advance_from(&current, false), deadline(), &Trace::default())
                .await
                .unwrap()
        };
        let (first_outcome, duplicate_outcome, different_outcome, advancement) = tokio::join!(
            scheduled_result(&pool, first.clone(), delay_first),
            scheduled_result(&pool, first.clone(), delay_duplicate),
            scheduled_result(&pool, different.clone(), delay_different),
            race_advance,
        );
        let outcomes = [first_outcome, duplicate_outcome, different_outcome];

        let (next, stale) = match advancement {
            ClaimOutcome::Stored(next) => {
                for outcome in &outcomes {
                    assert!(matches!(outcome, Publication::Refused), "case {case}: {outcomes:?}");
                }
                (next, first)
            },
            ClaimOutcome::Refused => {
                let published: Vec<_> = outcomes
                    .iter()
                    .enumerate()
                    .filter_map(|(index, outcome)| {
                        matches!(outcome, Publication::Published).then_some(index)
                    })
                    .collect();
                assert_eq!(published.len(), 1, "case {case}: {outcomes:?}");
                let different_won = published[0] == 2;
                for (index, outcome) in outcomes.iter().enumerate() {
                    match outcome {
                        Publication::Published | Publication::Refused => {},
                        Publication::Replayed => {
                            assert_eq!(index == 2, different_won, "case {case}")
                        },
                        Publication::Uncertain(_) => panic!("case {case}: unexpected ACK loss"),
                    }
                }
                let (winner, rival) = if different_won {
                    (different, first)
                } else {
                    (first, different)
                };
                assert_model_receipt(&pool, &winner, &mut previous_snapshot).await;
                let terminal = unchanged_row(&pool).await;
                assert!(matches!(
                    scheduled_result(&pool, winner.clone(), 0).await,
                    Publication::Replayed,
                ));
                assert!(matches!(scheduled_result(&pool, rival, 0).await, Publication::Refused));
                assert_eq!(unchanged_row(&pool).await, terminal);

                let next = stored_claim(&pool, advance_from(&winner, true)).await;
                (next, winner)
            },
            ClaimOutcome::Uncertain(_) => panic!("case {case}: unexpected claim ACK loss"),
        };

        assert_eq!(next.revision, current.claim.revision + 1);
        assert_native_claim(&pool, next.revision, &next.identity).await;
        assert_eq!(snapshot(&pool).await, previous_snapshot);
        let advanced = unchanged_row(&pool).await;
        assert!(matches!(scheduled_result(&pool, stale, 0).await, Publication::Refused));
        assert_eq!(unchanged_row(&pool).await, advanced);
        current.claim = next;

        // Insert a bounded no-answer schedule whenever the generated claim and
        // publication ranks coincide, and once unconditionally for coverage.
        if case == 0 || (allow_advance && delay_advance == delay_different) {
            let next =
                advance_past_unacknowledged_result(&pool, &current, &mut previous_snapshot).await;
            assert_eq!(next.revision, current.claim.revision + 1);
            current.claim = next;
        }
    }
}

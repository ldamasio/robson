use std::time::Duration;

use chrono::{DateTime, Utc};
use robson_exec::venue_clock::{ClockDomain, TickCapture};
use robson_testkit::{
    daemon_process::database_url_through,
    db_proxy::{DbProxy, ProxyMode},
};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use tokio::time::{sleep, timeout, Instant};
use uuid::Uuid;

use super::{
    claim, confirm_once, ClaimIdentity, ClaimObservation, ClaimOutcome, ClaimRequest, ClaimResult,
    FrequencyKey, PriorClaim, PriorEligibility,
};
use crate::venue_budget::{BudgetError, Trace};

const DAY: i64 = 86_400;
const CLAIM_TICK: i64 = 100;
const HANDOFF_TICK: i64 = 117;
const SNAPSHOT: &[u8] = &[0, 255, 17, 80];
const SNAPSHOT_HASH: [u8; 32] = [0x42; 32];

fn tick(domain: ClockDomain, seconds: i64, nanos: i64) -> TickCapture {
    TickCapture::new(domain, seconds, nanos).unwrap()
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(3)
}

async fn seed(pool: &PgPool, with_handoff: bool) -> ClaimRequest {
    let key = FrequencyKey {
        symbol: "BTCUSDT".to_owned(),
        initialization_id: Uuid::now_v7(),
        recovery_generation: 0,
        registration_id: Uuid::now_v7(),
    };
    let boot = Uuid::now_v7();
    let domain = ClockDomain::new(boot, -2, 345).unwrap();
    let audit_at = DateTime::<Utc>::from_timestamp(1_794_614_400, 123_456_000).unwrap();
    let prior = PriorClaim {
        revision: 2,
        identity: ClaimIdentity {
            claim_id: Uuid::now_v7(),
            owner: Uuid::now_v7(),
            digest: [0x71; 32],
            captured: tick(domain, CLAIM_TICK, 0),
            audit_at,
        },
        eligibility: PriorEligibility::SameDomain,
        handoff: with_handoff.then(|| tick(domain, HANDOFF_TICK, 0)),
        result: with_handoff.then_some(ClaimResult::Success),
    };

    sqlx::query(
        "INSERT INTO n5_registry_root VALUES
           (1, 38, 1, 1, true, 1, $1, $2, 0, 'normal', NULL, CURRENT_TIMESTAMP)",
    )
    .bind([0x31_u8; 32].as_slice())
    .bind(key.initialization_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO n5_refresh_keys VALUES
           ('binance:spot', $1, 'commission', 1, $2, 1, CURRENT_TIMESTAMP, 'claim-fixture')",
    )
    .bind(&key.symbol)
    .bind(key.registration_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO n5_refresh_state (
             venue_id, symbol, descriptor, revision, claim_id, owner_process, content_digest,
             claim_domain, claim_tick, claim_audit_at, eligibility_kind,
             handoff_domain, handoff_tick, result_class, snapshot_claim_id, snapshot_revision,
             snapshot_owner, snapshot_tenant, snapshot_scope, snapshot_uid, snapshot_domain,
             snapshot_tick, snapshot_as_of, snapshot_validation_version, snapshot_hash,
             snapshot_payload
         ) VALUES (
             'binance:spot', $1, 'commission', 2, $2, $3, $4,
             ROW('boottime', $5::uuid, -2::bigint, 345::integer)::budget_clock_domain_v38,
             $6::text::numeric, $7, 'same_domain',
             CASE WHEN $8::boolean THEN
               ROW('boottime', $5::uuid, -2::bigint, 345::integer)::budget_clock_domain_v38
             END,
             $9::text::numeric, $10, $11, $12, $13, $14, 'original-scope',
             18446744073709551615::numeric,
             ROW('boottime', $5::uuid, -2::bigint, 345::integer)::budget_clock_domain_v38,
             99000000000::numeric, $7, 1, $15, $16
         )",
    )
    .bind(&key.symbol)
    .bind(prior.identity.claim_id)
    .bind(prior.identity.owner)
    .bind(prior.identity.digest.as_slice())
    .bind(boot)
    .bind(prior.identity.captured.nanoseconds().to_string())
    .bind(audit_at)
    .bind(with_handoff)
    .bind(prior.handoff.map(|h| h.nanoseconds().to_string()))
    .bind(with_handoff.then_some("success"))
    .bind(if with_handoff {
        prior.identity.claim_id
    } else {
        Uuid::now_v7()
    })
    .bind(if with_handoff { 2_i64 } else { 1_i64 })
    .bind(if with_handoff {
        prior.identity.owner
    } else {
        Uuid::now_v7()
    })
    .bind(Uuid::now_v7())
    .bind(SNAPSHOT_HASH.as_slice())
    .bind(SNAPSHOT)
    .execute(pool)
    .await
    .unwrap();

    let threshold = if with_handoff {
        HANDOFF_TICK + DAY
    } else {
        CLAIM_TICK + 60 + DAY
    };
    ClaimRequest {
        key,
        prior,
        next: ClaimIdentity {
            claim_id: Uuid::now_v7(),
            owner: Uuid::now_v7(),
            digest: [0x72; 32],
            captured: tick(domain, threshold, 0),
            audit_at: audit_at + chrono::Duration::seconds(1),
        },
    }
}

struct Measured {
    proxy: DbProxy,
    pool: PgPool,
    trace: Trace,
}

impl Measured {
    async fn start(fixture: &PgPool) -> Self {
        let options = fixture.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await
            .unwrap()
            .next()
            .unwrap();
        assert!(upstream.ip().is_loopback(), "native fixture must use isolated loopback");
        let proxy = DbProxy::start(upstream).await.unwrap();
        proxy.enable_query_trace(64);
        proxy.enable_diagnostics();
        let url = database_url_through(
            &std::env::var("DATABASE_URL").unwrap(),
            proxy.addr(),
            options.get_database().unwrap(),
        )
        .unwrap();
        let pool = PgPoolOptions::new().max_connections(1).connect_lazy(&url).unwrap();
        Self { proxy, pool, trace: Trace::default() }
    }

    async fn claim(
        &self,
        request: ClaimRequest,
        until: Instant,
    ) -> Result<ClaimOutcome, BudgetError> {
        claim(&self.pool, request, until, &self.trace).await
    }

    fn wire(&self, statements: u64, connections: u64, commits: u64) -> Vec<String> {
        let stats = self.proxy.stats();
        assert_eq!(stats.statements, statements);
        assert_eq!(stats.connections, connections);
        assert_eq!(stats.commits, commits);
        assert_eq!(self.trace.issued(), statements);
        assert_eq!(self.proxy.query_trace_dropped(), 0);
        let commands: Vec<_> = self
            .proxy
            .query_trace()
            .iter()
            .map(|sql| sql.trim().to_ascii_uppercase())
            .collect();
        assert_eq!(commands.len(), usize::try_from(statements).unwrap());
        assert!(commands.iter().all(|sql| !sql.starts_with("ROLLBACK")), "{commands:?}");
        commands
    }

    async fn retired(&self, fixture: &PgPool, connections: usize) {
        timeout(Duration::from_secs(3), async {
            loop {
                let pids: Vec<_> = self
                    .proxy
                    .diagnostic_identities()
                    .unwrap()
                    .iter()
                    .map(|identity| identity.pid.expect("each authenticated backend has a PID"))
                    .collect();
                if pids.len() == connections
                    && self.proxy.closed_relays() == u64::try_from(connections).unwrap()
                    && sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM pg_stat_activity
                         WHERE datname = current_database() AND pid = ANY($1)",
                    )
                    .bind(pids)
                    .fetch_one(fixture)
                    .await
                    .unwrap()
                        == 0
                {
                    return;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("all measured native backends retired");
    }
}

fn assert_claim_sql(sql: &[String], committed: bool) {
    assert_eq!(sql.len(), if committed { 4 } else { 3 });
    assert!(sql[0].starts_with("BEGIN"), "{sql:?}");
    assert!(sql[1].contains("LOCK_TIMEOUT"), "{sql:?}");
    assert!(sql[2].contains("UPDATE N5_REFRESH_STATE"), "{sql:?}");
    if committed {
        assert_eq!(sql[3].trim_end_matches(';'), "COMMIT");
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn cold_raw_read_projects_only_active_claim_columns(pool: PgPool) {
    let seeded = seed(&pool, true).await;
    let measured = Measured::start(&pool).await;
    let active = std::slice::from_ref(&seeded.key.symbol);
    let rows = super::cold_load::read_rows(&measured.pool, active, deadline(), &measured.trace)
        .await
        .unwrap();
    let [row] = rows.rows() else {
        panic!("expected one registered key");
    };
    assert!(row.try_get::<bool>("active").unwrap());
    assert!(!row.try_get::<bool>("invalid_active_projection").unwrap());
    assert_eq!(row.try_get::<Uuid>("claim_id").unwrap(), seeded.prior.identity.claim_id);
    assert_eq!(row.try_get::<String>("snapshot_uid").unwrap(), u64::MAX.to_string());
    assert_eq!(row.try_get::<Vec<u8>>("snapshot_payload").unwrap(), SNAPSHOT);

    let rows = super::cold_load::read_rows(
        &measured.pool,
        &["NOTREGISTERED".to_owned()],
        deadline(),
        &measured.trace,
    )
    .await
    .unwrap();
    let [row] = rows.rows() else {
        panic!("the compact manifest row was lost");
    };
    assert!(!row.try_get::<bool>("active").unwrap());
    assert_eq!(row.try_get::<i64>("state_revision").unwrap(), seeded.prior.revision);
    assert!(row.try_get::<Option<Uuid>>("claim_id").unwrap().is_none());
    assert!(row.try_get::<Option<Vec<u8>>>("snapshot_payload").unwrap().is_none());

    let sql = measured.wire(2, 2, 0);
    assert!(sql
        .iter()
        .all(|sql| sql.starts_with("-- A SINGLE ANCHORED READ") && sql.contains("LIMIT 4097")));
    measured.retired(&pool, 2).await;
}

fn assert_observation(actual: &ClaimObservation, revision: i64, identity: &ClaimIdentity) {
    assert_eq!(actual.revision, revision);
    assert_eq!(actual.identity.claim_id, identity.claim_id);
    assert_eq!(actual.identity.owner, identity.owner);
    assert_eq!(actual.identity.digest, identity.digest);
    assert_eq!(actual.identity.captured, identity.captured);
    assert_eq!(actual.identity.audit_at, identity.audit_at);
}

async fn snapshot(pool: &PgPool) -> String {
    sqlx::query_scalar(
        "SELECT to_jsonb(ROW(snapshot_claim_id, snapshot_revision, snapshot_owner, snapshot_tenant,
                              snapshot_scope, snapshot_uid, snapshot_domain, snapshot_tick,
                              snapshot_as_of, snapshot_validation_version, snapshot_hash,
                              snapshot_payload))::text
         FROM n5_refresh_state WHERE venue_id='binance:spot' AND symbol='BTCUSDT'
                                   AND descriptor='commission'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn unchanged_row(pool: &PgPool) -> String {
    sqlx::query_scalar(
        "SELECT to_jsonb(s)::text FROM n5_refresh_state s
         WHERE venue_id='binance:spot' AND symbol='BTCUSDT' AND descriptor='commission'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn assert_native_claim(pool: &PgPool, revision: i64, identity: &ClaimIdentity) {
    let row = sqlx::query(
        "SELECT revision, claim_id, owner_process, content_digest, claim_tick::text,
                claim_audit_at, (claim_domain).kind, (claim_domain).boot_uuid,
                (claim_domain).offset_seconds, (claim_domain).offset_nanoseconds,
                eligibility_kind, num_nulls(handoff_domain, handoff_tick, result_class) AS cleared
         FROM n5_refresh_state WHERE venue_id='binance:spot' AND symbol='BTCUSDT'
                                   AND descriptor='commission'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>("revision"), revision);
    assert_eq!(row.get::<Uuid, _>("claim_id"), identity.claim_id);
    assert_eq!(row.get::<Uuid, _>("owner_process"), identity.owner);
    assert_eq!(row.get::<Vec<u8>, _>("content_digest"), identity.digest);
    assert_eq!(row.get::<String, _>("claim_tick"), identity.captured.nanoseconds().to_string());
    assert_eq!(row.get::<DateTime<Utc>, _>("claim_audit_at"), identity.audit_at);
    let domain = identity.captured.domain();
    let (seconds, nanos) = domain.offset();
    assert_eq!(row.get::<String, _>("kind"), "boottime");
    assert_eq!(row.get::<Uuid, _>("boot_uuid"), domain.boot());
    assert_eq!(row.get::<i64, _>("offset_seconds"), seconds);
    assert_eq!(row.get::<i32, _>("offset_nanoseconds"), i32::try_from(nanos).unwrap());
    assert_eq!(row.get::<String, _>("eligibility_kind"), "same_domain");
    assert_eq!(row.get::<i32, _>("cleared"), 3);
}

async fn threshold(pool: PgPool, with_handoff: bool) {
    let request = seed(&pool, with_handoff).await;
    let previous_row = unchanged_row(&pool).await;
    let previous_snapshot = snapshot(&pool).await;
    let measured = Measured::start(&pool).await;
    let mut early = request.clone();
    let seconds = i64::try_from(request.next.captured.nanoseconds() / 1_000_000_000).unwrap();
    early.next.captured = tick(request.next.captured.domain(), seconds - 1, 999_999_999);

    assert!(matches!(measured.claim(early, deadline()).await, Ok(ClaimOutcome::Refused)));
    assert_claim_sql(&measured.wire(3, 1, 0), false);
    assert_eq!(unchanged_row(&pool).await, previous_row);

    let Ok(ClaimOutcome::Stored(observation)) = measured.claim(request.clone(), deadline()).await
    else {
        panic!("claim at exact threshold must be stored");
    };
    assert_observation(&observation, 3, &request.next);
    assert_native_claim(&pool, 3, &request.next).await;
    assert_eq!(snapshot(&pool).await, previous_snapshot);
    measured.retired(&pool, 2).await;
    assert_claim_sql(&measured.wire(7, 2, 1)[3..], true);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn handoff_threshold_and_snapshot_preservation(pool: PgPool) {
    threshold(pool, true).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn fallback_threshold_and_snapshot_preservation(pool: PgPool) {
    threshold(pool, false).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn stale_claim_provenance_and_clock_domain_leave_row_unchanged(pool: PgPool) {
    let request = seed(&pool, true).await;
    let previous_row = unchanged_row(&pool).await;
    let measured = Measured::start(&pool).await;

    let mut wrong_domain = request.clone();
    wrong_domain.next.captured = tick(ClockDomain::new(Uuid::now_v7(), -2, 345).unwrap(), DAY, 0);
    assert!(matches!(
        measured.claim(wrong_domain, deadline()).await,
        Err(BudgetError::Invalid)
    ));
    assert!(matches!(
        measured.claim(request.clone(), Instant::now()).await,
        Err(BudgetError::Expired),
    ));
    measured.wire(0, 0, 0);

    let mut wrong_registration = request.clone();
    wrong_registration.key.registration_id = Uuid::now_v7();
    let mut wrong_initialization = request.clone();
    wrong_initialization.key.initialization_id = Uuid::now_v7();
    let mut wrong_generation = request.clone();
    wrong_generation.key.recovery_generation = 1;
    let mut wrong_symbol = request.clone();
    wrong_symbol.key.symbol = "ETHUSDT".to_owned();
    let mut wrong_digest = request.clone();
    wrong_digest.prior.identity.digest = [0x11; 32];
    let mut wrong_handoff = request.clone();
    wrong_handoff.prior.handoff = Some(tick(request.next.captured.domain(), HANDOFF_TICK - 1, 0));
    let mut wrong_result = request;
    wrong_result.prior.result = Some(ClaimResult::NoSend);

    for (index, wrong) in [
        wrong_registration,
        wrong_initialization,
        wrong_generation,
        wrong_symbol,
        wrong_digest,
        wrong_handoff,
        wrong_result,
    ]
    .into_iter()
    .enumerate()
    {
        assert!(matches!(measured.claim(wrong, deadline()).await, Ok(ClaimOutcome::Refused)));
        let expected_connections = u64::try_from(index + 1).unwrap();
        let commands = measured.wire(3 * expected_connections, expected_connections, 0);
        assert_claim_sql(&commands[index * 3..], false);
    }
    measured.retired(&pool, 7).await;
    measured.wire(21, 7, 0);
    assert_eq!(unchanged_row(&pool).await, previous_row);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn refuses_audit_timestamps_that_postgres_would_normalize(pool: PgPool) {
    let request = seed(&pool, true).await;
    let previous_row = unchanged_row(&pool).await;
    let measured = Measured::start(&pool).await;
    for subsec in [1_000_000_000, 123_456_789] {
        let audit_at = DateTime::<Utc>::from_timestamp(59, subsec).unwrap();
        let mut wrong_prior = request.clone();
        wrong_prior.prior.identity.audit_at = audit_at;
        let mut wrong_next = request.clone();
        wrong_next.next.audit_at = audit_at;
        for wrong in [wrong_prior, wrong_next] {
            assert!(matches!(measured.claim(wrong, deadline()).await, Err(BudgetError::Invalid)));
        }
    }
    measured.wire(0, 0, 0);
    assert_eq!(unchanged_row(&pool).await, previous_row);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn append_preserves_key_and_contenders_share_one_winner(pool: PgPool) {
    let request = seed(&pool, false).await;
    let original_snapshot = snapshot(&pool).await;
    // The append changes the manifest while retaining this key's registration.
    let mut append = pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE n5_registry_root
         SET registered_count = 2, manifest_revision = 2, canonical_digest = $1
         WHERE registry_id = 1",
    )
    .bind([0x32_u8; 32].as_slice())
    .execute(&mut *append)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO n5_refresh_keys VALUES
         ('binance:spot', 'ETHUSDT', 'commission', 1, $1, 2, CURRENT_TIMESTAMP, 'claim-fixture')",
    )
    .bind(Uuid::now_v7())
    .execute(&mut *append)
    .await
    .unwrap();
    append.commit().await.unwrap();

    // Another owner/content alias still uses the exact same frequency key.
    let mut conflicting = request.clone();
    conflicting.next.claim_id = Uuid::now_v7();
    conflicting.next.owner = Uuid::now_v7();
    conflicting.next.digest = [0x73; 32];
    let measured = Measured::start(&pool).await;
    let until = deadline();
    let (original, duplicate, different) = tokio::join!(
        measured.claim(request.clone(), until),
        measured.claim(request.clone(), until),
        measured.claim(conflicting.clone(), until),
    );
    let mut stored = 0;
    for (result, expected) in [
        (original, &request.next),
        (duplicate, &request.next),
        (different, &conflicting.next),
    ] {
        match result {
            Ok(ClaimOutcome::Stored(observation)) => {
                stored += 1;
                assert_observation(&observation, 3, expected);
                assert_native_claim(&pool, 3, expected).await;
            },
            Ok(ClaimOutcome::Refused) => {},
            _ => panic!("concurrent CAS must produce a stored observation or a refusal"),
        }
    }
    assert_eq!(stored, 1);
    assert_eq!(snapshot(&pool).await, original_snapshot);
    measured.retired(&pool, 3).await;
    measured.wire(10, 3, 1);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn lost_commit_ack_uses_exactly_one_positive_confirmation(pool: PgPool) {
    let request = seed(&pool, true).await;
    let original_snapshot = snapshot(&pool).await;
    let measured = Measured::start(&pool).await;
    measured.proxy.set_mode(ProxyMode::DropCommitAckOf("UPDATE n5_refresh_state"));

    let Ok(ClaimOutcome::Uncertain(original)) = measured.claim(request.clone(), deadline()).await
    else {
        panic!("a dropped matching COMMIT answer must report uncertainty");
    };
    assert_eq!(measured.proxy.stats().commit_acks_dropped, 1);
    assert_claim_sql(&measured.wire(4, 1, 1), true);
    let confirmed = confirm_once(&measured.pool, original, &measured.trace).await.unwrap().unwrap();
    assert_observation(&confirmed, 3, &request.next);
    assert_native_claim(&pool, 3, &request.next).await;
    assert_eq!(snapshot(&pool).await, original_snapshot);
    measured.retired(&pool, 2).await;
    assert_confirm_sql(&measured.wire(5, 2, 1)[4]);
}

fn assert_confirm_sql(sql: &str) {
    assert!(sql.starts_with("SELECT") || sql.starts_with("WITH"), "{sql}");
    assert!(sql.contains("N5_REFRESH_STATE"), "{sql}");
    assert!(!sql.contains("UPDATE N5_REFRESH_STATE"), "{sql}");
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn missing_confirmation_cannot_rule_out_a_late_original_commit(pool: PgPool) {
    let request = seed(&pool, true).await;
    let original_row = unchanged_row(&pool).await;
    let original_snapshot = snapshot(&pool).await;
    let measured = Measured::start(&pool).await;
    // A real deferred trigger stalls COMMIT after the CAS has returned its receipt.
    // Hold the backend only long enough to observe a separate read of the old row.
    sqlx::raw_sql(
        "CREATE FUNCTION n5_claim_test_block_commit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(730061); RETURN NEW; END;
         $$;
         CREATE CONSTRAINT TRIGGER n5_claim_test_block_commit AFTER UPDATE ON n5_refresh_state
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION n5_claim_test_block_commit();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(730061)")
        .execute(&mut *blocker)
        .await
        .unwrap();

    let sever_while_waiting = async {
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
                           WHERE pid = ANY($1) AND wait_event_type = 'Lock'
                                               AND wait_event = 'advisory')",
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
        .expect("the exact native COMMIT must wait on the deferred trigger");
    };
    let (outcome, ()) =
        tokio::join!(measured.claim(request.clone(), deadline()), sever_while_waiting);
    let Ok(ClaimOutcome::Uncertain(original)) = outcome else {
        panic!("the original COMMIT has no acknowledged result");
    };
    assert_claim_sql(&measured.wire(4, 1, 1), true);
    measured.proxy.set_mode(ProxyMode::Pass);
    let confirmation = confirm_once(&measured.pool, original, &measured.trace).await.unwrap();
    assert!(confirmation.is_none());
    assert_eq!(unchanged_row(&pool).await, original_row);
    blocker.commit().await.unwrap();

    measured.retired(&pool, 2).await;
    assert_confirm_sql(&measured.wire(5, 2, 1)[4]);
    assert_native_claim(&pool, 3, &request.next).await;
    assert_eq!(snapshot(&pool).await, original_snapshot);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated PostgreSQL"]
async fn postgres_lock_and_statement_timeouts_hard_retire(pool: PgPool) {
    let request = seed(&pool, true).await;
    let previous_row = unchanged_row(&pool).await;
    let measured = Measured::start(&pool).await;

    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT revision FROM n5_refresh_state WHERE symbol='BTCUSDT' FOR UPDATE")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    assert!(measured.claim(request.clone(), deadline()).await.is_err());
    assert_claim_sql(&measured.wire(3, 1, 0), false);
    assert!(measured
        .proxy
        .diagnostics()
        .unwrap()
        .records
        .iter()
        .any(|record| { record.sqlstate == Some(*b"55P03") }));
    measured.retired(&pool, 1).await;

    // The enclosing 180ms deadline ends while the CAS is still blocked, ahead
    // of PostgreSQL's 300ms lock timeout. Watch through native retirement so a
    // cancelled Rust future cannot hide a later cleanup statement.
    let caller_deadline = Instant::now() + Duration::from_millis(180);
    assert!(matches!(
        measured.claim(request.clone(), caller_deadline).await,
        Err(BudgetError::Expired),
    ));
    measured.retired(&pool, 2).await;
    assert_claim_sql(&measured.wire(6, 2, 0)[3..], false);
    blocker.rollback().await.unwrap();

    sqlx::raw_sql(
        "CREATE FUNCTION n5_claim_test_delay_statement() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_sleep(10); RETURN NEW; END;
         $$;
         CREATE TRIGGER n5_claim_test_delay_statement BEFORE UPDATE ON n5_refresh_state
         FOR EACH ROW EXECUTE FUNCTION n5_claim_test_delay_statement();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(measured.claim(request, deadline()).await.is_err());
    measured.retired(&pool, 3).await;
    assert_claim_sql(&measured.wire(9, 3, 0)[6..], false);
    assert!(measured
        .proxy
        .diagnostics()
        .unwrap()
        .records
        .iter()
        .any(|record| { record.sqlstate == Some(*b"57014") }));
    assert_eq!(unchanged_row(&pool).await, previous_row);
}

#[cfg(test)]
#[path = "result_tests.rs"]
mod result_tests;

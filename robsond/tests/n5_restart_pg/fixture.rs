use chrono::{DateTime, Utc};
use robson_exec::venue_clock::TickCapture;
use sqlx::PgPool;
use uuid::Uuid;

use super::{Key, RequestKey};

pub(super) async fn seed_registry(
    pool: &PgPool,
    keys: &[Key],
    declared_count: i32,
    digest: [u8; 32],
    initialized: bool,
    recovery_state: &str,
) {
    let mut tx = pool.begin().await.unwrap();
    let manifest_revision = (i64::from(declared_count) + 63) / 64;
    sqlx::query(
        "INSERT INTO n5_registry_root (
             registry_id, schema_version, manifest_version, manifest_revision,
             initialized, registered_count, canonical_digest, initialization_id,
             recovery_generation, recovery_state, registered_at
         ) VALUES (1, 38, 1, $1, $2, $3, $4, $5, 0, $6, CURRENT_TIMESTAMP)",
    )
    .bind(manifest_revision)
    .bind(initialized)
    .bind(declared_count)
    .bind(digest.as_slice())
    .bind(Uuid::now_v7())
    .bind(recovery_state)
    .execute(&mut *tx)
    .await
    .unwrap();

    let venues: Vec<_> = keys.iter().map(|key| key.venue.as_str()).collect();
    let symbols: Vec<_> = keys.iter().map(|key| key.symbol.as_str()).collect();
    let descriptors: Vec<_> = keys.iter().map(|key| key.descriptor.as_str()).collect();
    let registration_ids: Vec<_> = keys.iter().map(|_| Uuid::now_v7()).collect();
    let inserted = sqlx::query(
        "INSERT INTO n5_refresh_keys (
             venue_id, symbol, descriptor, registry_id, registration_id,
             registered_revision, registered_at, provenance
         )
         SELECT venue, symbol, descriptor, 1, registration_id,
                1 + (key_number - 1) / 64, CURRENT_TIMESTAMP, 'n5-restart-fixture'
         FROM UNNEST($1::text[], $2::text[], $3::text[], $4::uuid[]) WITH ORDINALITY
              AS fixture(venue, symbol, descriptor, registration_id, key_number)",
    )
    .bind(venues)
    .bind(symbols)
    .bind(descriptors)
    .bind(registration_ids)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(inserted.rows_affected(), u64::try_from(keys.len()).unwrap());
    tx.commit().await.unwrap();
}

pub(super) async fn seed_empty(pool: &PgPool, key: &Key) {
    sqlx::query(
        "INSERT INTO n5_refresh_state (venue_id, symbol, descriptor, revision)
         VALUES ($1, $2, $3, 0)",
    )
    .bind(&key.venue)
    .bind(&key.symbol)
    .bind(&key.descriptor)
    .execute(pool)
    .await
    .unwrap();
}

pub(super) async fn seed_success(
    pool: &PgPool,
    request: &RequestKey,
    capture: TickCapture,
    as_of: DateTime<Utc>,
    payload: &[u8],
    evidence_hash: &[u8; 32],
) -> Uuid {
    let claim_id = Uuid::now_v7();
    let owner = Uuid::now_v7();
    let domain = capture.domain();
    let (seconds, nanoseconds) = domain.offset();
    let offset_ns = i32::try_from(nanoseconds).unwrap();
    let tick = capture.nanoseconds().to_string();

    // Binding the integer coordinates as text before the NUMERIC cast keeps
    // the complete TickCapture and UID ranges without floating-point conversion.
    sqlx::query(
        "INSERT INTO n5_refresh_state (
             venue_id, symbol, descriptor, revision, claim_id, owner_process,
             content_digest, claim_domain, claim_tick, claim_audit_at, eligibility_kind,
             handoff_domain, handoff_tick, result_class, snapshot_claim_id,
             snapshot_revision, snapshot_owner, snapshot_tenant, snapshot_scope,
             snapshot_uid, snapshot_domain, snapshot_tick, snapshot_as_of,
             snapshot_validation_version, snapshot_hash, snapshot_payload
         ) VALUES (
             $1, $2, $3, 1, $4, $5, $6,
             ROW('boottime', $7::uuid, $8::bigint, $9::integer)::budget_clock_domain_v38,
             $10::text::numeric, $11, 'first_use',
             ROW('boottime', $7::uuid, $8::bigint, $9::integer)::budget_clock_domain_v38,
             $10::text::numeric, 'success', $4, 1, $5, $12, $13,
             $14::text::numeric,
             ROW('boottime', $7::uuid, $8::bigint, $9::integer)::budget_clock_domain_v38,
             $10::text::numeric, $11, 1, $15, $16
         )",
    )
    .bind(&request.key.venue)
    .bind(&request.key.symbol)
    .bind(&request.key.descriptor)
    .bind(claim_id)
    .bind(owner)
    .bind(&[0x51_u8; 32][..])
    .bind(domain.boot())
    .bind(seconds)
    .bind(offset_ns)
    .bind(tick)
    .bind(as_of)
    .bind(request.tenant)
    .bind(&request.scope)
    .bind(request.uid.to_string())
    .bind(evidence_hash.as_slice())
    .bind(payload)
    .execute(pool)
    .await
    .unwrap();
    claim_id
}

pub(super) async fn advance_unresolved(pool: &PgPool, key: &Key, capture: TickCapture) {
    let domain = capture.domain();
    let (seconds, nanoseconds) = domain.offset();
    let offset_ns = i32::try_from(nanoseconds).unwrap();
    let updated = sqlx::query(
        "UPDATE n5_refresh_state
         SET revision = 2, claim_id = $4, owner_process = $5, content_digest = $6,
             claim_domain =
                 ROW('boottime', $7::uuid, $8::bigint, $9::integer)::budget_clock_domain_v38,
             claim_tick = $10::text::numeric, claim_audit_at = CURRENT_TIMESTAMP,
             eligibility_kind = 'same_domain', handoff_domain = NULL, handoff_tick = NULL,
             result_class = NULL
         WHERE venue_id = $1 AND symbol = $2 AND descriptor = $3
           AND revision = 1 AND result_class = 'success'",
    )
    .bind(&key.venue)
    .bind(&key.symbol)
    .bind(&key.descriptor)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(&[0x52_u8; 32][..])
    .bind(domain.boot())
    .bind(seconds)
    .bind(offset_ns)
    .bind(capture.nanoseconds().to_string())
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(updated.rows_affected(), 1);
}

//! Test-only N5 same-domain cold-load witness over the exact M38 fixture.
//! The result is observed evidence, never a CostAgeAuthority or runtime permit.
#![cfg(feature = "postgres")]

#[path = "n5_restart_pg/fixture.rs"]
mod fixture;

use std::{
    collections::BTreeSet,
    process::Stdio,
    time::{Duration, Instant},
};

use chrono::{DateTime, TimeZone, Utc};
use robson_domain::value_objects::Symbol;
use robson_exec::{
    balance_costs::{
        exact::ExactDecimal, CommissionComponents, CostCandidate, CostIdentity,
        COST_VALIDATION_VERSION,
    },
    venue::VenueId,
    venue_clock::{ClockDomain, ClockError, NativeClock, TickCapture},
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool};
use tokio::process::Command;
use uuid::Uuid;

use fixture::{advance_unresolved, seed_empty, seed_registry, seed_success};

const LOAD_SQL: &str = include_str!("n5_restart_pg/load.sql");
const MAX_KEYS: usize = 4096;
const MAX_ACTIVE: usize = 64;
const MAX_PAYLOAD: usize = 16 * 1024;
const MAX_RESULT: usize = 8 * 1024 * 1024;
// The exact M38 constraints bound all text metadata (route 64+64, scope 128,
// enums and flattened integral ticks/UID). Fixed fields and PostgreSQL column
// prefixes fit comfortably in this per-row upper bound before payloads.
const METADATA_PER_ROW: usize = 1536;
const _: () = assert!((MAX_KEYS + 1) * METADATA_PER_ROW + MAX_ACTIVE * MAX_PAYLOAD <= MAX_RESULT);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const CHILD_REQUEST: &str = "ROBSON_TEST_N5_RESTART_REQUEST";
const CHILD_URL: &str = "ROBSON_TEST_N5_RESTART_URL";
const CHILD_MARK: &str = "N5_RESTART_CHILD=";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Key {
    venue: String,
    symbol: String,
    descriptor: String,
}

impl Key {
    fn spot(symbol: impl Into<String>) -> Self {
        Self {
            venue: VenueId::BINANCE_SPOT.into(),
            symbol: symbol.into(),
            descriptor: "commission".into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct RequestKey {
    key: Key,
    tenant: Uuid,
    scope: String,
    uid: u64,
    base: String,
    quote: String,
}

impl RequestKey {
    fn btc() -> Self {
        Self {
            key: Key::spot("BTCUSDT"),
            tenant: Uuid::now_v7(),
            scope: "n5-fixture".into(),
            uid: 7,
            base: "BTC".into(),
            quote: "USDT".into(),
        }
    }
}

fn manifest_digest(keys: &[Key]) -> [u8; 32] {
    // The fixture format is versioned and local; the durable producer must
    // separately fix its format. Sorting uses complete native key bytes.
    let mut keys: Vec<_> = keys.iter().collect();
    keys.sort_unstable();
    let mut hash = Sha256::new();
    hash.update(b"robson:test:n5-manifest:v1\0");
    for key in keys {
        for value in [&key.venue, &key.symbol, &key.descriptor] {
            hash.update(u32::try_from(value.len()).unwrap().to_be_bytes());
            hash.update(value.as_bytes());
        }
    }
    hash.finalize().into()
}

#[derive(Debug, thiserror::Error)]
enum Refusal {
    #[error("N5 fixture database rejected the cold read")]
    Database(#[from] sqlx::Error),
    #[error("no usable registry root")]
    Registry,
    #[error("registry has more than 4096 persisted keys")]
    Overflow,
    #[error("requested key set is absent or exceeds the fixture bounds")]
    Requested,
    #[error("manifest identity, digest or key-to-state completeness disagrees")]
    Manifest,
    #[error("compact native result exceeds the byte envelope")]
    Envelope,
    #[error("there is no current positive original result")]
    Unresolved,
    #[error("original receipt fields, timestamp or hash disagree")]
    Receipt,
    #[error("native clock capture refused")]
    Clock(#[from] ClockError),
    #[error("original receipt is at least 24 hours old")]
    Expired,
}

#[derive(sqlx::FromRow)]
struct ColdRow {
    root_id: Option<i16>,
    schema_version: Option<i16>,
    manifest_version: Option<i16>,
    manifest_revision: Option<i64>,
    initialized: Option<bool>,
    registered_count: Option<i32>,
    canonical_digest: Option<Vec<u8>>,
    initialization_id: Option<Uuid>,
    recovery_generation: Option<i64>,
    recovery_state: Option<String>,
    venue_id: Option<String>,
    symbol: Option<String>,
    descriptor: Option<String>,
    key_registry: Option<i16>,
    registration_id: Option<Uuid>,
    registered_revision: Option<i64>,
    state_revision: Option<i64>,
    active: bool,
    claim_id: Option<Uuid>,
    owner_process: Option<Uuid>,
    result_class: Option<String>,
    snapshot_claim_id: Option<Uuid>,
    snapshot_revision: Option<i64>,
    snapshot_owner: Option<Uuid>,
    snapshot_tenant: Option<Uuid>,
    snapshot_scope: Option<String>,
    snapshot_uid: Option<String>,
    clock_kind: Option<String>,
    boot_uuid: Option<Uuid>,
    offset_seconds: Option<i64>,
    offset_ns: Option<i32>,
    snapshot_tick: Option<String>,
    snapshot_as_of: Option<DateTime<Utc>>,
    validation_version: Option<i16>,
    snapshot_hash: Option<Vec<u8>>,
    snapshot_payload: Option<Vec<u8>>,
    same_receipt_domain: bool,
}

impl ColdRow {
    fn key(&self) -> Option<Key> {
        Some(Key {
            venue: self.venue_id.clone()?,
            symbol: self.symbol.clone()?,
            descriptor: self.descriptor.clone()?,
        })
    }
}

struct ColdRead {
    selected: Vec<ColdRow>,
    registered: usize,
    payload_bytes: usize,
}

async fn load(conn: &mut PgConnection, requests: &[Key]) -> Result<ColdRead, Refusal> {
    let unique: BTreeSet<_> = requests.iter().collect();
    if requests.is_empty() || requests.len() > MAX_ACTIVE || unique.len() != requests.len() {
        return Err(Refusal::Requested);
    }
    let venue: Vec<_> = requests.iter().map(|k| k.venue.as_str()).collect();
    let symbol: Vec<_> = requests.iter().map(|k| k.symbol.as_str()).collect();
    let descriptor: Vec<_> = requests.iter().map(|k| k.descriptor.as_str()).collect();
    let rows: Vec<ColdRow> = sqlx::query_as(LOAD_SQL)
        .bind(venue)
        .bind(symbol)
        .bind(descriptor)
        .fetch_all(conn)
        .await?;
    let root = rows.first().ok_or(Refusal::Registry)?;
    if root.root_id != Some(1)
        || root.schema_version != Some(38)
        || root.manifest_version != Some(1)
        || root.recovery_state.as_deref() != Some("normal")
        || root.initialized != Some(true)
        || root.initialization_id.is_none_or(|id| id.is_nil())
        || root.recovery_generation.is_none_or(|value| value < 0)
    {
        return Err(Refusal::Registry);
    }
    if rows.len() > MAX_KEYS {
        return Err(Refusal::Overflow);
    }

    let revision = root.manifest_revision.ok_or(Refusal::Manifest)?;
    let mut keys = Vec::with_capacity(rows.len());
    let mut active = BTreeSet::new();
    let mut payload_bytes = 0;
    for row in &rows {
        if let Some(key) = row.key() {
            if row.key_registry != Some(1)
                || row.registration_id.is_none_or(|id| id.is_nil())
                || row.registered_revision.is_none_or(|value| value <= 0 || value > revision)
                || row.state_revision.is_none_or(|value| value < 0)
            {
                return Err(Refusal::Manifest);
            }
            if row.active && !active.insert(key.clone()) {
                return Err(Refusal::Manifest);
            }
            keys.push(key);
        }
        if let Some(bytes) = &row.snapshot_payload {
            if !row.active || bytes.len() > MAX_PAYLOAD {
                return Err(Refusal::Envelope);
            }
            payload_bytes += bytes.len();
        }
    }
    let registered = keys.len();
    if root.registered_count != i32::try_from(registered).ok()
        || BTreeSet::from_iter(keys.iter()).len() != registered
        || root.canonical_digest.as_deref() != Some(manifest_digest(&keys).as_slice())
    {
        return Err(Refusal::Manifest);
    }
    if active.iter().collect::<BTreeSet<_>>() != unique {
        return Err(Refusal::Requested);
    }
    if rows.len() * METADATA_PER_ROW + payload_bytes > MAX_RESULT {
        return Err(Refusal::Envelope);
    }
    Ok(ColdRead {
        selected: rows.into_iter().filter(|row| row.active).collect(),
        registered,
        payload_bytes,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    as_of: DateTime<Utc>,
    standard: [ExactDecimal; 4],
    tax: [ExactDecimal; 4],
    special: [ExactDecimal; 4],
    enabled_for_account: bool,
    enabled_for_symbol: bool,
    discount: ExactDecimal,
    discount_asset: String,
}

fn candidate(req: &RequestKey, data: Payload) -> Result<CostCandidate, Refusal> {
    let symbol = Symbol::new(req.base.clone(), req.quote.clone()).map_err(|_| Refusal::Receipt)?;
    if symbol.as_pair() != req.key.symbol {
        return Err(Refusal::Receipt);
    }
    let identity = CostIdentity::new(
        req.tenant,
        VenueId::new(&req.key.venue),
        req.scope.clone(),
        req.uid,
        symbol,
    )
    .map_err(|_| Refusal::Receipt)?;
    CostCandidate::commission(
        identity,
        data.as_of,
        CommissionComponents {
            standard: data.standard,
            tax: data.tax,
            special: data.special,
            enabled_for_account: data.enabled_for_account,
            enabled_for_symbol: data.enabled_for_symbol,
            discount: data.discount,
            discount_asset: data.discount_asset,
        },
    )
    .map_err(|_| Refusal::Receipt)
}

fn at_tick(domain: ClockDomain, tick: u128) -> Result<TickCapture, Refusal> {
    let whole = i64::try_from(tick / 1_000_000_000).map_err(|_| Refusal::Receipt)?;
    let remainder = (tick % 1_000_000_000) as i64;
    Ok(TickCapture::new(domain, whole, remainder)?)
}

#[derive(Debug, Serialize, Deserialize)]
struct OriginalReceipt {
    claim: Uuid,
    revision: i64,
    boot: Uuid,
    offset_seconds: i64,
    offset_ns: u32,
    original_tick_ns: u128,
    age_ns: u128,
    as_of: DateTime<Utc>,
    hash: [u8; 32],
    payload_hash: [u8; 32],
}

fn prove(row: &ColdRow, req: &RequestKey, now: TickCapture) -> Result<OriginalReceipt, Refusal> {
    let claim = row.claim_id.ok_or(Refusal::Unresolved)?;
    let revision = row.state_revision.filter(|value| *value > 0).ok_or(Refusal::Unresolved)?;
    if row.result_class.as_deref() != Some("success")
        || row.snapshot_claim_id != Some(claim)
        || row.snapshot_revision != Some(revision)
        || row.snapshot_owner != row.owner_process
        || row.owner_process.is_none()
        || !row.same_receipt_domain
    {
        return Err(Refusal::Unresolved);
    }
    if row.key().as_ref() != Some(&req.key)
        || row.snapshot_tenant != Some(req.tenant)
        || row.snapshot_scope.as_deref() != Some(req.scope.as_str())
        || row.snapshot_uid.as_deref().and_then(|s| s.parse::<u64>().ok()) != Some(req.uid)
        || row.validation_version != i16::try_from(COST_VALIDATION_VERSION).ok()
        || row.clock_kind.as_deref() != Some("boottime")
    {
        return Err(Refusal::Receipt);
    }
    let bytes = row.snapshot_payload.as_deref().ok_or(Refusal::Receipt)?;
    let payload: Payload = serde_json::from_slice(bytes).map_err(|_| Refusal::Receipt)?;
    let as_of = row.snapshot_as_of.ok_or(Refusal::Receipt)?;
    if payload.as_of != as_of {
        return Err(Refusal::Receipt);
    }
    let candidate = candidate(req, payload)?;
    let hash = *candidate.evidence().hash();
    if row.snapshot_hash.as_deref() != Some(hash.as_slice()) {
        return Err(Refusal::Receipt);
    }
    let boot = row.boot_uuid.ok_or(Refusal::Receipt)?;
    let offset_seconds = row.offset_seconds.ok_or(Refusal::Receipt)?;
    let offset_ns = row
        .offset_ns
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(Refusal::Receipt)?;
    let domain = ClockDomain::new(boot, offset_seconds, offset_ns)?;
    let tick = row
        .snapshot_tick
        .as_deref()
        .and_then(|t| t.parse::<u128>().ok())
        .ok_or(Refusal::Receipt)?;
    let age = now.elapsed_since(&at_tick(domain, tick)?)?;
    if age >= DAY {
        return Err(Refusal::Expired);
    }
    Ok(OriginalReceipt {
        claim,
        revision,
        boot,
        offset_seconds,
        offset_ns,
        original_tick_ns: tick,
        age_ns: age.as_nanos(),
        as_of,
        hash,
        payload_hash: Sha256::digest(bytes).into(),
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct ChildReport {
    pid: u32,
    elapsed_micros: u128,
    registered: usize,
    payload_bytes: usize,
    receipt: OriginalReceipt,
}

// The parent calls this explicit entry point in the same compiled artifact.
// A normal libtest enumeration has no child request and starts no operation.
#[test]
fn n5_restart_child_entry() {
    let Some(request) = std::env::var_os(CHILD_REQUEST) else {
        return;
    };
    let request: RequestKey = serde_json::from_str(request.to_str().unwrap()).unwrap();
    let url = std::env::var(CHILD_URL).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let report = rt.block_on(async {
        let start = Instant::now();
        let clock = NativeClock::initialize().unwrap();
        let mut conn = PgConnection::connect(&url).await.expect("child fixture connection");
        let cold = load(&mut conn, std::slice::from_ref(&request.key)).await.unwrap();
        let receipt = prove(&cold.selected[0], &request, clock.capture().unwrap()).unwrap();
        conn.close().await.unwrap();
        ChildReport {
            pid: std::process::id(),
            elapsed_micros: start.elapsed().as_micros(),
            registered: cold.registered,
            payload_bytes: cold.payload_bytes,
            receipt,
        }
    });
    println!("{CHILD_MARK}{}", serde_json::to_string(&report).unwrap());
}

async fn child(
    exe: &std::path::Path,
    url: &str,
    request: &RequestKey,
    proxy: &DbProxy,
) -> ChildReport {
    let mut command = Command::new(exe);
    command
        .args([
            "--exact",
            "n5_restart_child_entry",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(CHILD_URL, url)
        .env(CHILD_REQUEST, serde_json::to_string(request).unwrap())
        .env("RUST_MIN_STACK", "1048576")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let before = proxy.stats();
    let trace_start = proxy.query_trace().len();
    let process = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let pid = process.id().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), process.wait_with_output())
        .await
        .expect("bounded N5 child")
        .unwrap();
    assert!(result.status.success(), "N5 child exit {}", result.status);
    let output = String::from_utf8(result.stdout).unwrap();
    let lines: Vec<_> = output.lines().filter_map(|line| line.strip_prefix(CHILD_MARK)).collect();
    assert_eq!(lines.len(), 1, "exactly one actual child result");
    let report: ChildReport = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(report.pid, pid);
    let after = proxy.stats();
    let trace = proxy.query_trace();
    assert_eq!(after.connections - before.connections, 1);
    assert_eq!(after.statements - before.statements, 1, "physical SELECT including setup");
    assert_eq!(after.commits - before.commits, 0);
    assert_eq!(proxy.query_trace_dropped(), 0);
    assert_eq!(trace.len() - trace_start, 1);
    assert!(trace[trace_start].contains("bounded_keys AS MATERIALIZED"));
    println!("N5 physical child run pid={pid} statements=1 connections=1 commits=0 elapsed_us={} registered={} payload_bytes={}",
             report.elapsed_micros, report.registered, report.payload_bytes);
    report
}

fn original_as_of() -> DateTime<Utc> {
    Utc.timestamp_opt(1_794_614_400, 123_456_000).single().unwrap()
}

fn fixture_payload(as_of: DateTime<Utc>) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "as_of": as_of,
        "standard": ["0.001", "0.001", "0", "0"],
        "tax": ["0", "0", "0", "0"],
        "special": ["0", "0", "0", "0"],
        "enabled_for_account": false,
        "enabled_for_symbol": false,
        "discount": "0",
        "discount_asset": "BNB",
    }))
    .unwrap()
}

fn receipt_hash(req: &RequestKey, data: &[u8]) -> [u8; 32] {
    *candidate(req, serde_json::from_slice(data).unwrap()).unwrap().evidence().hash()
}

async fn read_local(pool: &PgPool, requested: &[Key]) -> Result<ColdRead, Refusal> {
    let mut conn = PgConnection::connect_with(&pool.connect_options()).await?;
    load(&mut conn, requested).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated DATABASE_URL"]
async fn n5_same_domain_cold_read_survives_real_process_restart(pool: PgPool) {
    let req = RequestKey::btc();
    let mut inactive = RequestKey::btc();
    inactive.key = Key::spot("ETHUSDT");
    inactive.base = "ETH".into();
    let keys = [inactive.key.clone(), req.key.clone()];
    seed_registry(&pool, &keys, 2, manifest_digest(&keys), true, "normal").await;
    let as_of = original_as_of();
    let payload = fixture_payload(as_of);
    let hash = receipt_hash(&req, &payload);
    let capture = NativeClock::initialize().unwrap().capture().unwrap();
    let claim = seed_success(&pool, &req, capture, as_of, &payload, &hash).await;
    seed_success(&pool, &inactive, capture, as_of, &payload, &receipt_hash(&inactive, &payload))
        .await;

    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await
        .unwrap()
        .next()
        .unwrap();
    assert!(upstream.ip().is_loopback());
    let proxy = DbProxy::start(upstream).await.unwrap();
    proxy.enable_query_trace(8);
    let url = database_url_through(
        &std::env::var("DATABASE_URL").unwrap(),
        proxy.addr(),
        options.get_database().unwrap(),
    )
    .unwrap();
    let exe = std::env::current_exe().unwrap();
    let first = child(&exe, &url, &req, &proxy).await;
    let second = child(&exe, &url, &req, &proxy).await;
    assert_ne!(first.pid, second.pid);
    assert_ne!(first.pid, std::process::id());
    assert_ne!(second.pid, std::process::id());
    let domain = capture.domain();
    let (s, ns) = domain.offset();
    for report in [&first, &second] {
        assert_eq!(report.registered, 2);
        assert_eq!(
            report.payload_bytes,
            payload.len(),
            "inactive payload stayed out of the result"
        );
        assert_eq!(report.receipt.claim, claim);
        assert_eq!(report.receipt.revision, 1);
        assert_eq!(
            (report.receipt.boot, report.receipt.offset_seconds, report.receipt.offset_ns),
            (domain.boot(), s, ns)
        );
        assert_eq!(report.receipt.original_tick_ns, capture.nanoseconds());
        assert_eq!(report.receipt.as_of, as_of);
        assert_eq!(report.receipt.hash, hash);
        assert_eq!(report.receipt.payload_hash, <[u8; 32]>::from(Sha256::digest(&payload)));
    }
    assert!(second.receipt.age_ns >= first.receipt.age_ns);
    assert_eq!(proxy.stats().statements, 2);
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated DATABASE_URL"]
async fn n5_refuses_incomparable_expired_and_unresolved_receipts(pool: PgPool) {
    let req = RequestKey::btc();
    let keys = [req.key.clone()];
    seed_registry(&pool, &keys, 1, manifest_digest(&keys), true, "normal").await;
    let clock = NativeClock::initialize().unwrap();
    let original = clock.capture().unwrap();
    let as_of = original_as_of();
    let payload = fixture_payload(as_of);
    let hash = receipt_hash(&req, &payload);
    let claim = seed_success(&pool, &req, original, as_of, &payload, &hash).await;
    let cold = read_local(&pool, &keys).await.unwrap();
    let row = &cold.selected[0];
    assert_eq!(prove(row, &req, original).unwrap().claim, claim);

    let tick = original.nanoseconds();
    let domain = original.domain();
    let one_day = at_tick(domain, tick + DAY.as_nanos()).unwrap();
    assert!(
        matches!(prove(row, &req, one_day), Err(Refusal::Expired)),
        "a restarted process cannot renew a receipt at exactly 24 hours"
    );
    let before = at_tick(domain, tick - 1).unwrap();
    assert!(matches!(prove(row, &req, before), Err(Refusal::Clock(ClockError::Reversed))));
    let (s, ns) = domain.offset();
    for other in [
        ClockDomain::new(Uuid::now_v7(), s, ns).unwrap(),
        ClockDomain::new(domain.boot(), s.checked_add(1).unwrap(), ns).unwrap(),
    ] {
        let now = at_tick(other, tick).unwrap();
        assert!(matches!(prove(row, &req, now), Err(Refusal::Clock(ClockError::Incomparable))));
    }

    advance_unresolved(&pool, &req.key, original).await;
    let cold = read_local(&pool, &keys).await.unwrap();
    let row = &cold.selected[0];
    assert_eq!((row.state_revision, row.snapshot_revision), (Some(2), Some(1)));
    assert_eq!(row.snapshot_claim_id, Some(claim));
    assert_eq!(row.snapshot_payload.as_deref(), Some(payload.as_slice()));
    assert!(
        matches!(prove(row, &req, clock.capture().unwrap()), Err(Refusal::Unresolved)),
        "a fresh previous snapshot cannot stand in for an unresolved current result"
    );
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated DATABASE_URL"]
async fn n5_rejects_submicrosecond_timestamp_loss(pool: PgPool) {
    let req = RequestKey::btc();
    let keys = [req.key.clone()];
    seed_registry(&pool, &keys, 1, manifest_digest(&keys), true, "normal").await;
    let capture = NativeClock::initialize().unwrap().capture().unwrap();
    let source_as_of = original_as_of() + chrono::Duration::nanoseconds(1);
    let payload = fixture_payload(source_as_of);
    seed_success(&pool, &req, capture, source_as_of, &payload, &receipt_hash(&req, &payload)).await;
    let cold = read_local(&pool, &keys).await.unwrap();
    let row = &cold.selected[0];
    assert_eq!(row.snapshot_payload.as_deref(), Some(payload.as_slice()));
    assert_eq!(row.snapshot_as_of, Some(original_as_of()));
    assert!(matches!(prove(row, &req, capture), Err(Refusal::Receipt)));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated DATABASE_URL"]
async fn n5_refuses_missing_registry_state_and_requested_key(pool: PgPool) {
    let keys = [Key::spot("BTCUSDT"), Key::spot("ETHUSDT")];
    assert!(matches!(read_local(&pool, &keys).await, Err(Refusal::Registry)));
    seed_registry(&pool, &keys, 2, manifest_digest(&keys), true, "normal").await;
    seed_empty(&pool, &keys[0]).await;
    assert!(matches!(read_local(&pool, &keys[..1]).await, Err(Refusal::Manifest)));
    seed_empty(&pool, &keys[1]).await;
    assert_eq!(read_local(&pool, &keys).await.unwrap().selected.len(), 2);
    assert!(matches!(
        read_local(&pool, &[Key::spot("BNBUSDT")]).await,
        Err(Refusal::Requested)
    ));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated DATABASE_URL"]
async fn n5_refuses_digest_mismatch(pool: PgPool) {
    let keys = [Key::spot("BTCUSDT")];
    seed_registry(&pool, &keys, 1, manifest_digest(&[]), true, "normal").await;
    seed_empty(&pool, &keys[0]).await;
    assert!(matches!(read_local(&pool, &keys).await, Err(Refusal::Manifest)));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated DATABASE_URL"]
async fn n5_refuses_registered_count_mismatch(pool: PgPool) {
    let keys = [Key::spot("BTCUSDT")];
    seed_registry(&pool, &keys, 2, manifest_digest(&keys), true, "normal").await;
    seed_empty(&pool, &keys[0]).await;
    assert!(matches!(read_local(&pool, &keys).await, Err(Refusal::Manifest)));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated DATABASE_URL"]
async fn n5_refuses_quarantined_registry(pool: PgPool) {
    let keys = [Key::spot("BTCUSDT")];
    seed_registry(&pool, &keys, 1, manifest_digest(&keys), true, "quarantined").await;
    seed_empty(&pool, &keys[0]).await;
    assert!(matches!(read_local(&pool, &keys).await, Err(Refusal::Registry)));
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires isolated DATABASE_URL"]
async fn n5_refuses_native_4097_key_overflow(pool: PgPool) {
    let keys: Vec<_> = (0..=MAX_KEYS).map(|i| Key::spot(format!("FIX{i:04}USDT"))).collect();
    seed_registry(
        &pool,
        &keys,
        MAX_KEYS as i32,
        manifest_digest(&keys[..MAX_KEYS]),
        true,
        "normal",
    )
    .await;
    assert!(matches!(read_local(&pool, &keys[..1]).await, Err(Refusal::Overflow)));
}

#![cfg(all(test, feature = "crash-points"))]

//! Native test composition with a real loopback venue and a disposable SQLx database.
//! The registered prior claims are fixture data, not startup or recovery evidence.

use std::{
    fmt::Write,
    process::Stdio,
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{anyhow, ensure, Context};
use chrono::{DateTime, Utc};
use robson_connectors::{binance::spot::SpotCommission, BinanceRestClient};
use robson_domain::Symbol;
use robson_exec::{
    balance_costs::{native_n5::PreparedFeeOwner, AgeError, CostError},
    venue::VenueId,
    venue_clock::{NativeClock, TickCapture},
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool, Row,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command,
    task::JoinHandle,
    time::{sleep, timeout, timeout_at, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    process_epoch::{self, Epoch},
    venue_budget::{self, bootstrap, restore, state::SharedBudget},
    venue_clock_http::TimeError,
    venue_clock_owner::ClockOwner,
    venue_clock_store::ClockSampleStore,
};

use super::super::{
    cold_load::{self, fixture_manifest, ReceiptState, Registration},
    ClaimIdentity, ClaimResult, FrequencyKey, PriorClaim, PriorEligibility, Trace,
};
use super::{
    content_digest, CommissionOwner, CostIdentity, RefreshOutcome, VerifiedCommissionAccount,
    WIRE_LIMIT,
};

impl VerifiedCommissionAccount {
    /// Verify the fixture UID with the same signed client used by the owner.
    /// Tenant and scope are the explicit trusted local fixture contract.
    async fn verified_loopback(
        identity: CostIdentity,
        client: Arc<BinanceRestClient>,
    ) -> Result<Self, TimeError> {
        if identity.venue().as_str() != "binance:spot" {
            return Err(TimeError::Native);
        }
        let response = timeout_at(
            Instant::now() + WIRE_LIMIT,
            robson_connectors::binance::spot::SpotRestReader::new(Arc::clone(&client)).account(),
        )
        .await
        .map_err(|_| TimeError::Expired)?
        .map_err(|_| TimeError::Transport)?;
        if response.uid != identity.uid() {
            return Err(TimeError::Venue);
        }
        Ok(Self {
            identity,
            client,
            binding: uuid::Uuid::now_v7(),
        })
    }

    fn with_symbol(&self, symbol: robson_domain::Symbol) -> Result<Self, TimeError> {
        let identity = CostIdentity::new(
            self.identity.tenant(),
            self.identity.venue().clone(),
            self.identity.scope().to_owned(),
            self.identity.uid(),
            symbol,
        )
        .map_err(|_| TimeError::Native)?;
        Ok(Self {
            identity,
            client: Arc::clone(&self.client),
            binding: self.binding,
        })
    }
}

const PAIR: &str = "BTCUSDT";
const NO_CLOCK_PAIR: &str = "ETHUSDT";
const REDIRECT_PAIR: &str = "SOLUSDT";
const SCOPE: &str = "loopback-preverified";
const API_KEY: &str = "n5-loopback-fixture-key";
const API_SECRET: &[u8] = b"n5-loopback-fixture-secret";
const UID: u64 = 12345678;
const PRIOR_AGE: Duration = Duration::from_secs(25 * 60 * 60);
const BUDGET_STARTUP: Duration = Duration::from_secs(60);
const COMMISSION: &str = r#"{
  "symbol": "BTCUSDT",
  "standardCommission": {"maker": "0.00100", "taker": "0.002", "buyer": "0", "seller": "0"},
  "specialCommission": {"maker": "0", "taker": "0", "buyer": "0", "seller": "0"},
  "taxCommission": {"maker": "0", "taker": "0", "buyer": "0", "seller": "0"},
  "discount": {"enabledForAccount": false, "enabledForSymbol": false, "discountAsset": "BNB", "discount": "0.25"},
  "unrelatedSymbolMetadata": "omit from retained receipt"
}"#;

#[derive(Debug)]
enum Request {
    Account,
    Time,
    Commission {
        pair: String,
        signed_ms: i64,
        arrived_ms: i64,
    },
}

struct Loopback {
    origin: String,
    arrivals: Arc<Mutex<Vec<Request>>>,
    cancel: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Loopback {
    async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let arrivals = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancellationToken::new();
        let (wire, stopped, server_origin) =
            (Arc::clone(&arrivals), cancel.clone(), origin.clone());
        let task = tokio::spawn(async move {
            loop {
                let (socket, _) = tokio::select! {
                    biased;
                    _ = stopped.cancelled() => return Ok(()),
                    accept = listener.accept() => accept?,
                };
                timeout(Duration::from_secs(3), serve(socket, &wire, &server_origin))
                    .await
                    .context("loopback request deadline")??;
            }
        });
        Ok(Self { origin, arrivals, cancel, task })
    }

    fn count(&self) -> anyhow::Result<usize> {
        Ok(self.arrivals.lock().map_err(|_| anyhow!("loopback arrivals poisoned"))?.len())
    }

    async fn finish(mut self) -> anyhow::Result<Vec<Request>> {
        // Leave the listener armed briefly after the caller's last result,
        // including after the redirect and stale-history controls.
        sleep(Duration::from_millis(25)).await;
        self.cancel.cancel();
        timeout(Duration::from_secs(2), &mut self.task)
            .await
            .context("loopback join deadline")???;
        let observed = std::mem::take(
            &mut *self.arrivals.lock().map_err(|_| anyhow!("loopback arrivals poisoned"))?,
        );
        Ok(observed)
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.cancel.cancel();
        if !self.task.is_finished() {
            self.task.abort();
        }
    }
}

async fn serve(
    mut socket: TcpStream,
    wire: &Mutex<Vec<Request>>,
    origin: &str,
) -> anyhow::Result<()> {
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        ensure!(request.len() < 4096, "bounded loopback request headers");
        request.push(socket.read_u8().await?);
    }
    let request = std::str::from_utf8(&request)?;
    let mut lines = request.lines();
    let start = lines.next().context("loopback request line")?;
    let mut keys = lines.filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("x-mbx-apikey").then_some(value.trim())
    });
    let arrived_ms = Utc::now().timestamp_millis();
    let (record, status, location, body) = if start == "GET /api/v3/time HTTP/1.1" {
        ensure!(keys.next().is_none(), "TIME is unsigned");
        (
            Request::Time,
            "200 OK",
            String::new(),
            format!(r#"{{"serverTime":{arrived_ms}}}"#),
        )
    } else if let Some(query) = start
        .strip_prefix("GET /api/v3/account?")
        .and_then(|line| line.strip_suffix(" HTTP/1.1"))
    {
        ensure!(keys.next() == Some(API_KEY) && keys.next().is_none(), "one fixture key header");
        let (payload, signature) = query.split_once("&signature=").context("account signature")?;
        ensure!(signature == fixture_signature(payload), "account signature of exact parameters");
        ensure!(
            payload
                .strip_prefix("timestamp=")
                .context("account timestamp")?
                .parse::<i64>()?
                > 0
        );
        (
            Request::Account,
            "200 OK",
            String::new(),
            format!(
                r#"{{"uid":{UID},"canTrade":false,"accountType":"SPOT","permissions":["SPOT"],"balances":[]}}"#
            ),
        )
    } else {
        ensure!(keys.next() == Some(API_KEY) && keys.next().is_none(), "one fixture key header");
        let query = start
            .strip_prefix("GET /api/v3/account/commission?")
            .and_then(|line| line.strip_suffix(" HTTP/1.1"))
            .context("unexpected loopback method or route")?;
        let (payload, signature) = query.split_once("&signature=").context("signature field")?;
        ensure!(signature == fixture_signature(payload), "signature of exact wire parameters");
        let (symbol, timestamp) = payload.split_once("&timestamp=").context("timestamp field")?;
        let pair = symbol.strip_prefix("symbol=").context("symbol field")?;
        let signed_ms = timestamp.parse::<i64>().context("millisecond integer timestamp")?;
        ensure!(matches!(pair, PAIR | REDIRECT_PAIR), "unexpected commission symbol");
        let record = Request::Commission {
            pair: pair.to_owned(),
            signed_ms,
            arrived_ms,
        };
        if pair == PAIR {
            (record, "200 OK", String::new(), COMMISSION.to_owned())
        } else {
            (
                record,
                "302 Found",
                format!("Location: {origin}/unexpected-redirect\r\n"),
                String::new(),
            )
        }
    };
    wire.lock().map_err(|_| anyhow!("loopback arrivals poisoned"))?.push(record);
    let response = format!(
        "HTTP/1.1 {status}\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await?;
    Ok(())
}

fn fixture_signature(payload: &str) -> String {
    // Independent HMAC-SHA256 for the fixed short synthetic fixture key.
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for (index, byte) in API_SECRET.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(payload.as_bytes());
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    let mut written = String::new();
    for byte in outer.finalize() {
        write!(&mut written, "{byte:02x}").unwrap();
    }
    written
}

async fn seed_history(
    pool: &PgPool,
    clock: &NativeClock,
    epoch: Epoch,
) -> anyhow::Result<Vec<(FrequencyKey, PriorClaim)>> {
    let now = clock.capture()?;
    let domain = now.domain();
    let (offset_seconds, offset_ns) = domain.offset();
    let prior_ns = now
        .nanoseconds()
        .checked_sub(PRIOR_AGE.as_nanos())
        .context("this fixture needs 25h in its isolated BOOTTIME namespace")?;
    let captured = TickCapture::new(
        domain,
        i64::try_from(prior_ns / 1_000_000_000)?,
        i64::try_from(prior_ns % 1_000_000_000)?,
    )?;
    let audit_at = DateTime::<Utc>::from_timestamp_micros(
        (Utc::now() - chrono::Duration::from_std(PRIOR_AGE)?).timestamp_micros(),
    )
    .context("fixture prior wall instant")?;
    let initialization_id = Uuid::now_v7();
    let registered_at = DateTime::<Utc>::from_timestamp_micros(Utc::now().timestamp_micros())
        .context("fixture registration instant")?;
    let provenance = "loopback-fixture";
    let mut registrations = [NO_CLOCK_PAIR, PAIR, REDIRECT_PAIR].map(|pair| Registration {
        venue_id: VenueId::BINANCE_SPOT.to_owned(),
        symbol: pair.to_owned(),
        descriptor: "commission".to_owned(),
        key_registry: 1,
        registration_id: Uuid::now_v7(),
        registered_revision: 1,
        registered_at,
        provenance: provenance.to_owned(),
    });
    let digest = fixture_manifest(&mut registrations);
    sqlx::query(
        "INSERT INTO n5_registry_root VALUES
           (1, 38, 1, 1, true, 3, $1, $2, 0, 'normal', NULL, CURRENT_TIMESTAMP)",
    )
    .bind(digest.as_slice())
    .bind(initialization_id)
    .execute(pool)
    .await?;

    let mut histories = Vec::new();
    for pair in [NO_CLOCK_PAIR, PAIR, REDIRECT_PAIR] {
        let registration = registrations
            .iter()
            .find(|entry| entry.symbol == pair)
            .context("fixture registration")?;
        let key = FrequencyKey {
            symbol: pair.to_owned(),
            initialization_id,
            recovery_generation: 0,
            registration_id: registration.registration_id,
        };
        let prior = PriorClaim {
            revision: 2,
            identity: ClaimIdentity {
                claim_id: Uuid::now_v7(),
                owner: epoch.epoch_id,
                digest: [0x71; 32],
                captured,
                audit_at,
            },
            eligibility: PriorEligibility::SameDomain,
            handoff: None,
            result: Some(ClaimResult::NoSend),
        };
        sqlx::query(
            "INSERT INTO n5_refresh_keys VALUES
               ('binance:spot', $1, 'commission', 1, $2, 1, $3, $4)",
        )
        .bind(&key.symbol)
        .bind(key.registration_id)
        .bind(registered_at)
        .bind(provenance)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO n5_refresh_state (
                 venue_id, symbol, descriptor, revision, claim_id, owner_process, content_digest,
                 claim_domain, claim_tick, claim_audit_at, eligibility_kind, result_class
             ) VALUES (
                 'binance:spot', $1, 'commission', 2, $2, $3, $4,
                 ROW('boottime', $5::uuid, $6::bigint, $7::integer)::budget_clock_domain_v38,
                 $8::text::numeric, $9, 'same_domain', 'no_send'
             )",
        )
        .bind(&key.symbol)
        .bind(prior.identity.claim_id)
        .bind(prior.identity.owner)
        .bind(prior.identity.digest.as_slice())
        .bind(domain.boot())
        .bind(offset_seconds)
        .bind(i32::try_from(offset_ns)?)
        .bind(captured.nanoseconds().to_string())
        .bind(audit_at)
        .execute(pool)
        .await?;
        histories.push((key, prior));
    }
    Ok(histories)
}

async fn count_store_wire(
    pool: &PgPool,
    base_url: &str,
    owner: &mut CommissionOwner<'_>,
    key: FrequencyKey,
    prior: PriorClaim,
    outcome: RefreshOutcome,
    stores: usize,
) -> anyhow::Result<()> {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .context("native PostgreSQL fixture upstream")?;
    ensure!(upstream.ip().is_loopback(), "native fixture uses loopback");
    let proxy = DbProxy::start(upstream).await?;
    proxy.enable_query_trace(32);
    proxy.enable_diagnostics();
    let url = database_url_through(
        base_url,
        proxy.addr(),
        options.get_database().context("macro database name")?,
    )?;
    let measured_pool = PgPoolOptions::new().max_connections(1).connect_lazy(&url)?;
    let trace = Trace::default();
    let result = owner
        .refresh_registered(
            &measured_pool,
            key,
            prior,
            Instant::now() + Duration::from_secs(15),
            &trace,
        )
        .await
        .map_err(|error| anyhow!("fixture N5 refresh failed: {error:?}"))?;
    ensure!(result == outcome, "unexpected native N5 outcome: {result:?}");
    timeout(Duration::from_secs(3), async {
        let connections = if stores == 0 { 1 } else { stores };
        loop {
            let pids: Vec<_> = proxy
                .diagnostic_identities()
                .context("native backend identities")?
                .iter()
                .map(|identity| identity.pid.context("native backend PID"))
                .collect::<anyhow::Result<_>>()?;
            if pids.len() == connections
                && proxy.closed_relays() == u64::try_from(connections)?
                && sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM pg_stat_activity
                     WHERE datname = current_database() AND pid = ANY($1)",
                )
                .bind(pids)
                .fetch_one(pool)
                .await?
                    == 0
            {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("N5 native backends must retire")??;

    let commands = proxy.query_trace();
    let commits = u64::try_from(stores)?;
    let expected = if stores == 0 { 3 } else { 4 * commits };
    ensure!(proxy.stats().commits == commits, "confirmed SQL commits");
    ensure!(
        proxy.stats().statements == expected && trace.issued() == expected,
        "N5 SQL count"
    );
    ensure!(proxy.query_trace_dropped() == 0 && commands.len() == usize::try_from(expected)?);
    let text: Vec<_> = commands.iter().map(|sql| sql.trim().to_ascii_uppercase()).collect();
    for store in text.chunks(4) {
        ensure!(store[0].starts_with("BEGIN"));
        ensure!(store[1].contains("LOCK_TIMEOUT"));
        ensure!(store[2].contains("UPDATE N5_REFRESH_STATE"));
        if stores > 0 {
            ensure!(store[3] == "COMMIT");
        }
    }
    ensure!(owner.preparation.is_none(), "only owned preparation work");
    Ok(())
}

fn account(tenant: Uuid, pair: &str) -> anyhow::Result<CostIdentity> {
    Ok(CostIdentity::new(
        tenant,
        VenueId::new(VenueId::BINANCE_SPOT),
        SCOPE.to_owned(),
        UID,
        Symbol::from_pair(pair)?,
    )?)
}

async fn no_snapshot(
    pool: &PgPool,
    pair: &str,
    disposition: &str,
    handed_off: bool,
) -> anyhow::Result<()> {
    let observed: bool = sqlx::query_scalar(
        "SELECT revision=3 AND result_class=$2 AND (handoff_tick IS NOT NULL)=$3
            AND snapshot_tick IS NULL AND snapshot_payload IS NULL
         FROM n5_refresh_state WHERE venue_id='binance:spot' AND symbol=$1",
    )
    .bind(pair)
    .bind(disposition)
    .bind(handed_off)
    .fetch_one(pool)
    .await?;
    ensure!(observed, "expected exact terminal state without fee snapshot");
    Ok(())
}

async fn verify_persisted_success(
    pool: &PgPool,
    expected: &CostIdentity,
    epoch: Epoch,
    native: &NativeClock,
    signer_binding: Uuid,
) -> anyhow::Result<ReadReport> {
    let row = sqlx::query(
        "SELECT revision, claim_id, owner_process, content_digest, claim_audit_at,
                result_class, snapshot_claim_id, snapshot_revision, snapshot_owner,
                snapshot_tenant, snapshot_scope, snapshot_uid::text AS uid,
                claim_tick::text AS c, handoff_tick::text AS h, snapshot_tick::text AS r,
                (snapshot_domain).boot_uuid AS boot, (snapshot_domain).offset_seconds AS os,
                (snapshot_domain).offset_nanoseconds AS ons, snapshot_as_of,
                snapshot_validation_version, snapshot_hash, snapshot_payload
         FROM n5_refresh_state WHERE venue_id='binance:spot' AND symbol=$1",
    )
    .bind(PAIR)
    .fetch_one(pool)
    .await?;
    ensure!(row.try_get::<i64, _>("revision")? == 3);
    ensure!(row.try_get::<String, _>("result_class")? == "success");
    ensure!(row.try_get::<Uuid, _>("owner_process")? == epoch.epoch_id);
    ensure!(row.try_get::<Uuid, _>("snapshot_owner")? == epoch.epoch_id);
    ensure!(row.try_get::<Uuid, _>("claim_id")? == row.try_get::<Uuid, _>("snapshot_claim_id")?);
    ensure!(row.try_get::<i64, _>("snapshot_revision")? == 3);
    ensure!(row.try_get::<Uuid, _>("snapshot_tenant")? == expected.tenant());
    ensure!(row.try_get::<String, _>("snapshot_scope")? == expected.scope());
    ensure!(row.try_get::<String, _>("uid")? == expected.uid().to_string());
    ensure!(row.try_get::<i16, _>("snapshot_validation_version")? == 1);
    ensure!(
        row.try_get::<Vec<u8>, _>("content_digest")?
            == content_digest(expected, signer_binding, PAIR),
        "durable claim binds this fixture's account and signer"
    );
    let (c, h, r) = (
        row.try_get::<String, _>("c")?.parse::<u128>()?,
        row.try_get::<String, _>("h")?.parse::<u128>()?,
        row.try_get::<String, _>("r")?.parse::<u128>()?,
    );
    let now = native.capture()?;
    ensure!(c <= h && h - c <= Duration::from_secs(60).as_nanos());
    ensure!(h <= r && r <= now.nanoseconds());
    let domain = now.domain();
    let (offset_seconds, offset_ns) = domain.offset();
    ensure!(row.try_get::<Uuid, _>("boot")? == domain.boot());
    ensure!(row.try_get::<i64, _>("os")? == offset_seconds);
    ensure!(row.try_get::<i32, _>("ons")? == i32::try_from(offset_ns)?);

    let payload: Vec<u8> = row.try_get("snapshot_payload")?;
    let retained: serde_json::Value = serde_json::from_slice(&payload)?;
    let original = &retained["original"];
    ensure!(retained["format_version"] == 1);
    ensure!(
        original["epoch"].as_str().context("EOF mark epoch")?.parse::<Uuid>()? == epoch.epoch_id
    );
    ensure!(original["boot"].as_str().context("EOF boot ID")?.parse::<Uuid>()? == domain.boot());
    ensure!(original["offset_seconds"].as_i64() == Some(offset_seconds));
    ensure!(original["offset_nanoseconds"].as_u64() == Some(u64::from(offset_ns)));
    ensure!(original["before_ns"].as_str().context("EOF first tick")?.parse::<u128>()? == r);
    let after = original["after_ns"].as_str().context("EOF last tick")?.parse::<u128>()?;
    ensure!(r <= after && after <= now.nanoseconds());
    let wall_seconds = original["wall_seconds"].as_i64().context("original wall seconds")?;
    let wall_ns = original["wall_nanoseconds"].as_u64().context("original wall fraction")?;
    let expected_as_of =
        DateTime::<Utc>::from_timestamp(wall_seconds, u32::try_from(wall_ns / 1_000 * 1_000)?)
            .context("canonical original response microseconds")?;
    let stored_as_of: DateTime<Utc> = row.try_get("snapshot_as_of")?;
    ensure!(stored_as_of == expected_as_of);
    let mut original_fees: serde_json::Value = serde_json::from_str(COMMISSION)?;
    original_fees
        .as_object_mut()
        .context("commission object")?
        .remove("unrelatedSymbolMetadata");
    ensure!(
        retained["commission"] == original_fees,
        "retains only original fee fields and strings"
    );
    let exact: SpotCommission = serde_json::from_str(COMMISSION)?;
    let evidence = exact.cost_candidate(expected.clone(), stored_as_of)?;
    ensure!(row.try_get::<Vec<u8>, _>("snapshot_hash")? == evidence.evidence().hash().as_slice());
    let audit: DateTime<Utc> = row.try_get("claim_audit_at")?;
    Ok(ReadReport {
        pid: std::process::id(),
        age_ns: now.nanoseconds() - r,
        warm_checks: 0,
        receipt: ObservedReceipt {
            claim: row.try_get("claim_id")?,
            revision: 3,
            owner: epoch.epoch_id,
            claim_digest: content_digest(expected, signer_binding, PAIR),
            audit_at: audit,
            claim_tick: c,
            handoff_tick: h,
            boot: domain.boot(),
            offset_seconds,
            offset_ns,
            original_before_ns: r,
            original_after_ns: after,
            wall_seconds,
            wall_ns: u32::try_from(wall_ns)?,
            as_of: stored_as_of,
            evidence_hash: *evidence.evidence().hash(),
            payload_digest: Sha256::digest(payload).into(),
        },
    })
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ObservedReceipt {
    claim: Uuid,
    revision: i64,
    owner: Uuid,
    claim_digest: [u8; 32],
    audit_at: DateTime<Utc>,
    claim_tick: u128,
    handoff_tick: u128,
    boot: Uuid,
    offset_seconds: i64,
    offset_ns: u32,
    original_before_ns: u128,
    original_after_ns: u128,
    wall_seconds: i64,
    wall_ns: u32,
    as_of: DateTime<Utc>,
    evidence_hash: [u8; 32],
    payload_digest: [u8; 32],
}

#[derive(Debug, Serialize, Deserialize)]
struct ReadReport {
    pid: u32,
    age_ns: u128,
    warm_checks: u8,
    receipt: ObservedReceipt,
}

#[derive(Debug, Serialize, Deserialize)]
struct WriteReport {
    tenant: Uuid,
    committed: ReadReport,
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires disposable loopback PostgreSQL and isolated native BOOTTIME >=25h"]
async fn native_commission_first_poll_and_exact_result(pool: PgPool) -> anyhow::Result<()> {
    let options = pool.connect_options();
    let database = options.get_database().context("macro database name")?;
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .context("fixture PostgreSQL endpoint")?;
    ensure!(upstream.ip().is_loopback(), "local PostgreSQL fixture");
    let base_url = std::env::var("DATABASE_URL")?;
    let writer_url = database_url_through(&base_url, upstream, database)?;
    let (writer_pid, writer): (_, WriteReport) =
        run_child("writer", WRITER_MARK, &writer_url, None, Duration::from_secs(120)).await?;
    ensure!(writer_pid == writer.committed.pid);
    wait_child_sessions(&pool, "robson-n5-writer").await?;
    eprintln!("N5_NATIVE_LOOPBACK time=1 success=1 redirect=1 no_c8=0 stale_claim=0 sql=8,8,3,8");

    let proxy = DbProxy::start(upstream).await?;
    proxy.enable_exact_query_trace(4);
    proxy.enable_diagnostics();
    let reader_url = database_url_through(&base_url, proxy.addr(), database)?;
    let (reader_pid, reader): (_, ReadReport) =
        run_child("reader", READER_MARK, &reader_url, Some(writer.tenant), Duration::from_secs(10))
            .await?;
    ensure!(reader_pid == reader.pid && reader_pid != writer_pid);
    ensure!(reader.receipt == writer.committed.receipt, "original committed receipt");
    ensure!(reader.age_ns > writer.committed.age_ns, "original response aged across restart");
    ensure!(writer.committed.warm_checks == 0 && reader.warm_checks == 4);
    wait_child_sessions(&pool, "robson-n5-reader").await?;
    let stats = proxy.stats();
    ensure!(stats.connections == 1 && stats.statements == 1 && stats.commits == 0);
    ensure!(proxy.query_trace_dropped() == 0 && proxy.query_trace().len() == 1);
    ensure!(proxy.query_trace()[0].contains("LIMIT 4097"));
    let result_bytes = proxy.data_row_bytes();
    let response_bytes = proxy.query_response_bytes();
    ensure!(proxy.data_rows() == 3 && result_bytes > 0 && result_bytes < response_bytes);
    ensure!(response_bytes <= 8 * 1024 * 1024);
    eprintln!("N5_NATIVE_RESTART writer_pid={writer_pid} writer_exit=0 reader_pid={reader_pid} reader_exit=0 sql=1 rows=3 data_row_wire_bytes={result_bytes} total_response_bytes={response_bytes} reader_http=0 age_advanced=true");
    eprintln!("N5_WARM_SELECTION exact_terms=1 refused_wrong_uid=1 retired_view_refused=1 terms_unchanged=1 sql=0 http=0");
    cold_load::verify_real_receipt_controls(&pool, &account(writer.tenant, PAIR)?).await?;
    Ok(())
}

async fn signed_writer(pool: PgPool, base_url: &str) -> anyhow::Result<WriteReport> {
    let clock = Arc::new(NativeClock::initialize()?);
    let tenant = Uuid::now_v7();
    let init = venue_budget::fixture::prepare(
        &pool,
        &clock,
        Instant::now() + Duration::from_secs(2),
        &Trace::default(),
    )
    .await
    .map_err(|error| anyhow!("fresh fixture preparation: {error:?}"))?;

    // The isolated fixture supplies a marked predecessor so this test can
    // measure a clean startup. The UnknownPriorHistory 60s gate remains real.
    let predecessor: (Uuid, i64) = sqlx::query_as(
        "INSERT INTO process_epochs (epoch_id, tenant_id, binary_version)
         VALUES ($1, $2, 'N5-loopback-predecessor') RETURNING epoch_id, epoch_seq",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .fetch_one(&pool)
    .await?;
    process_epoch::mark_clean_stop(
        &pool,
        Epoch {
            epoch_id: predecessor.0,
            epoch_seq: predecessor.1,
        },
    )
    .await
    .map_err(|_| anyhow!("fixture predecessor stop marker"))?;
    let start = process_epoch::start_epoch(&pool, tenant, "N5-loopback").await?;
    ensure!(start.predecessor.unclean_reason().is_none() && start.open_incidents.is_empty());
    let mut rows = bootstrap::observe_bootstrap(
        &pool,
        tenant,
        &start,
        &init,
        Instant::now() + Duration::from_secs(2),
        &Trace::default(),
    )
    .await
    .map_err(|error| anyhow!("fixture budget bootstrap: {error:?}"))?;
    restore::existing_pause(
        &pool,
        tenant,
        &mut rows,
        &clock,
        Instant::now() + Duration::from_secs(2),
        &Trace::default(),
    )
    .await
    .map_err(|error| anyhow!("fixture budget pause: {error:?}"))?;
    let budget = SharedBudget::from_rows(rows, start.predecessor.unclean_reason().is_some())
        .map_err(|error| anyhow!("fixture shared budget: {error:?}"))?;
    let budget_start =
        budget.start().map_err(|error| anyhow!("fixture budget start: {error:?}"))?;

    let histories = seed_history(&pool, &clock, start.epoch).await?;
    let [(no_clock, no_clock_prior), (success, success_prior), (redirect, redirect_prior)] =
        <[_; 3]>::try_from(histories).map_err(|_| anyhow!("three fixture frequency keys"))?;
    let elapsed = clock.capture()?.elapsed_since(&budget_start)?;
    sleep(BUDGET_STARTUP.saturating_sub(elapsed) + Duration::from_millis(5)).await;
    ensure!(clock.capture()?.elapsed_since(&budget_start)? >= BUDGET_STARTUP);

    let wire = Loopback::start().await?;
    let client = Arc::new(BinanceRestClient::with_harness_base_url(
        wire.origin.clone(),
        API_KEY.to_owned(),
        std::str::from_utf8(API_SECRET)?.to_owned(),
    ));
    let expected = account(tenant, PAIR)?;
    let verified =
        VerifiedCommissionAccount::verified_loopback(expected.clone(), Arc::clone(&client))
            .await
            .map_err(|error| anyhow!("verified loopback account: {error:?}"))?;
    let signer_binding = verified.binding;
    let mut owner = ClockOwner::controlled(
        Arc::clone(&clock),
        Arc::clone(&client),
        ClockSampleStore::from_committed_epoch(&pool, tenant, &start),
        budget.clone(),
        tenant,
        start.epoch.epoch_id,
    )
    .map_err(|error| anyhow!("fixture clock owner: {error:?}"))?;

    let other_signer = Arc::new(BinanceRestClient::with_harness_base_url(
        wire.origin.clone(),
        API_KEY.to_owned(),
        std::str::from_utf8(API_SECRET)?.to_owned(),
    ));
    let mut denied = None;
    ensure!(owner
        .prepare_commission(
            &expected,
            &other_signer,
            &mut denied,
            Instant::now() + Duration::from_secs(1)
        )
        .await
        .is_err());
    ensure!(
        denied.is_none() && wire.count()? == 1,
        "different signer never prepares or sends"
    );
    println!("N5 fixture account verification uses one separate signed account request");

    count_store_wire(
        &pool,
        base_url,
        &mut CommissionOwner {
            clock: &owner,
            account: verified
                .with_symbol(account(tenant, NO_CLOCK_PAIR)?.symbol().clone())
                .map_err(|error| anyhow!("same verified account: {error:?}"))?,
            preparation: None,
        },
        no_clock,
        no_clock_prior,
        RefreshOutcome::Published,
        2,
    )
    .await?;
    no_snapshot(&pool, NO_CLOCK_PAIR, "no_send", false).await?;
    ensure!(budget.handoffs() == Ok(0) && wire.count()? == 1, "C8 not published yet");

    let sample = owner
        .run_cycle()
        .await
        .map_err(|error| anyhow!("native TIME cycle: {error:?}"))?;
    let stored_sample: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM clock_samples
         WHERE sample_id=$1 AND process_epoch=$2 AND tenant_id=$3 AND numeric_failures=0)",
    )
    .bind(sample)
    .bind(start.epoch.epoch_id)
    .bind(tenant)
    .fetch_one(&pool)
    .await?;
    ensure!(stored_sample && budget.handoffs() == Ok(1) && wire.count()? == 2);

    let mut commission = CommissionOwner {
        clock: &owner,
        account: verified
            .with_symbol(expected.symbol().clone())
            .map_err(|error| anyhow!("same verified account: {error:?}"))?,
        preparation: None,
    };
    count_store_wire(
        &pool,
        base_url,
        &mut commission,
        success.clone(),
        success_prior.clone(),
        RefreshOutcome::Published,
        2,
    )
    .await?;
    let committed =
        verify_persisted_success(&pool, &expected, start.epoch, &clock, signer_binding).await?;
    ensure!(budget.handoffs() == Ok(2) && wire.count()? == 3, "one signed commission");
    count_store_wire(
        &pool,
        base_url,
        &mut commission,
        success,
        success_prior,
        RefreshOutcome::ClaimRefused,
        0,
    )
    .await?;
    ensure!(
        budget.handoffs() == Ok(2) && wire.count()? == 3,
        "stale claim cannot send again"
    );

    count_store_wire(
        &pool,
        base_url,
        &mut CommissionOwner {
            clock: &owner,
            account: verified
                .with_symbol(account(tenant, REDIRECT_PAIR)?.symbol().clone())
                .map_err(|error| anyhow!("same verified account: {error:?}"))?,
            preparation: None,
        },
        redirect,
        redirect_prior,
        RefreshOutcome::Published,
        2,
    )
    .await?;
    no_snapshot(&pool, REDIRECT_PAIR, "unknown", true).await?;
    ensure!(budget.handoffs() == Ok(3));
    let observed = wire.finish().await?;
    let [Request::Account, Request::Time, Request::Commission { pair, signed_ms, arrived_ms }, Request::Commission { pair: declined, .. }] =
        observed.as_slice()
    else {
        return Err(anyhow!("expected one account verification, one TIME and two commission connections without redirect"));
    };
    ensure!(pair == PAIR && declined == REDIRECT_PAIR);
    ensure!(
        committed.receipt.audit_at.timestamp_millis() <= *signed_ms && *signed_ms <= *arrived_ms,
        "signed timestamp is original bounded millisecond wall time"
    );
    pool.close().await;
    Ok(WriteReport { tenant, committed })
}

const WRITER_MARK: &str = "N5_RESTART_WRITER=";
const READER_MARK: &str = "N5_RESTART_READER=";
const ROLE_VAR: &str = "ROBSON_TEST_N5_RESTART_ROLE";
const URL_VAR: &str = "ROBSON_TEST_N5_RESTART_URL";
const TENANT_VAR: &str = "ROBSON_TEST_N5_RESTART_TENANT";

async fn run_child<T: DeserializeOwned>(
    role: &str,
    marker: &str,
    url: &str,
    tenant: Option<Uuid>,
    limit: Duration,
) -> anyhow::Result<(u32, T)> {
    let (_, module) = module_path!().split_once("::").context("libtest module")?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            &format!("{module}::n5_restart_participant"),
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("RUST_MIN_STACK", "1048576")
        .env(ROLE_VAR, role)
        .env(URL_VAR, url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(tenant) = tenant {
        command.env(TENANT_VAR, tenant.to_string());
    }
    let child = command.spawn()?;
    let pid = child.id().context("child pid")?;
    let output = timeout(limit, child.wait_with_output()).await.context("bounded N5 child")??;
    ensure!(output.status.success(), "N5 {role} child exit {}", output.status);
    let stdout = String::from_utf8(output.stdout)?;
    let mut reports = stdout.lines().filter_map(|line| line.strip_prefix(marker));
    let (Some(report), None) = (reports.next(), reports.next()) else {
        return Err(anyhow!("expected exactly one N5 {role} report"));
    };
    ensure!(pid != std::process::id());
    Ok((pid, serde_json::from_str(report)?))
}

async fn wait_child_sessions(pool: &PgPool, application: &str) -> anyhow::Result<()> {
    timeout(Duration::from_secs(3), async {
        loop {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname = current_database() AND application_name = $1",
            )
            .bind(application)
            .fetch_one(pool)
            .await?;
            if count == 0 {
                return Ok(());
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("native child PostgreSQL sessions retired")?
}

#[test]
fn n5_restart_participant() -> anyhow::Result<()> {
    let Ok(role) = std::env::var(ROLE_VAR) else {
        return Ok(());
    };
    ensure!(matches!(role.as_str(), "writer" | "reader"));
    let url = std::env::var(URL_VAR)?;
    let options = PgConnectOptions::from_str(&url)
        .map_err(|_| anyhow!("fixture PostgreSQL connection options"))?
        .application_name(&format!("robson-n5-{role}"));
    // The parent creates the migrated SQLx database. Children can only connect.
    ensure!(options.get_host().parse::<std::net::IpAddr>()?.is_loopback());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let pool = PgPoolOptions::new().connect_lazy_with(options);
            if role == "writer" {
                let report = signed_writer(pool, &url).await?;
                println!("\n{WRITER_MARK}{}", serde_json::to_string(&report)?);
            } else {
                let tenant = std::env::var(TENANT_VAR)?.parse::<Uuid>()?;
                let report = read_committed(&pool, tenant).await?;
                pool.close().await;
                println!("\n{READER_MARK}{}", serde_json::to_string(&report)?);
            }
            Ok(())
        })
}

async fn read_committed(pool: &PgPool, tenant: Uuid) -> anyhow::Result<ReadReport> {
    let clock = Arc::new(NativeClock::initialize()?);
    let identity = account(tenant, PAIR)?;
    let trace = Trace::default();
    let entries = cold_load::load(
        pool,
        std::slice::from_ref(&identity),
        &clock,
        Instant::now() + Duration::from_secs(2),
        &trace,
    )
    .await
    .map_err(|error| anyhow!("bounded N5 read: {error:?}"))?;
    let [entry] = entries.entries() else {
        return Err(anyhow!("expected one configured key"));
    };
    ensure!(trace.issued() == 1 && entry.key.symbol == PAIR && entry.key.recovery_generation == 0);
    let prior = entry.prior.as_ref().context("committed prior identity")?;
    let ReceiptState::Current(receipt) = &entry.receipt else {
        return Err(anyhow!("committed current receipt"));
    };
    let mark = receipt.original;
    let (offset_seconds, offset_ns) = mark.domain.offset();
    let mut report = ReadReport {
        pid: std::process::id(),
        age_ns: receipt.age.as_nanos(),
        warm_checks: 0,
        receipt: ObservedReceipt {
            claim: prior.identity.claim_id,
            revision: prior.revision,
            owner: prior.identity.owner,
            claim_digest: prior.identity.digest,
            audit_at: prior.identity.audit_at,
            claim_tick: prior.identity.captured.nanoseconds(),
            handoff_tick: prior.handoff.context("committed handoff")?.nanoseconds(),
            boot: mark.domain.boot(),
            offset_seconds,
            offset_ns,
            original_before_ns: u128::try_from(mark.before_ns)?,
            original_after_ns: u128::try_from(mark.after_ns)?,
            wall_seconds: mark.wall_seconds,
            wall_ns: mark.wall_nanoseconds,
            as_of: receipt.evidence().as_of(),
            evidence_hash: *receipt.evidence().hash(),
            payload_digest: receipt.payload_digest,
        },
    };
    let owner = PreparedFeeOwner::prepare(entries, clock)?;
    let view = owner.selection();
    let gap = "1.25".parse()?;
    let terms = view.select(&identity, gap)?;
    ensure!(
        terms.evidence().hash() == &report.receipt.evidence_hash
            && terms.evidence().as_of() == report.receipt.as_of
    );
    let wrong_uid = CostIdentity::new(
        identity.tenant(),
        identity.venue().clone(),
        identity.scope().to_owned(),
        identity.uid() ^ 1,
        identity.symbol().clone(),
    )?;
    ensure!(matches!(view.select(&wrong_uid, gap), Err(CostError::Age(AgeError::Identity))));
    let held_terms_hash = *terms.hash();
    drop(owner);
    ensure!(matches!(view.select(&identity, gap), Err(CostError::Age(AgeError::Unresolved))));
    ensure!(*terms.hash() == held_terms_hash && trace.issued() == 1);
    report.warm_checks = 4;
    Ok(report)
}

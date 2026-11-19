//! Native test composition with a real loopback venue and a disposable SQLx database.
//! The registered prior claims are fixture data, not startup or recovery evidence.

use std::{
    fmt::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{anyhow, ensure, Context};
use chrono::{DateTime, Utc};
use robson_connectors::{binance::spot::SpotCommission, BinanceRestClient};
use robson_domain::Symbol;
use robson_exec::{
    venue::VenueId,
    venue_clock::{NativeClock, TickCapture},
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::{sleep, timeout, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    process_epoch::{self, Epoch},
    venue_budget::{self, bootstrap, restore, state::SharedBudget},
    venue_clock_owner::ClockOwner,
    venue_clock_store::ClockSampleStore,
};

use super::super::{ClaimIdentity, ClaimResult, FrequencyKey, PriorClaim, PriorEligibility, Trace};
use super::{content_digest, CommissionOwner, CostIdentity, RefreshOutcome};

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
    sqlx::query(
        "INSERT INTO n5_registry_root VALUES
           (1, 38, 1, 1, true, 3, $1, $2, 0, 'normal', NULL, CURRENT_TIMESTAMP)",
    )
    .bind([0x35_u8; 32].as_slice())
    .bind(initialization_id)
    .execute(pool)
    .await?;

    let mut histories = Vec::new();
    for pair in [NO_CLOCK_PAIR, PAIR, REDIRECT_PAIR] {
        let key = FrequencyKey {
            symbol: pair.to_owned(),
            initialization_id,
            recovery_generation: 0,
            registration_id: Uuid::now_v7(),
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
               ('binance:spot', $1, 'commission', 1, $2, 1, CURRENT_TIMESTAMP, 'loopback-fixture')",
        )
        .bind(&key.symbol)
        .bind(key.registration_id)
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
        &std::env::var("DATABASE_URL")?,
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
) -> anyhow::Result<i64> {
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
    Ok(audit.timestamp_millis())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires disposable loopback PostgreSQL and isolated native BOOTTIME >=25h"]
async fn native_commission_first_poll_and_exact_result(pool: PgPool) -> anyhow::Result<()> {
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
    let mut owner = ClockOwner::controlled(
        Arc::clone(&clock),
        client,
        ClockSampleStore::from_committed_epoch(&pool, tenant, &start),
        budget.clone(),
        tenant,
        start.epoch.epoch_id,
    )
    .map_err(|error| anyhow!("fixture clock owner: {error:?}"))?;
    let signer_binding = Uuid::now_v7();

    count_store_wire(
        &pool,
        &mut CommissionOwner {
            clock: &owner,
            account: account(tenant, NO_CLOCK_PAIR)?,
            signer_binding,
            preparation: None,
        },
        no_clock,
        no_clock_prior,
        RefreshOutcome::Published,
        2,
    )
    .await?;
    no_snapshot(&pool, NO_CLOCK_PAIR, "no_send", false).await?;
    ensure!(budget.handoffs() == Ok(0) && wire.count()? == 0, "C8 not published yet");

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
    ensure!(stored_sample && budget.handoffs() == Ok(1) && wire.count()? == 1);

    let expected = account(tenant, PAIR)?;
    let mut commission = CommissionOwner {
        clock: &owner,
        account: expected.clone(),
        signer_binding,
        preparation: None,
    };
    count_store_wire(
        &pool,
        &mut commission,
        success.clone(),
        success_prior.clone(),
        RefreshOutcome::Published,
        2,
    )
    .await?;
    let claim_wall_ms =
        verify_persisted_success(&pool, &expected, start.epoch, &clock, signer_binding).await?;
    ensure!(budget.handoffs() == Ok(2) && wire.count()? == 2, "one signed commission");
    count_store_wire(
        &pool,
        &mut commission,
        success,
        success_prior,
        RefreshOutcome::ClaimRefused,
        0,
    )
    .await?;
    ensure!(
        budget.handoffs() == Ok(2) && wire.count()? == 2,
        "stale claim cannot send again"
    );

    count_store_wire(
        &pool,
        &mut CommissionOwner {
            clock: &owner,
            account: account(tenant, REDIRECT_PAIR)?,
            signer_binding,
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
    let [Request::Time, Request::Commission { pair, signed_ms, arrived_ms }, Request::Commission { pair: declined, .. }] =
        observed.as_slice()
    else {
        return Err(anyhow!("expected one TIME and two commission connections without redirect"));
    };
    ensure!(pair == PAIR && declined == REDIRECT_PAIR);
    ensure!(
        claim_wall_ms <= *signed_ms && *signed_ms <= *arrived_ms,
        "signed timestamp is original bounded millisecond wall time"
    );
    eprintln!("N5_NATIVE_LOOPBACK time=1 success=1 redirect=1 no_c8=0 stale_claim=0 sql=8,8,3,8");
    Ok(())
}

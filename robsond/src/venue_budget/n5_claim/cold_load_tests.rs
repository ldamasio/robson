use anyhow::{anyhow, ensure, Context};
use robson_domain::Symbol;
use robson_testkit::{
    daemon_process::database_url_through,
    db_proxy::{DbProxy, ProxyMode},
};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tokio::time::{sleep, timeout};

use super::*;
use robson_store::native_n5::{
    MAX_ACTIVE_METADATA_WIRE_BYTES, MAX_BYTES, MAX_COMPACT_WIRE_BYTES, MAX_KEYS,
    MAX_MESSAGE_METADATA_WIRE_BYTES, MAX_ROWS,
};

const SQL: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../robson-store/src/n5_cold_load.sql"));

fn registration(index: usize) -> Registration {
    Registration {
        venue_id: "binance:spot".to_owned(),
        symbol: format!("FIX{index:05}USDT"),
        descriptor: "commission".to_owned(),
        key_registry: 1,
        registration_id: Uuid::from_u128(index as u128 + 121),
        registered_revision: 1,
        registered_at: DateTime::<Utc>::from_timestamp(1_795_120_000, 123_456_000).unwrap(),
        provenance: "fixture".to_owned(),
    }
}

#[test]
fn complete_registration_digest_binds_provenance_and_registration_identity() {
    let mut fixture = registration(0);
    fixture.symbol = "ETHUSDT".to_owned();
    fixture.registration_id = Uuid::from_u128(2);
    fixture.registered_revision = 3;
    let a = fixture_manifest(std::slice::from_mut(&mut fixture));
    assert_eq!(
        format!("{:x}", sha2::digest::Output::<Sha256>::from(a)),
        "9693cda5670945470763818b5136cb9dd75a07e4ea1bff9dc946c880d8efd222",
    );
    fixture.registration_id = Uuid::from_u128(3);
    assert_ne!(fixture_manifest(std::slice::from_mut(&mut fixture)), a);
    fixture.registration_id = Uuid::from_u128(2);
    fixture.registered_revision -= 1;
    assert_ne!(fixture_manifest(std::slice::from_mut(&mut fixture)), a);
    fixture.registered_revision += 1;
    fixture.provenance.push('2');
    assert_ne!(fixture_manifest(std::slice::from_mut(&mut fixture)), a);
}

async fn root(
    pool: &PgPool,
    entries: &mut [Registration],
    digest_matches: bool,
) -> anyhow::Result<()> {
    let digest = if digest_matches {
        fixture_manifest(entries)
    } else {
        [4; 32]
    };
    sqlx::query(
        "INSERT INTO n5_registry_root VALUES
                 (1,38,1,1,true,$1,$2,$3,0,'normal',NULL,CURRENT_TIMESTAMP)",
    )
    .bind(i32::try_from(entries.len())?)
    .bind(digest.as_slice())
    .bind(Uuid::from_u128(127))
    .execute(pool)
    .await?;
    keys(pool, entries).await
}

async fn keys(pool: &PgPool, entries: &[Registration]) -> anyhow::Result<()> {
    let symbols: Vec<_> = entries.iter().map(|entry| entry.symbol.as_str()).collect();
    let ids: Vec<_> = entries.iter().map(|entry| entry.registration_id).collect();
    sqlx::query(
        "INSERT INTO n5_refresh_keys
                 SELECT 'binance:spot', symbol, 'commission', 1, id, 1, $3, $4
                 FROM unnest($1::text[], $2::uuid[]) AS k(symbol,id)",
    )
    .bind(symbols)
    .bind(ids)
    .bind(entries[0].registered_at)
    .bind(&entries[0].provenance)
    .execute(pool)
    .await?;
    Ok(())
}

async fn zero_claims(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO n5_refresh_state (venue_id,symbol,descriptor,revision)
                 SELECT k.venue_id,k.symbol,k.descriptor,0 FROM n5_refresh_keys k
                 ON CONFLICT DO NOTHING",
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn account(symbol: &str) -> anyhow::Result<CostIdentity> {
    Ok(CostIdentity::new(
        Uuid::from_u128(4),
        VenueId::new(VenueId::BINANCE_SPOT),
        "test".to_owned(),
        18,
        Symbol::from_pair(symbol)?,
    )?)
}

#[test]
fn cold_receipt_refuses_at_exactly_day_and_before_original_bracket_end() {
    let domain = ClockDomain::new(Uuid::from_u128(1), -2, 345).unwrap();
    let raw = RawClockCapture {
        epoch: Uuid::from_u128(2),
        wall_seconds: 1_795_120_000,
        wall_nanoseconds: 123_456_789,
        domain,
        before_ns: 100,
        after_ns: 120,
    };
    let first = tick(domain, 100).unwrap();
    let calculate = |at| receipt_age(tick(domain, at).unwrap(), &raw, first);
    assert!(matches!(calculate(119), Err(BudgetError::PrerequisiteRefusal)));
    assert_eq!(calculate(120).unwrap(), Some(Duration::from_nanos(20)));
    assert_eq!(calculate(100 + DAY_NS - 1).unwrap().unwrap().as_nanos(), DAY_NS - 1);
    assert!(calculate(100 + DAY_NS).unwrap().is_none(), "exactly 24h must be stale");
    let foreign = ClockDomain::new(domain.boot(), -2, 346).unwrap();
    assert!(matches!(
        receipt_age(tick(foreign, 120).unwrap(), &raw, first),
        Err(BudgetError::PrerequisiteRefusal)
    ));
}

pub(in crate::venue_budget::n5_claim) async fn verify_real_receipt_controls(
    pool: &PgPool,
    identity: &CostIdentity,
) -> anyhow::Result<()> {
    let clock = NativeClock::initialize()?;
    let trace = Trace::default();
    let deadline = || Instant::now() + Duration::from_secs(2);
    let symbol = identity.symbol().as_pair();
    let rows = read_rows(pool, std::slice::from_ref(&symbol), deadline(), &trace)
        .await
        .map_err(|err| anyhow!("original receipt controls: {err:?}"))?;
    let row = rows
        .rows()
        .iter()
        .find(|row| row.try_get::<bool>("active").unwrap())
        .context("active receipt")?;
    let revision = row.try_get::<i64>("state_revision")?;
    let now = clock.capture()?;
    let (prior, proven) =
        state(row, revision, identity, now).map_err(|err| anyhow!("current receipt: {err:?}"))?;
    let prior = prior.context("current claim")?;
    let ReceiptState::Current(receipt) = proven else {
        return Err(anyhow!("positive control"));
    };
    let at_day =
        tick(receipt.original.domain, u128::try_from(receipt.original.before_ns)? + DAY_NS)
            .map_err(|err| anyhow!("exactly 24h: {err:?}"))?;
    ensure!(matches!(state(row, revision, identity, at_day), Ok((_, ReceiptState::Stale))));
    let wrong_account = CostIdentity::new(
        identity.tenant(),
        identity.venue().clone(),
        identity.scope().to_owned(),
        identity.uid() ^ 1,
        identity.symbol().clone(),
    )?;
    ensure!(matches!(state(row, revision, &wrong_account, now), Err(BudgetError::Invalid)));

    // Stage an unresolved next generation in this isolated test. M38 keeps
    // the complete previous snapshot; the loader must not promote it.
    let update = sqlx::query("UPDATE n5_refresh_state
        SET revision=revision+1, claim_id=$2, handoff_domain=NULL, handoff_tick=NULL, result_class=NULL
        WHERE venue_id='binance:spot' AND symbol=$1 AND descriptor='commission'
          AND claim_id=$3 AND result_class='success' AND snapshot_revision=revision")
        .bind(&symbol)
        .bind(Uuid::now_v7())
        .bind(prior.identity.claim_id)
        .execute(pool)
        .await?;
    ensure!(update.rows_affected() == 1);
    let kept: Vec<u8> = sqlx::query_scalar(
        "SELECT snapshot_payload FROM n5_refresh_state
        WHERE venue_id='binance:spot' AND symbol=$1 AND descriptor='commission'
          AND snapshot_revision=revision-1 AND result_class IS NULL",
    )
    .bind(symbol)
    .fetch_one(pool)
    .await?;
    ensure!(receipt.payload_digest.as_slice() == Sha256::digest(kept).as_slice());
    let reloaded = load(pool, std::slice::from_ref(identity), &clock, deadline(), &trace)
        .await
        .map_err(|err| anyhow!("unresolved next generation: {err:?}"))?;
    let [entry] = reloaded.as_slice() else {
        return Err(anyhow!("one configured key"));
    };
    ensure!(matches!(entry.receipt, ReceiptState::Unproven));
    ensure!(entry.prior.as_ref().is_some_and(|claim| claim.revision == revision + 1
        && claim.result.is_none()
        && claim.handoff.is_none()));
    ensure!(trace.issued() == 2);
    eprintln!("N5_COLD_CONTROLS exactly_24h=stale wrong_account=refused prior_payload_preserved=true unresolved_current=unproven");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires disposable PostgreSQL"]
async fn native_cold_manifest_refuses_absent_keys_and_never_certifies_revision_zero(
    pool: PgPool,
) -> anyhow::Result<()> {
    let clock = NativeClock::initialize()?;
    let trace = Trace::default();
    let request = account("FIX00000USDT")?;
    let deadline = || Instant::now() + Duration::from_secs(2);
    let read = || load(&pool, std::slice::from_ref(&request), &clock, deadline(), &trace);
    ensure!(matches!(read().await, Err(BudgetError::Invalid)));
    root(&pool, &mut [registration(0), registration(1)], true).await?;
    ensure!(
        matches!(read().await, Err(BudgetError::Invalid)),
        "whole manifest requires states"
    );
    zero_claims(&pool).await?;
    let rows = read().await.map_err(|err| anyhow!("exact manifest: {err:?}"))?;
    let [entry] = rows.as_slice() else {
        return Err(anyhow!("one configured key"));
    };
    ensure!(entry.key.registration_id == registration(0).registration_id);
    ensure!(entry.prior.is_none() && matches!(entry.receipt, ReceiptState::Unproven));
    let unknown = account("FIX00007USDT")?;
    ensure!(matches!(
        load(&pool, &[unknown], &clock, deadline(), &trace).await,
        Err(BudgetError::Invalid)
    ));
    ensure!(trace.issued() == 4);
    ensure!(matches!(
        load(&pool, &[request.clone(), request], &clock, deadline(), &trace).await,
        Err(BudgetError::Invalid)
    ));
    ensure!(trace.issued() == 4, "duplicate aliases refuse before SQL");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires disposable PostgreSQL"]
async fn native_cold_manifest_refuses_wrong_digest(pool: PgPool) -> anyhow::Result<()> {
    root(&pool, &mut [registration(0)], false).await?;
    zero_claims(&pool).await?;
    let trace = Trace::default();
    ensure!(matches!(
        load(
            &pool,
            &[account("FIX00000USDT")?],
            &NativeClock::initialize()?,
            Instant::now() + Duration::from_secs(2),
            &trace
        )
        .await,
        Err(BudgetError::Invalid)
    ));
    ensure!(trace.issued() == 1);
    Ok(())
}

async fn fake_bounded_payloads(pool: &PgPool, populated: usize) -> anyhow::Result<()> {
    // Legal M38 envelope, synthetic opaque bytes. The format decoder has its
    // own positive witness using the actual owner in a separate process.
    sqlx::query(
        "INSERT INTO n5_refresh_state (venue_id,symbol,descriptor,revision,
                     claim_id,owner_process,content_digest,claim_domain,claim_tick,claim_audit_at,
                     eligibility_kind,handoff_domain,handoff_tick,result_class,snapshot_claim_id,
                     snapshot_revision,snapshot_owner,snapshot_tenant,snapshot_scope,snapshot_uid,
                     snapshot_domain,snapshot_tick,snapshot_as_of,snapshot_validation_version,
                     snapshot_hash,snapshot_payload)
                 SELECT venue_id,symbol,descriptor,1,registration_id,$1,digest,dom,100,
                     CURRENT_TIMESTAMP,'first_use',dom,101,'success',registration_id,1,$1,$2,
                     repeat('s',128),18446744073709551615,dom,102,CURRENT_TIMESTAMP,1,digest,payload
                 FROM (SELECT * FROM n5_refresh_keys ORDER BY venue_id,symbol,descriptor LIMIT $3) k
                 CROSS JOIN (SELECT decode(repeat('ab',32),'hex') AS digest,
                     ROW('boottime',$4::uuid,0::bigint,0::integer)::budget_clock_domain_v38 AS dom,
                     decode(repeat('cd',16384),'hex') AS payload) v",
    )
    .bind(Uuid::from_u128(87))
    .bind(Uuid::from_u128(88))
    .bind(i64::try_from(populated)?)
    .bind(Uuid::from_u128(89))
    .execute(pool)
    .await?;
    zero_claims(pool).await
}

fn plan_node<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    if node["Index Name"].as_str() == Some(name) {
        return Some(node);
    }
    node["Plans"].as_array()?.iter().find_map(|child| plan_node(child, name))
}

async fn proxied(pool: &PgPool) -> anyhow::Result<(DbProxy, PgPool)> {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .context("fixture PostgreSQL endpoint")?;
    ensure!(upstream.ip().is_loopback());
    let proxy = DbProxy::start(upstream).await?;
    proxy.enable_query_trace(4);
    let url = database_url_through(
        &std::env::var("DATABASE_URL")?,
        proxy.addr(),
        options.get_database().context("fixture migrated database")?,
    )?;
    let measured = PgPoolOptions::new().connect_lazy(&url)?;
    Ok((proxy, measured))
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires disposable PostgreSQL"]
async fn native_cold_read_never_accepts_rows_without_completion(
    pool: PgPool,
) -> anyhow::Result<()> {
    root(&pool, &mut [registration(0), registration(1)], true).await?;
    zero_claims(&pool).await?;
    let (proxy, measured) = proxied(&pool).await?;
    let trace = Trace::default();
    let clock = NativeClock::initialize()?;
    let expected = [account("FIX00000USDT")?];
    proxy.set_mode(ProxyMode::DropAutocommitAckOf("LIMIT 4097"));
    ensure!(
        load(&measured, &expected, &clock, Instant::now() + Duration::from_secs(2), &trace)
            .await
            .is_err()
    );
    ensure!(proxy.autocommit_acks_dropped() == 1 && proxy.data_rows() == 2);
    ensure!(proxy.stats().statements == 1 && trace.issued() == 1);

    proxy.set_mode(ProxyMode::Blackhole);
    ensure!(matches!(
        load(
            &measured,
            &expected,
            &clock,
            Instant::now() + Duration::from_millis(150),
            &trace
        )
        .await,
        Err(BudgetError::Expired)
    ));
    ensure!(proxy.stats().statements == 1 && trace.issued() == 1);
    eprintln!(
        "N5_COLD_FAULT rows_without_completion=refused blackholed_connect=expired fallback_sql=0"
    );
    Ok(())
}

async fn measure(pool: &PgPool, active: usize, registered: usize) -> anyhow::Result<()> {
    let (proxy, measured) = proxied(pool).await?;
    let symbols: Vec<_> = (0..active).map(|i| registration(i).symbol).collect();
    let trace = Trace::default();
    let result =
        read_rows(&measured, &symbols, Instant::now() + Duration::from_secs(2), &trace).await;
    if registered == MAX_ROWS {
        ensure!(matches!(result, Err(BudgetError::Unpublished)), "4097th key must refuse");
    } else {
        let rows = result.map_err(|err| anyhow!("bounded raw read: {err:?}"))?;
        ensure!(rows.rows().len() == registered);
        let mut retained = 0;
        for row in rows.rows() {
            if row.try_get::<bool>("active")? {
                let payload = row.try_get::<&[u8]>("snapshot_payload")?;
                ensure!(payload.len() == super::super::result::MAX_PAYLOAD_BYTES);
                retained += 1;
            } else {
                ensure!(row.try_get::<Option<&[u8]>>("snapshot_payload")?.is_none());
            }
        }
        ensure!(retained == active);
    }
    timeout(Duration::from_secs(2), async {
        while proxy.closed_relays() == 0 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    let stats = proxy.stats();
    ensure!(stats.statements == 1 && stats.connections == 1 && stats.commits == 0);
    ensure!(trace.issued() == 1 && proxy.query_trace_dropped() == 0);
    ensure!(proxy.query_trace()[0].contains("LIMIT 4097"));
    let wire = proxy.query_response_bytes();
    let data_wire = proxy.data_row_bytes();
    ensure!(proxy.data_rows() as usize == registered && data_wire > 0 && wire > data_wire);
    ensure!(wire as usize <= MAX_BYTES);
    ensure!((wire - data_wire) as usize <= MAX_MESSAGE_METADATA_WIRE_BYTES);
    ensure!(
        data_wire as usize
            <= registered * MAX_COMPACT_WIRE_BYTES
                + active
                    * (MAX_ACTIVE_METADATA_WIRE_BYTES + super::super::result::MAX_PAYLOAD_BYTES)
    );

    // A separate test-only EXPLAIN does not go through the operation's proxy.
    let plan: Value = sqlx::query_scalar(&format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {SQL}"))
        .bind(&symbols)
        .fetch_one(pool)
        .await?;
    ensure!(plan[0]["Plan"]["Actual Rows"].as_u64() == Some(registered as u64));
    if registered >= MAX_KEYS {
        let pk = plan_node(&plan[0]["Plan"], "n5_refresh_keys_pkey")
            .context("native ordered registry PK")?;
        ensure!(pk["Actual Rows"].as_u64() == Some(registered as u64));
        let state = plan_node(&plan[0]["Plan"], "n5_refresh_state_pkey")
            .context("native bounded state PK")?;
        ensure!(state["Actual Rows"].as_u64() == Some(1));
        ensure!(state["Actual Loops"].as_u64() == Some(registered as u64));
    }
    eprintln!("N5_COLD_WIRE registered={registered} active={active} sql=1 data_rows={} data_row_bytes={data_wire} total_response_bytes={wire} bounded_plan=true", proxy.data_rows());
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires disposable PostgreSQL"]
async fn native_cold_wire_two_keys_projects_one_full_payload(pool: PgPool) -> anyhow::Result<()> {
    root(&pool, &mut [registration(0), registration(1)], true).await?;
    fake_bounded_payloads(&pool, 2).await?;
    measure(&pool, 1, 2).await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires disposable PostgreSQL"]
async fn native_cold_wire_maximum_and_overflow_sentinel(pool: PgPool) -> anyhow::Result<()> {
    let mut all: Vec<_> = (0..MAX_KEYS).map(registration).collect();
    root(&pool, &mut all, true).await?;
    fake_bounded_payloads(&pool, MAX_ACTIVE + 1).await?;
    sqlx::raw_sql("ANALYZE n5_refresh_keys; ANALYZE n5_refresh_state;")
        .execute(&pool)
        .await?;
    measure(&pool, MAX_ACTIVE, MAX_KEYS).await?;
    keys(&pool, &[registration(MAX_KEYS)]).await?;
    zero_claims(&pool).await?;
    measure(&pool, MAX_ACTIVE, MAX_ROWS).await
}

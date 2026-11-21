#![cfg(all(test, feature = "crash-points"))]
//! Native C2 fixture stages for the held, physically restored local PG16 run.
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use robson_domain::Symbol;
use robson_exec::{
    balance_costs::{
        native_n5::{fixture_manifest, ReceiptState, Registration},
        CostIdentity,
    },
    venue::VenueId,
    venue_clock::NativeClock,
};
use sqlx::{ConnectOptions, PgPool};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

use super::cold_load;
use crate::venue_budget::Trace;

const CUT: &str = include_str!("rehearsal-legacy-reason-drop.sql");
const SETTINGS: &str = "SELECT set_config('lock_timeout','20ms',true),set_config('statement_timeout','40ms',true),set_config('idle_in_transaction_session_timeout','20ms',true),set_config('robson_cut.old_oid',$1,true),set_config('robson_cut.new_oid',$2,true),set_config('robson_cut.old_def',$3,true),set_config('robson_cut.new_def',$4,true)";

/// The current fixture only has an initialized synthetic registry. Revision
/// zero explicitly supplies no commission receipt or authority to send.
pub(crate) async fn seed_manifest(pool: &PgPool, symbol: &str) -> Result<()> {
    let registered_at =
        chrono::DateTime::from_timestamp_micros(chrono::Utc::now().timestamp_micros())
            .context("fixture registration time")?;
    let mut registration = [Registration {
        venue_id: VenueId::BINANCE_SPOT.to_owned(),
        symbol: symbol.to_owned(),
        descriptor: "commission".to_owned(),
        key_registry: 1,
        registration_id: Uuid::now_v7(),
        registered_revision: 1,
        registered_at,
        provenance: "held-pg16-no-capital".to_owned(),
    }];
    let digest = fixture_manifest(&mut registration);
    let [entry] = registration;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO public.n5_registry_root VALUES
                 (1,38,1,1,true,1,$1,$2,0,'normal',NULL,CURRENT_TIMESTAMP)",
    )
    .bind(digest.as_slice())
    .bind(Uuid::now_v7())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO public.n5_refresh_keys
         VALUES ('binance:spot',$1,'commission',1,$2,1,$3,$4)",
    )
    .bind(&entry.symbol)
    .bind(entry.registration_id)
    .bind(entry.registered_at)
    .bind(&entry.provenance)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO public.n5_refresh_state (venue_id,symbol,descriptor,revision)
         VALUES ('binance:spot',$1,'commission',0)",
    )
    .bind(&entry.symbol)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn load_fixture_registry(
    pool: &PgPool,
    tenant: Uuid,
    scope: &str,
    uid: u64,
    symbol: &str,
    clock: &NativeClock,
    deadline: Instant,
) -> Result<()> {
    let actual: String = timeout_at(
        deadline,
        sqlx::query_scalar(
            "SELECT uid FROM public.venue_accounts WHERE tenant_id=$1 AND account_scope=$2
             AND venue_id='binance:spot'",
        )
        .bind(tenant)
        .bind(scope)
        .fetch_one(pool),
    )
    .await??;
    ensure!(actual == uid.to_string(), "restored fixture account UID differs");
    let identity = CostIdentity::new(
        tenant,
        VenueId::new(VenueId::BINANCE_SPOT),
        scope.to_owned(),
        uid,
        Symbol::from_pair(symbol)?,
    )?;
    let trace = Trace::default();
    let history = cold_load::load(pool, &[identity], clock, deadline, &trace)
        .await
        .map_err(|e| anyhow::anyhow!("native N5 verification: {e:?}"))?;
    let [entry] = history.entries() else {
        anyhow::bail!("same fixture registry key expected");
    };
    ensure!(
        trace.issued() == 1
            && entry.prior.is_none()
            && matches!(&entry.receipt, ReceiptState::Unproven),
        "native registry must stay unproven for commission"
    );
    Ok(())
}

/// The host independently verified the full pre-cut native catalogue. Join
/// the exact OIDs/definitions again inside the previously reviewed cut's lock.
pub(crate) async fn cut_legacy_reason(pool: &PgPool) -> Result<()> {
    let snapshot_deadline = Instant::now() + Duration::from_secs(2);
    let checks: Vec<(String, i64, String)> = timeout_at(
        snapshot_deadline,
        sqlx::query_as(
            "SELECT conname::text, oid::int8, pg_catalog.pg_get_constraintdef(oid)
             FROM pg_catalog.pg_constraint
             WHERE conrelid='public.recovery_incidents'::regclass AND contype='c' AND convalidated
               AND conname IN ('recovery_incidents_reason_check','recovery_incidents_reason_v38')
             ORDER BY conname",
        )
        .fetch_all(pool),
    )
    .await??;
    let [(old_name, old_oid, old_def), (new_name, new_oid, new_def)] = checks.as_slice() else {
        anyhow::bail!("both exact validated reason constraints must be present");
    };
    ensure!(
        old_name == "recovery_incidents_reason_check"
            && new_name == "recovery_incidents_reason_v38"
    );
    ensure!(
        old_def.len() <= 1024
            && new_def.len() <= 1024
            && old_oid != new_oid
            && *old_oid > 0
            && *new_oid > 0
    );
    let deadline = Instant::now() + Duration::from_millis(200);
    let work = async {
        let mut c = pool.connect_options().connect().await?;
        sqlx::raw_sql("BEGIN").execute(&mut c).await?;
        let echoes: (String, String, String, String, String, String, String) =
            sqlx::query_as(SETTINGS)
                .bind(old_oid.to_string())
                .bind(new_oid.to_string())
                .bind(old_def)
                .bind(new_def)
                .fetch_one(&mut c)
                .await?;
        ensure!(
            echoes
                == (
                    "20ms".into(),
                    "40ms".into(),
                    "20ms".into(),
                    old_oid.to_string(),
                    new_oid.to_string(),
                    old_def.clone(),
                    new_def.clone()
                )
        );
        sqlx::raw_sql(CUT).execute(&mut c).await?;
        ensure!(Instant::now() < deadline, "legacy cut deadline before COMMIT");
        sqlx::raw_sql("COMMIT").execute(&mut c).await?;
        ensure!(Instant::now() < deadline, "legacy cut acknowledgement missed the deadline");
        Ok::<_, anyhow::Error>(())
    };
    timeout_at(deadline, work).await.context("legacy cut outcome unconfirmed")?
}

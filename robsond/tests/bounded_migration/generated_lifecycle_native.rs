//! Native lifecycle and independent persisted state; test-owned holders only.
use robson_testkit::db_proxy::DbProxy;
use sqlx::{postgres::PgSslMode, Connection, PgConnection};

use super::*;

#[path = "generated_lifecycle_native_dispose.rs"]
mod dispose;
pub(super) use dispose::{dispose, dispose_resources};

pub(super) const SETTINGS:&str="SELECT pg_backend_pid(),current_setting('lock_timeout'),current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout')";
pub(super) const GONE:&str="SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1) AND NOT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1)";
const HISTORY:&str="SELECT COALESCE(jsonb_agg((to_jsonb(m)||jsonb_build_object('checksum',encode(m.checksum,'hex'))) ORDER BY version),'[]'::jsonb) FROM (SELECT * FROM _sqlx_migrations ORDER BY version LIMIT 65) m";
type Metadata = (String, bool, Vec<u8>, i64);
pub(super) struct Oracle {
    before: Value,
    reference: Value,
    applied: Option<Value>,
    old_history: Value,
    applied_history: Option<Value>,
    metadata: Option<Metadata>,
    legacy: Value,
    lost_reply: bool,
}
pub(super) struct Parked {
    proxy: DbProxy,
    observer: PgConnection,
    pub pid: i32,
    full: Value,
    history: Value,
    legacy: Value,
    pub metadata: Option<Metadata>,
    witness: Vec<(i32, String, String, String)>,
    catalog_before: Value,
    legacy_before: Value,
}
fn same(a: &Value, b: &Value, native: bool) -> Result<()> {
    let (a, ai) = contract::maps(a)?;
    let (b, bi) = contract::maps(b)?;
    ensure!(
        a == b && (!native || ai == bi),
        "generated full semantic/native catalog mismatch"
    );
    Ok(())
}
async fn history(c: &mut PgConnection) -> Result<Value> {
    let h: Value = sqlx::query_scalar(HISTORY).fetch_one(c).await?;
    ensure!(
        h.as_array().is_some_and(|v| v.len() <= 64) && serde_json::to_vec(&h)?.len() <= 65536,
        "generated history64/64KiB"
    );
    Ok(h)
}
fn before39(h: &Value) -> Result<Value> {
    Ok(Value::Array(
        h.as_array()
            .context("history array")?
            .iter()
            .filter(|r| r["version"].as_i64() != Some(VERSION + 1))
            .cloned()
            .collect(),
    ))
}
pub(super) async fn prepare(f: &mut Fixture, profile: Profile) -> Result<Oracle> {
    prepare_with(f, matches!(profile, Profile::Deferred), matches!(profile, Profile::LostReply))
        .await
}

/// `deferred` installs the commit-rejecting trigger before B0 for the whole
/// fixture; `lost_reply` sets the committed elapsed expectation (-1).
pub(super) async fn prepare_with(
    f: &mut Fixture,
    deferred: bool,
    lost_reply: bool,
) -> Result<Oracle> {
    contract::source_hash()?;
    // Fixed TEST profile before B0; never removed between same-DB attempts.
    sqlx::query("CREATE TABLE generated_native_witness(actual_pid integer NOT NULL,lock_limit text NOT NULL,statement_limit text NOT NULL,idle_limit text NOT NULL)").persistent(false).execute(&mut f.observer).await?;
    if deferred {
        sqlx::query("CREATE FUNCTION generated_reject_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.version=20240101000038 THEN RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='generated commit rejected'; END IF; RETURN NEW; END $$").persistent(false).execute(&mut f.observer).await?;
        sqlx::query("CREATE CONSTRAINT TRIGGER generated_reject_commit AFTER INSERT ON _sqlx_migrations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION generated_reject_commit()").persistent(false).execute(&mut f.observer).await?;
    }
    let before = capture(&mut f.observer, "B0").await?;
    ensure!(metadata(&mut f.observer).await?.is_none(), "generated initial38 present");
    let old_history = history(&mut f.observer).await?;
    let legacy = f.legacy_before.clone();
    let n = f.proxy.stats().statements;
    f.adapter.lock().await?;
    ensure!(f.proxy.stats().statements - n == 2, "generated real original advisory setup2");
    let reference = {
        let mut tx = f.adapter.connection()?.begin().await?;
        sqlx::raw_sql(contract::ORIGINAL).execute(&mut *tx).await?;
        let r = capture(&mut tx, "R").await?;
        tx.rollback().await?;
        r
    };
    contract::declared(&reference)?;
    same(&before, &capture(&mut f.observer, "B1").await?, true)?;
    f.baseline = f.proxy.stats();
    f.proxy.enable_query_trace(32);
    eprintln!("GENERATED_SETUP fixed_fixture_once=true trigger_DDL={} full_captures=3 metadata_reads=1 history_reads=1 advisory_SQL=2 reference_owner_SQL=4",if deferred{3}else{1});
    Ok(Oracle {
        before,
        reference,
        applied: None,
        old_history,
        applied_history: None,
        metadata: None,
        legacy,
        lost_reply,
    })
}

pub(super) async fn park(f: Fixture) -> Result<Parked> {
    let Fixture {
        adapter,
        proxy,
        mut observer,
        pid,
        catalog_before,
        legacy_before,
        ..
    } = f;
    let before = proxy.stats();
    let end = tokio::time::Instant::now() + ACQUIRE_BOUND;
    drop(adapter); // ORIGINAL raw owner, before any serialized observation.
    if pid <= 0 {
        // A failed first PID query supplies no native retirement identity.
        drop(observer);
        proxy.set_mode(ProxyMode::Sever);
        drop(proxy);
        anyhow::bail!("generated unknown PID; raw-drop-only cleanup; native retirement unproved");
    }
    let retired = tokio::time::timeout(Duration::from_millis(300), async {
        for n in 1..=60 {
            if sqlx::query_scalar::<_, bool>(GONE).bind(pid).fetch_one(&mut observer).await? {
                return Ok::<_, sqlx::Error>(n);
            }
            if n < 60 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        Err(sqlx::Error::Protocol("generated original PID/locks not retired".into()))
    })
    .await
    .context("generated native retirement300ms")
    .and_then(|r| r.map_err(anyhow::Error::from));
    // Attempt ownership disposal on every path; a failed retirement grants no
    // later run/reconnect or authoritative absence observation.
    let snapshots = if retired.is_ok() {
        tokio::time::timeout_at(end, async {
            observer.lock().await?;
            let full = capture(&mut observer, "C").await?;
            let legacy = legacy(&mut observer).await?;
            let metadata = metadata(&mut observer).await?;
            let history = history(&mut observer).await?;
            let witness = sqlx::query_as(engine::WITNESS).fetch_all(&mut observer).await?;
            observer.unlock().await?;
            Ok::<_, anyhow::Error>((full, legacy, metadata, history, witness))
        })
        .await
        .context("generated fresh advisory/snapshots5s")
        .and_then(|r| r)
    } else {
        Err(anyhow::anyhow!("native retirement unproved; no absence authority"))
    };
    eprintln!("GENERATED_BARRIER pid={pid} retirement={} snapshots={} polls={:?} observer_SQL_after_poll=9",cause(&retired),cause(&snapshots),retired.as_ref().ok());
    let checked = (|| -> Result<_> {
        retired?;
        let snapshots = snapshots?;
        ensure!(
            proxy.stats() == before && proxy.query_trace_dropped() == 0,
            "generated whole drop flushed hidden SQL/lost trace"
        );
        Ok(snapshots)
    })();
    match checked {
        Ok((full, legacy, metadata, history, witness)) => Ok(Parked {
            proxy,
            observer,
            pid,
            full,
            legacy,
            metadata,
            history,
            witness,
            catalog_before,
            legacy_before,
        }),
        Err(e) => {
            drop(observer);
            proxy.set_mode(ProxyMode::Sever);
            drop(proxy);
            Err(e)
        },
    }
}

pub(super) fn validate(p: &Parked, o: &mut Oracle, continued: bool) -> Result<()> {
    ensure!(p.legacy == o.legacy, "generated legacy/pre38 history changed");
    let rows = p.history.as_array().context("generated history shape")?;
    let original: Vec<_> = rows
        .iter()
        .filter(|r| r["version"].as_i64().is_some_and(|v| v < VERSION))
        .cloned()
        .collect();
    ensure!(Value::Array(original) == o.old_history, "generated pre38 lineage changed");
    if let Some(m) = &p.metadata {
        ensure!(
            m.0 == migration().description.as_ref()
                && m.1
                && m.2 == migration().checksum.as_ref()
                && if o.lost_reply { m.3 == -1 } else { m.3 >= 0 },
            "generated original38 metadata/checksum/elapsed"
        );
        contract::declared(&p.full)?;
        same(&o.reference, &p.full, false)?;
        if let Some(old) = &o.applied {
            same(old, &p.full, true)?;
        } else {
            o.applied = Some(p.full.clone());
        }
        if let Some(old) = &o.metadata {
            ensure!(m == old, "generated applied38 metadata rewritten");
        } else {
            o.metadata = Some(m.clone());
        }
        let h = before39(&p.history)?;
        if let Some(old) = &o.applied_history {
            ensure!(&h == old, "generated original history changed after ordinary skip");
        } else {
            o.applied_history = Some(h);
        }
        ensure!(
            rows.len()
                == o.old_history.as_array().context("old history")?.len()
                    + 1
                    + usize::from(continued),
            "generated unexpected migration/39"
        );
    } else {
        same(&o.before, &p.full, true)?;
        ensure!(
            p.history == o.old_history && !continued,
            "generated unresolved/unapplied history changed or39 ran"
        );
    }
    let expected = if continued {
        vec![(p.pid, "310ms".into(), "410ms".into(), "510ms".into())]
    } else {
        vec![]
    };
    ensure!(p.witness == expected, "generated actual ordinary39 PID/GUC/count witness");
    contract::source_hash()?;
    Ok(())
}

pub(super) async fn reconnect(p: Parked, pool: &PgPool) -> Result<(Fixture, i32)> {
    let Parked {
        proxy,
        observer,
        pid: old_pid,
        catalog_before,
        legacy_before,
        ..
    } = p;
    proxy.set_mode(ProxyMode::Pass);
    let options = pool
        .connect_options()
        .as_ref()
        .clone()
        .host("127.0.0.1")
        .port(proxy.addr().port())
        .ssl_mode(PgSslMode::Disable);
    let adapter = BoundedMigration::connect(&options).await?;
    let baseline = proxy.stats();
    // Caller takes ownership BEFORE the first fallible configuration query,
    // ensuring its unconditional cleanup sees the new actual raw owner.
    Ok((
        Fixture {
            adapter,
            proxy,
            observer,
            pid: 0,
            catalog_before,
            legacy_before,
            baseline,
        },
        old_pid,
    ))
}
pub(super) async fn configure_reconnect(f: &mut Fixture, old_pid: i32) -> Result<()> {
    f.pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(f.adapter.connection()?)
        .await?;
    ensure!(f.pid != old_pid, "generated reconnect reused original PID");
    sqlx::query("SELECT set_config('lock_timeout','310ms',false),set_config('statement_timeout','410ms',false),set_config('idle_in_transaction_session_timeout','510ms',false)").execute(f.adapter.connection()?).await?;
    let n = f.proxy.stats().statements;
    f.adapter.lock().await?;
    ensure!(
        f.proxy.stats().statements - n == 2,
        "generated new owner's actual advisory setup2"
    );
    f.baseline = f.proxy.stats();
    f.proxy.enable_query_trace(32);
    eprintln!(
        "GENERATED_RECONNECT old_pid={old_pid} new_pid={} setup_owner_SQL=4 reseed_SQL=0",
        f.pid
    );
    Ok(())
}

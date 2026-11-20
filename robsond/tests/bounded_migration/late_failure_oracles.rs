//! Bounded independent original-DO catalog oracle and whole-owner cleanup.
use super::*;

const COMMITTED: &str = "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.pid=$1 AND a.backend_start::text=$2 AND a.state='idle' AND a.xact_start IS NULL AND EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND granted) AND NOT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_class c ON c.oid=l.relation JOIN pg_namespace n ON n.oid=c.relnamespace WHERE l.pid=$1 AND n.nspname='public') AND EXISTS(SELECT 1 FROM _sqlx_migrations m WHERE m.version=$3 AND m.success AND m.description=$4 AND m.checksum=$5 AND CASE WHEN $6 THEN m.execution_time=-1 ELSE m.execution_time>=0 END))";

pub(super) struct Oracle {
    pub(super) before: Value,
    pub(super) reference: Value,
    pub(super) applied: Option<Value>,
    pub(super) backend_start: String,
    pub(super) committed_at_gate: bool,
}

pub(super) fn same(a: &Value, b: &Value, native: bool) -> Result<()> {
    let (a_sem, a_ids) = contract::maps(a)?;
    let (b_sem, b_ids) = contract::maps(b)?;
    ensure!(a_sem == b_sem && (!native || a_ids == b_ids), "complete catalog mismatch");
    Ok(())
}
pub(super) fn database_code(error: &MigrateError) -> Option<String> {
    match error {
        MigrateError::Execute(sqlx::Error::Database(db))
        | MigrateError::ExecuteMigration(sqlx::Error::Database(db), _) => {
            db.code().map(|s| s.into_owned())
        },
        _ => None,
    }
}
pub(super) async fn committed(
    c: &mut sqlx::PgConnection,
    pid: i32,
    start: &str,
    fault: Fault,
) -> Result<bool> {
    Ok(sqlx::query_scalar(COMMITTED)
        .bind(pid)
        .bind(start)
        .bind(migration().version)
        .bind(migration().description.as_ref())
        .bind(migration().checksum.as_ref())
        .bind(matches!(fault, Fault::ElapsedUpdate))
        .fetch_one(c)
        .await?)
}

pub(super) fn error_class(error: &MigrateError) -> &'static str {
    match error {
        MigrateError::Execute(sqlx::Error::Database(_)) => "execute_database",
        MigrateError::ExecuteMigration(sqlx::Error::Database(_), _) => "migration_database",
        MigrateError::Execute(sqlx::Error::Io(_)) => "execute_io",
        MigrateError::Source(source) => match source.downcast_ref::<std::io::Error>() {
            Some(e)
                if e.kind() == std::io::ErrorKind::Other
                    && e.to_string() == "migration38 whole-apply deadline exceeded" =>
            {
                "source_global200"
            },
            Some(_) => "source_io_other",
            None => "source_other",
        },
        MigrateError::Execute(_) => "execute_other",
        _ => "migrate_other",
    }
}

pub(super) async fn post_fault(f: &mut Fixture, oracle: &mut Oracle, fault: Fault) -> Result<()> {
    if fault.applied() && oracle.committed_at_gate {
        // AFTER actual apply return, outside APPLY_BOUND. Not an at-gate snapshot.
        let full = capture(&mut f.observer, "C").await?;
        contract::declared(&full)?;
        same(&oracle.reference, &full, false)?;
        oracle.applied = Some(full);
    }
    Ok(())
}

pub(super) async fn prepare(f: &mut Fixture, fault: Fault) -> Result<Oracle> {
    contract::source_hash()?;
    // Test-owned DDL is installed BEFORE B0, never removed to conceal drift.
    let trigger = match fault {
        Fault::DeferredCommit => Some((
            "CREATE FUNCTION late_reject_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.version=20240101000038 THEN RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='late commit rejected'; END IF; RETURN NEW; END $$",
            "CREATE CONSTRAINT TRIGGER late_reject_commit AFTER INSERT ON _sqlx_migrations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION late_reject_commit()")),
        Fault::ElapsedUpdate => Some((
            "CREATE FUNCTION late_reject_elapsed() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.version=20240101000038 THEN RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='late elapsed rejected'; END IF; RETURN NEW; END $$",
            "CREATE TRIGGER late_reject_elapsed BEFORE UPDATE ON _sqlx_migrations FOR EACH ROW EXECUTE FUNCTION late_reject_elapsed()")),
        Fault::RestoreSever => None,
    };
    if let Some((function, trigger)) = trigger {
        sqlx::query(function).persistent(false).execute(&mut f.observer).await?;
        sqlx::query(trigger).persistent(false).execute(&mut f.observer).await?;
    }
    let before = capture(&mut f.observer, "B0").await?;
    ensure!(metadata(&mut f.observer).await?.is_none(), "38 already applied");
    let setup = f.proxy.stats().statements;
    f.adapter.lock().await?;
    ensure!(f.proxy.stats().statements - setup == 2, "advisory setup inventory");
    // Independent original DO, not candidate chunks. All outside APPLY_BOUND apply.
    let reference = {
        let mut tx = f.adapter.connection()?.begin().await?;
        sqlx::raw_sql(contract::ORIGINAL).execute(&mut *tx).await?;
        let reference = capture(&mut tx, "R").await?;
        tx.rollback().await?;
        reference
    };
    contract::declared(&reference)?;
    same(&before, &capture(&mut f.observer, "B1").await?, true)?;
    let backend_start: String =
        sqlx::query_scalar("SELECT backend_start::text FROM pg_stat_activity WHERE pid=$1")
            .bind(f.pid)
            .fetch_one(&mut f.observer)
            .await?;
    ensure!(
        !committed(&mut f.observer, f.pid, &backend_start, fault).await?,
        "warm committed witness found preexisting38"
    );
    f.baseline = f.proxy.stats();
    f.proxy.enable_query_trace(32);
    Ok(Oracle {
        before,
        reference,
        applied: None,
        backend_start,
        committed_at_gate: false,
    })
}

pub(super) async fn cleanup(f: Fixture, oracle: Option<&Oracle>, fault: Fault) -> Result<()> {
    let Fixture {
        adapter,
        proxy,
        mut observer,
        pid,
        legacy_before,
        baseline,
        ..
    } = f;
    let deadline = tokio::time::Instant::now() + ACQUIRE_BOUND;
    drop(adapter); // Drop whole raw owner before any observer/barrier work.
    let retired = tokio::time::timeout(NATIVE_RETIREMENT, async {
        for _ in 0..NATIVE_RETIREMENT_POLLS {
            if !alive(&mut observer, pid).await? {
                return Ok::<_, sqlx::Error>(true);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(false)
    })
    .await;
    let stats = proxy.stats();
    let snapshots = tokio::time::timeout_at(deadline, async {
        observer.lock().await?;
        let full = capture(&mut observer, "C").await?;
        let legacy = legacy(&mut observer).await?;
        let metadata = metadata(&mut observer).await?;
        observer.unlock().await?;
        Ok::<_, anyhow::Error>((full, legacy, metadata))
    })
    .await;
    drop(observer);
    proxy.set_mode(ProxyMode::Sever);
    drop(proxy);
    ensure!(retired??, "actual original backend not retired");
    let (full, legacy, metadata) = snapshots??;
    ensure!(legacy == legacy_before, "legacy/history changed");
    let oracle = oracle.context("setup did not finish; no catalog oracle")?;
    if fault.applied() {
        same(&oracle.reference, &full, false)?;
        same(
            oracle
                .applied
                .as_ref()
                .context("post-apply-return APPLIED native baseline missing")?,
            &full,
            true,
        )?;
        contract::declared(&full)?;
        let (description, success, checksum, elapsed) =
            metadata.context("committed38 metadata missing")?;
        ensure!(
            description == migration().description.as_ref()
                && success
                && checksum.as_slice() == migration().checksum.as_ref(),
            "original38 lineage drift"
        );
        ensure!(
            if matches!(fault, Fault::ElapsedUpdate) {
                elapsed == -1
            } else {
                elapsed >= 0
            },
            "elapsed durability boundary wrong"
        );
    } else {
        same(&oracle.before, &full, true)?;
        ensure!(metadata.is_none(), "deferred rejection committed38");
    }
    ensure!(
        stats.statements - baseline.statements == fault.prefix(),
        "retirement flushed hidden SQL"
    );
    contract::source_hash()?;
    Ok(())
}

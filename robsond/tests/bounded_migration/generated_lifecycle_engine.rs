//! Existing cap1 stages schedule the ORIGINAL future, no protocol authority.
use super::*;

pub(super) const WITNESS: &str =
    "SELECT actual_pid,lock_limit,statement_limit,idle_limit FROM generated_native_witness LIMIT 2";
const SQL39: &str = "INSERT INTO generated_native_witness SELECT pg_backend_pid(),current_setting('lock_timeout'),current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout')";

pub(super) async fn attempt(f: &mut Fixture, profile: Profile, seed: u64) -> Result<()> {
    let offset = usize::try_from((seed - 0x8a410001) % 6)?;
    let milestones = [
        [10, 26, 42],
        [11, 27, 43],
        [12, 28, 44],
        [13, 29, 45],
        [14, 30, 46],
        [16, 32, 48],
    ][offset];
    attempt_with(f, profile, milestones, seed).await
}

/// One actual attempt released at three explicit DDL milestones and COMMIT.
/// Each received window must stay within 18 statements: m0 <= 16,
/// consecutive gaps <= 18 and m2 >= 35 (COMMIT is statement 55).
pub(super) async fn attempt_with(
    f: &mut Fixture,
    profile: Profile,
    milestones: [u8; 3],
    seed: u64,
) -> Result<()> {
    ensure!(
        (1..=16).contains(&milestones[0])
            && milestones[0] < milestones[1]
            && milestones[1] < milestones[2]
            && milestones[2] <= 52
            && milestones[1] - milestones[0] <= 18
            && milestones[2] - milestones[1] <= 18
            && milestones[2] >= 35,
        "generated milestone triple outside the bounded window contract"
    );
    let points = [
        Stage::Ddl(milestones[0]),
        Stage::Ddl(milestones[1]),
        Stage::Ddl(milestones[2]),
        Stage::Commit,
    ];
    let counts = [
        u64::from(milestones[0]) + 2,
        u64::from(milestones[1]) + 2,
        u64::from(milestones[2]) + 2,
        55,
    ];
    let slot = f.adapter.gate.clone();
    let first = stages::arm(&slot, points[0], Edge::AfterReturn)?;
    let before = f.proxy.stats();
    let proxy = &f.proxy;
    let controller = async {
        let mut current = first;
        let mut windows = Vec::with_capacity(4);
        for n in 0..4 {
            current.0.await.map_err(|_| anyhow::anyhow!("generated stage absent"))?;
            windows.push(Window {
                text: proxy.query_trace(),
                dropped: proxy.query_trace_dropped(),
                statements: proxy.stats().statements - before.statements,
            });
            let next = if n < 3 {
                Some(stages::arm(
                    &slot,
                    points[n + 1],
                    if n == 2 {
                        Edge::BeforeCall
                    } else {
                        Edge::AfterReturn
                    },
                )?)
            } else {
                None
            };
            if n == 3 && matches!(profile, Profile::LostReply) {
                ensure!(proxy.mode() == ProxyMode::Pass, "unexpected preexisting fault");
                proxy.set_mode(ProxyMode::DropCommitAck);
            }
            proxy.enable_exact_query_trace(32);
            current.1.send(()).map_err(|_| anyhow::anyhow!("generated release lost"))?;
            if let Some(next) = next {
                current = next;
            } else {
                break;
            }
        }
        Ok::<_, anyhow::Error>(windows)
    };
    let began = Instant::now();
    let producer = async {
        let r = f.adapter.apply(migration()).await;
        (r, began.elapsed())
    };
    let ((result, elapsed), windows) =
        tokio::join!(producer, tokio::time::timeout(CONTROLLER_CEILING, controller));
    let after = f.proxy.stats();
    let status = f.adapter.status();
    let refused = if status.phase == Phase::Retired {
        refuses_after_retirement(&mut f.adapter).await
    } else {
        false
    };
    let after_refusals = f.proxy.stats();
    let (class, code) = match result.as_ref().err() {
        Some(MigrateError::Execute(sqlx::Error::Database(d))) => {
            ("database", d.code().map(|c| c.into_owned()))
        },
        Some(MigrateError::Execute(sqlx::Error::Io(_))) => ("io", None),
        Some(MigrateError::ExecuteMigration(sqlx::Error::Database(d), _)) => {
            ("migration_database", d.code().map(|c| c.into_owned()))
        },
        Some(_) => ("other", None),
        None => ("ok", None),
    };
    let code = code
        .as_deref()
        .filter(|s| s.len() == 5 && s.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
        .unwrap_or("none");
    eprintln!("GENERATED_ERROR seed={seed:016x} class={class} sqlstate={code}");
    eprintln!("GENERATED_APPLY seed={seed:016x} profile={profile:?} elapsed={elapsed:?} status={status:?} result={} prefix={} commits={}",if result.is_ok(){"ok"}else{"error"},after.statements-before.statements,after.commits-before.commits);
    ensure!(elapsed < APPLY_BOUND, "generated real apply missed APPLY_BOUND; no retry");
    let windows = windows.context("generated controller ceiling")??;
    let mut all = Vec::with_capacity(58);
    let mut previous = 0;
    for (index, (w, n)) in windows.iter().zip(counts).enumerate() {
        eprintln!(
            "GENERATED_WINDOW seed={seed:016x} n={index} statements={} len={} dropped={}",
            w.statements,
            w.text.len(),
            w.dropped
        );
        ensure!(
            w.dropped == 0
                && w.statements == n
                && w.text.len() as u64 == n - previous
                && n - previous <= 18,
            "generated bounded received window"
        );
        previous = n;
        all.extend(w.text.iter().map(|s| s.trim().to_owned()));
    }
    ensure!(windows.len() == 4, "generated four windows missing");
    all.extend(f.proxy.query_trace().iter().map(|s| s.trim().to_owned()));
    for (n, sql) in all.iter().take(58).enumerate() {
        let prefix: String = sql.chars().take(64).collect();
        eprintln!(
            "GENERATED_RECEIVED seed={seed:016x} n={n} bytes={} prefix={}",
            sql.len(),
            serde_json::to_string(&prefix)?
        );
    }
    let expected = if matches!(profile, Profile::Deferred | Profile::LostReply) {
        56
    } else {
        58
    };
    ensure!(
        after.statements - before.statements == expected
            && all.len() as u64 == expected
            && f.proxy.query_trace_dropped() == 0
            && after.commits - before.commits == 1,
        "generated actual exact prefix/COMMIT"
    );
    ensure!(
        all[0].starts_with("WITH old AS MATERIALIZED") && all[1] == "BEGIN",
        "generated setup/BEGIN provenance"
    );
    for (actual, source) in all[2..54].iter().zip(fixed52::chunks(migration())?) {
        ensure!(actual.trim() == source.trim(), "generated original52 source/order");
    }
    ensure!(
        all[54] == public_apply::metadata_insert_sql_for_tests().trim() && all[55] == "COMMIT",
        "generated metadata55/COMMIT56 provenance"
    );
    if expected == 58 {
        result?;
        ensure!(
            all[56].starts_with("UPDATE _sqlx_migrations")
                && all[57].starts_with("SELECT set_config(")
                && status == (Status { phase: Phase::Restored, failure: None }),
            "generated restoration not complete"
        );
        let p: (i32, String, String, String) = sqlx::query_as(native::SETTINGS)
            .persistent(false)
            .fetch_one(f.adapter.connection()?)
            .await?;
        ensure!(
            p == (f.pid, "310ms".into(), "410ms".into(), "510ms".into()),
            "generated original SAME PID/GUCs not restored"
        );
        ensure!(
            f.proxy.stats().statements - after.statements == 1,
            "restored settings readback SQL count"
        );
    } else {
        let error = result.as_ref().err().context("generated fault unexpectedly succeeded")?;
        let actual = match error {
            MigrateError::Execute(sqlx::Error::Database(d)) => d.code().map(|s| s.into_owned()),
            _ => None,
        };
        ensure!(
            if matches!(profile, Profile::Deferred) {
                actual.as_deref() == Some("23514")
            } else {
                matches!(error, MigrateError::Execute(sqlx::Error::Io(_)))
            },
            "generated wrong actual native fault class"
        );
        ensure!(
            status
                == (Status {
                    phase: Phase::Retired,
                    failure: Some(Failure {
                        at: Phase::DelegatedApply,
                        outcome: Outcome::MayHaveCommitted
                    })
                })
                && refused
                && after_refusals == after,
            "generated failed owner reused/sent SQL or39"
        );
    }
    ensure!(
        after.commit_acks_dropped - before.commit_acks_dropped
            == u64::from(matches!(profile, Profile::LostReply))
            && f.proxy.autocommit_acks_dropped() == 0,
        "generated ACK fault count"
    );
    Ok(())
}

pub(super) async fn ordinary(f: &mut Fixture) -> Result<()> {
    use sqlx::migrate::MigrationType;
    let mut source = sqlx::migrate!("../migrations");
    let next = Migration::new(
        VERSION + 1,
        "generated ordinary39 witness".into(),
        MigrationType::Simple,
        SQL39.into(),
        false,
    );
    let mut list: Vec<_> = source.iter().filter(|m| m.version <= VERSION).cloned().collect();
    list.push(next.clone());
    source.migrations = std::borrow::Cow::Owned(list);
    ensure!(
        source.locking && !source.ignore_missing && !source.no_tx,
        "generated upstream safeguards changed"
    );
    // Release only this owned lock before run_direct acquires its real lock.
    let before = f.proxy.stats();
    f.adapter.unlock().await?;
    ensure!(
        f.proxy.stats().statements - before.statements == 2,
        "generated explicit unlock inventory"
    );
    f.proxy.enable_query_trace(32);
    let before = f.proxy.stats();
    source.run_direct(&mut f.adapter).await?;
    let after = f.proxy.stats();
    let trace = f.proxy.query_trace();
    let q: Vec<_> = trace.iter().map(|s| s.trim().trim_end_matches(';').trim()).collect();
    ensure!(
        after.statements - before.statements == 12
            && q.len() == 12
            && f.proxy.query_trace_dropped() == 0
            && after.commits - before.commits == 1,
        "generated matching38 skip/ordinary39 count"
    );
    ensure!(q[0]=="SELECT current_database()" && q[1]=="SELECT pg_advisory_lock($1)" && q[2].starts_with("CREATE TABLE IF NOT EXISTS _sqlx_migrations") && q[3]=="SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version LIMIT 1" && q[4]=="SELECT version, checksum FROM _sqlx_migrations ORDER BY version" && q[5]=="BEGIN" && q[6]==SQL39 && q[7]==public_apply::metadata_insert_sql_for_tests().trim() && q[8]=="COMMIT" && q[9].starts_with("UPDATE _sqlx_migrations") && q[10]=="SELECT current_database()" && q[11]=="SELECT pg_advisory_unlock($1)","generated ordinary actual source/order; original38 must SKIP");
    let row: (String, bool, Vec<u8>, i64) = sqlx::query_as(
        "SELECT description,success,checksum,execution_time FROM _sqlx_migrations WHERE version=$1",
    )
    .bind(VERSION + 1)
    .fetch_one(&mut f.observer)
    .await?;
    ensure!(
        row.0 == next.description.as_ref()
            && row.1
            && row.2 == next.checksum.as_ref()
            && row.3 >= 0,
        "generated actual39 lineage"
    );
    Ok(())
}

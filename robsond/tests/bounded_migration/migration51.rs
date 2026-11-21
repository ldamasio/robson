//! Native M51-M54 controls, the S8-5 boundary snapshot schema (ADR-0056 step
//! 8p-1). The per-version healthy apply and migration 40's eight faults are
//! `migration46.rs`'s suite (`m51` to `m54`); here, the refusal of every
//! non-identical source before SQL, the official runner and the rollback
//! boundary, each on the image frozen at 54, and, in
//! `migration51_constraints.rs`, the schema's rules.
use super::*;

#[path = "migration51_constraints.rs"]
mod constraints;

const BOUNDARY: [i64; 4] = [VERSION51, VERSION52, VERSION53, VERSION54];

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migration51_to_54_refuse_a_non_identical_source_before_sql(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool, VERSION51).await?;
    for version in BOUNDARY {
        let migration = contract(version);
        let foreign = contract(if version == VERSION51 {
            VERSION52
        } else {
            VERSION51
        });
        for wrong in [
            Migration { no_tx: true, ..migration.clone() },
            Migration {
                checksum: Cow::Owned(vec![0; 48]),
                ..migration.clone()
            },
            Migration {
                sql: Cow::Borrowed("SELECT 1"),
                ..migration.clone()
            },
            Migration {
                description: Cow::Borrowed("other"),
                ..migration.clone()
            },
            Migration {
                migration_type: MigrationType::ReversibleUp,
                ..migration.clone()
            },
            Migration {
                sql: foreign.sql.clone(),
                checksum: foreign.checksum.clone(),
                ..migration.clone()
            },
        ] {
            let denied =
                f.adapter.apply(&wrong).await.err().context("unembedded S8-5 source applied")?;
            let expected = format!("migration{} contract mismatch", version % 100);
            ensure!(
                matches!(&denied, MigrateError::Source(source) if source.to_string() == expected),
                "M{}: {denied}",
                version % 100
            );
            ensure!(f.adapter.status() == (Status { phase: Phase::Preflight, failure: None }));
            f.assert_wire_prefix(0);
        }
    }
    for version in BOUNDARY {
        ensure!(!landed(&mut f.observer, version).await?);
    }
    f.adapter.unlock().await?;
    f.finish(None).await
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migrations51_to_54_official_runner_applies_each_once_in_order(pool: PgPool) -> Result<()> {
    let mut f = Fixture::new(&pool, VERSION51).await?;
    // run_direct takes the same advisory lock itself. The image is frozen at
    // 54, the one that carries S8-5, so later files never change this witness.
    let image = through(VERSION54);
    f.adapter.unlock().await?;
    f.proxy.enable_exact_query_trace(64);
    let before = f.proxy.stats().statements;
    let started = Instant::now();
    image.run_direct(&mut f.adapter).await?;
    let duration = started.elapsed();
    let trace: Vec<String> =
        f.proxy.query_trace().iter().map(|sql| sql.trim().to_owned()).collect();
    let statements = f.proxy.stats().statements - before;
    ensure!(
        statements == 35 && trace.len() == 35 && f.proxy.query_trace_dropped() == 0,
        "{trace:?}"
    );
    ensure!(trace[0] == "SELECT current_database()" && trace[1] == "SELECT pg_advisory_lock($1)");
    for (k, version) in BOUNDARY.into_iter().enumerate() {
        let slot = 5 + 7 * k;
        ensure!(
            trace[slot].starts_with("WITH old AS MATERIALIZED"),
            "install before M{}",
            version % 100
        );
        ensure!(trace[slot + 1] == "BEGIN");
        ensure!(
            trace[slot + 2].trim() == contract(version).sql.trim(),
            "M{} out of order",
            version % 100
        );
        ensure!(trace[slot + 3].trim() == public_apply::metadata_insert_sql_for_tests().trim());
        ensure!(trace[slot + 4] == "COMMIT");
        ensure!(trace[slot + 5].starts_with(ELAPSED), "{:?}", trace[slot + 5]);
        ensure!(trace[slot + 6].starts_with(RESTORE), "{:?}", trace[slot + 6]);
    }
    ensure!(
        trace[33] == "SELECT current_database()" && trace[34] == "SELECT pg_advisory_unlock($1)"
    );
    ensure!(f.adapter.status() == (Status { phase: Phase::Restored, failure: None }));
    let history: Vec<MigrationRow> = sqlx::query_as(HISTORY).fetch_all(&mut f.observer).await?;
    ensure!(history[..f.history.len()] == f.history[..], "prior history changed");
    let added = &history[f.history.len()..];
    ensure!(added.len() == BOUNDARY.len());
    for ((version, description, _, success, checksum, elapsed), expected) in
        added.iter().zip(BOUNDARY)
    {
        let migration = contract(expected);
        ensure!(
            (*version, description.as_str(), *success, checksum.as_slice())
                == (expected, migration.description.as_ref(), true, migration.checksum.as_ref())
        );
        ensure!(*elapsed >= 0);
        ensure!(landed(&mut f.observer, expected).await?);
        eprintln!("M{} official begin_to_commit_ns={elapsed}", expected % 100);
    }
    eprintln!("M51-M54 official runner physical_sql={statements} duration={duration:?}");
    // A second official run skips every applied version.
    let again = f.proxy.stats().statements;
    image.run_direct(&mut f.adapter).await?;
    ensure!(f.proxy.stats().statements - again == 7, "a repeated run applied something");
    ensure!(
        sqlx::query_as::<_, MigrationRow>(HISTORY).fetch_all(&mut f.observer).await? == history
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires isolated PostgreSQL DATABASE_URL"]
async fn migrations51_to_54_older_images_refuse_with_version_missing(pool: PgPool) -> Result<()> {
    install_through(&pool, VERSION54).await?;
    let options = pool.connect_options();
    let mut observer = options.connect().await?;
    let history: Vec<MigrationRow> = sqlx::query_as(HISTORY).fetch_all(&mut observer).await?;
    let catalog: String = sqlx::query_scalar(CATALOG).fetch_one(&mut observer).await?;
    for version in SPOT_SCHEMA {
        ensure!(landed(&mut observer, version).await?);
    }
    // Each older image, from the one that ends at 50 (S8-4's) to the one
    // before 54, stops at the first applied version missing from its
    // embedded set, before any apply: the hook fails and the sync stops.
    for missing in BOUNDARY {
        let mut older = BoundedMigration::connect(options.as_ref()).await?;
        let refused = timeout(ACQUIRE_BOUND, image_through(missing - 1).run_direct(&mut older))
            .await
            .context("older image did not answer")?;
        ensure!(
            matches!(refused, Err(MigrateError::VersionMissing(v)) if v == missing),
            "image through M{} did not refuse with VersionMissing(M{}): {refused:?}",
            (missing - 1) % 100,
            missing % 100
        );
        ensure!(
            older.status() == (Status { phase: Phase::Preflight, failure: None }),
            "older image applied"
        );
        // SQLx returns before its unlock: only the raw drop releases the key.
        drop(older);
    }
    ensure!(sqlx::query_as::<_, MigrationRow>(HISTORY).fetch_all(&mut observer).await? == history);
    ensure!(sqlx::query_scalar::<_, String>(CATALOG).fetch_one(&mut observer).await? == catalog);
    // The image frozen at 54 skips every applied version and unlocks.
    let mut canonical = BoundedMigration::connect(options.as_ref()).await?;
    timeout(ACQUIRE_BOUND, image_through(VERSION54).run_direct(&mut canonical))
        .await
        .context("canonical image did not answer")??;
    ensure!(sqlx::query_as::<_, MigrationRow>(HISTORY).fetch_all(&mut observer).await? == history);
    eprintln!("M51-M54 rollback boundary: images through M50..M53 refused with VersionMissing; canonical skipped");
    Ok(())
}

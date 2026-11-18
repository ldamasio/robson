//! Cancellation cut plans and the original-apply canceller shared by the
//! precommit cases and the generated lifecycles.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum Cut {
    BeforeInstall,
    AfterInstall,
    BeforeBegin,
    AfterBegin,
    BeforeMetadata,
    AfterMetadata,
    BeforeDdl(u8),
    AfterDdl(u8),
    BeforeCommit,
}

pub(super) struct CutPlan {
    pub(super) points: Vec<(Stage, Edge, u64)>,
    pub(super) ddl_count: usize,
    pub(super) metadata: bool,
    pub(super) prefix: usize,
    pub(super) failure: Failure,
}

pub(super) fn plan(cut: Cut) -> Result<CutPlan> {
    let (stage, edge, ddl_count, metadata) = match cut {
        Cut::BeforeDdl(n) | Cut::AfterDdl(n) => {
            ensure!((1..=52).contains(&n), "DDL cut outside original52");
            match cut {
                Cut::BeforeDdl(_) => (Stage::Ddl(n), Edge::BeforeCall, usize::from(n - 1), false),
                _ => (Stage::Ddl(n), Edge::AfterReturn, usize::from(n), false),
            }
        },
        Cut::BeforeInstall => (Stage::Install, Edge::BeforeCall, 0, false),
        Cut::AfterInstall => (Stage::Install, Edge::AfterReturn, 0, false),
        Cut::BeforeBegin => (Stage::Begin, Edge::BeforeCall, 0, false),
        Cut::AfterBegin => (Stage::Begin, Edge::AfterReturn, 0, false),
        Cut::BeforeMetadata => (Stage::Metadata, Edge::BeforeCall, 52, false),
        Cut::AfterMetadata => (Stage::Metadata, Edge::AfterReturn, 52, true),
        Cut::BeforeCommit => (Stage::Commit, Edge::BeforeCall, 52, true),
    };
    let mut points = Vec::with_capacity(4);
    for milestone in [16_u8, 32, 48] {
        // Strict stage ordering, not just received-count ordering. BeforeDdl17
        // follows AfterDdl16 with no intervening SQL: final window is then0.
        if matches!(stage, Stage::Install | Stage::Begin)
            || matches!(stage, Stage::Ddl(n) if milestone >= n)
        {
            continue;
        }
        points.push((Stage::Ddl(milestone), Edge::AfterReturn, u64::from(milestone) + 2));
    }
    let prefix = match (stage, edge) {
        (Stage::Install, Edge::BeforeCall) => 0,
        (Stage::Install, Edge::AfterReturn) | (Stage::Begin, Edge::BeforeCall) => 1,
        _ => 2 + ddl_count + usize::from(metadata),
    };
    let failure = if matches!(stage, Stage::Install) {
        Failure {
            at: Phase::SetupStarted,
            outcome: Outcome::ApplyNotStarted,
        }
    } else {
        Failure {
            at: Phase::DelegatedApply,
            outcome: Outcome::MayHaveCommitted,
        }
    };
    points.push((stage, edge, u64::try_from(prefix)?));
    Ok(CutPlan {
        points,
        ddl_count,
        metadata,
        prefix,
        failure,
    })
}

pub(super) async fn cancel_original(f: &mut Fixture, cut: Cut) -> Result<()> {
    let plan = plan(cut)?;
    let points = &plan.points;
    let expected_prefix = plan.prefix;
    let slot = f.adapter.gate.clone();
    let first = stages::arm(&slot, points[0].0, points[0].1)?;
    let baseline = f.baseline;
    let proxy = &f.proxy;
    let controller = async {
        let mut current = first;
        let mut windows = Vec::with_capacity(4);
        for n in 0..points.len() {
            current.0.await.map_err(|_| anyhow::anyhow!("precommit stage hit missing"))?;
            windows.push(Window {
                text: proxy.query_trace(),
                dropped: proxy.query_trace_dropped(),
                statements: proxy.stats().statements - baseline.statements,
            });
            if n + 1 == points.len() {
                // Hold the release sender until AFTER the original owner drops.
                return Ok::<_, anyhow::Error>((windows, current.1));
            }
            let next = stages::arm(&slot, points[n + 1].0, points[n + 1].1)?;
            proxy.enable_query_trace(32);
            current.1.send(()).map_err(|_| anyhow::anyhow!("precommit release lost"))?;
            current = next;
        }
        anyhow::bail!("precommit final stage missing")
    };
    let started = Instant::now();
    let (primary, observed) = {
        let mut producer = f.adapter.apply(migration());
        tokio::pin!(controller);
        let selected = tokio::select! {
            result = &mut producer => (Some(result), None),
            observed = &mut controller => (None, Some(observed)),
        };
        // This is the original boxed Migrate future, not a pinned reference.
        // Its borrowed Transaction drops before OwnedApply drops the raw socket.
        drop(producer);
        selected
    };
    let duration = started.elapsed();
    let status = f.adapter.status();
    let stats = f.proxy.stats();
    // Exercise every entry before cleanup drops the entire retired adapter.
    let refused = refuses_after_retirement(&mut f.adapter).await;
    let after_refusals = f.proxy.stats();
    eprintln!("PRECOMMIT_CUT cut={cut:?} primary_returned={} reached={} duration={duration:?} status={status:?} statements={} commits={}",
        primary.is_some(), observed.as_ref().is_some_and(|r| r.is_ok()),
        stats.statements - baseline.statements, stats.commits - baseline.commits);
    ensure!(
        primary.is_none(),
        "precommit producer returned before original-owner cancellation"
    );
    let (windows, release) = observed.context("precommit gate not observed")??;
    drop(release);
    ensure!(duration < APPLY_BOUND, "precommit cut missed global200");
    ensure!(
        status.phase == Phase::Retired && status.failure == Some(plan.failure),
        "precommit runtime outcome"
    );
    ensure!(refused && after_refusals == stats, "precommit retired entry sent SQL or39");
    ensure!(
        stats.statements - baseline.statements == u64::try_from(expected_prefix)?
            && stats.commits == baseline.commits,
        "cancellation exact prefix and zero COMMIT"
    );
    ensure!(
        stats.commit_acks_dropped == baseline.commit_acks_dropped
            && f.proxy.autocommit_acks_dropped() == 0,
        "precommit used a transport ACK fault"
    );
    ensure!(windows.len() == points.len(), "cancellation trace window count");
    let mut received = Vec::with_capacity(expected_prefix);
    let mut previous = 0;
    for (n, window) in windows.iter().enumerate() {
        let expected_window = usize::try_from(points[n].2 - previous)?;
        ensure!(
            window.dropped == 0
                && window.statements == points[n].2
                && window.text.len() == expected_window
                && expected_window <= 18,
            "cancellation received window incomplete"
        );
        received.extend(window.text.iter().map(|s| s.trim().trim_end_matches(';').trim()));
        previous = points[n].2;
    }
    ensure!(received.len() == expected_prefix, "cancellation received prefix length");
    let chunks = fixed52::chunks(migration())?;
    let expected: Vec<_> = chunks[..plan.ddl_count]
        .iter()
        .map(|s| s.trim().trim_end_matches(';').trim())
        .collect();
    if plan.ddl_count > 0 {
        ensure!(
            received[2..2 + plan.ddl_count] == expected,
            "cancellation original DDL prefix order or bytes"
        );
    }
    if expected_prefix > 0 {
        ensure!(
            received[0].starts_with("WITH old AS MATERIALIZED"),
            "cancellation setup inventory"
        );
    }
    if expected_prefix > 1 {
        ensure!(received[1] == "BEGIN", "cancellation transaction inventory");
    }
    if plan.metadata {
        ensure!(
            received.last().is_some_and(|sql| sql.contains("INSERT INTO _sqlx_migrations")),
            "precommit metadata INSERT missing"
        );
    } else {
        ensure!(
            received.len() == expected_prefix.min(2) + plan.ddl_count,
            "cancellation unexpectedly reached metadata"
        );
    }
    Ok(())
}

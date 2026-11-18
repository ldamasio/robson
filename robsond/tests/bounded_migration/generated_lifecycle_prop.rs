//! Property-based native lifecycles (guardrail 1.2): proptest draws 16
//! schedules from a fixed master seed; each runs on real PostgreSQL in its own
//! SQLx-macro-owned database and is checked by the same park/validate
//! observers as the fixed controls. No schema reset and no native shrinking:
//! sqlx deletes a test database only on success, so a failing case keeps its
//! database and evidence.
use super::*;

#[path = "generated_lifecycle_prop_space.rs"]
mod space;
use space::{generated_schedule, Round, GENERATED_CASES, MASTER_SEED};

fn resolve_present(model: Model, expected: bool, present: bool) -> Result<Model> {
    ensure!(present == expected, "generated property model/durable-state disagreement");
    ensure!(
        model == Model::Unresolved || (model == Model::Applied && present),
        "generated property observation used invalid model transition"
    );
    Ok(if present {
        Model::Applied
    } else {
        Model::Unapplied
    })
}

async fn run_generated(pool: PgPool, index: usize) -> Result<()> {
    ensure!(index < GENERATED_CASES, "generated index outside 0..16");
    let s = generated_schedule(index);
    eprintln!(
        "GENERATED_PROP_CASE index={index} master_seed={:?} schedule={s:?}",
        std::str::from_utf8(&MASTER_SEED)
    );
    let initial = tokio::time::timeout(ACQUIRE_BOUND, Fixture::new(&pool))
        .await
        .context("property fixture5s")
        .and_then(|r| r);
    let mut active = Some(initial.context("generated property initialization")?);
    let mut parked = None;
    let mut retiring = None;
    // Set before reconnect consumes the parked predecessor and cleared only once
    // the new owner is in `active`: an interrupted reconnect leaves raw
    // connections whose retirement is unproved.
    let mut reconnecting = false;
    let lost_reply = s.rounds.iter().any(|r| matches!(r, Round::LostReply));
    let label = 0x9e00_0000_u64 + index as u64;
    let primary = tokio::time::timeout(ACQUIRE_BOUND, async {
        let f = active.as_mut().context("property fixture absent")?;
        let mut oracle = native::prepare_with(f, s.deferred_fixture, lost_reply).await?;
        let mut model = Model::Unapplied;
        let mut committed = false;
        for (i, round) in s.rounds.iter().copied().enumerate() {
            let f = active.as_mut().context("property owner absent")?;
            ensure!(model != Model::Unresolved, "unresolved absence cannot authorize next operation");
            match round {
                Round::Ordinary39 => {
                    ensure!(model == Model::Applied, "ordinary39 only after observed application");
                    engine::ordinary(f).await?;
                },
                Round::Cancel(c) => cancel_original(f, c).await?,
                Round::Normal => engine::attempt_with(f, Profile::Normal, s.milestones, label).await?,
                Round::LostReply => engine::attempt_with(f, Profile::LostReply, s.milestones, label).await?,
                Round::Deferred => engine::attempt_with(f, Profile::Deferred, s.milestones, label).await?,
            }
            committed |= matches!(round, Round::Normal | Round::LostReply);
            model = if matches!(round, Round::Normal | Round::Ordinary39) { Model::Applied } else { Model::Unresolved };
            let f = active.take().context("property retiring fixture absent")?;
            retiring = Some(f.pid);
            parked = Some(native::park(f).await?);
            retiring = None;
            let p = parked.as_ref().context("property observation absent")?;
            native::validate(p, &mut oracle, matches!(round, Round::Ordinary39))?;
            model = resolve_present(model, committed, p.metadata.is_some())?;
            eprintln!("GENERATED_PROP_ROUND index={index} round={i} op={round:?} model={model:?} pid={} observed_after_native_retirement=true", p.pid);
            if i + 1 < s.rounds.len() {
                let p = parked.take().context("property reconnect requires closed predecessor")?;
                reconnecting = true;
                let (f, old_pid) = native::reconnect(p, &pool).await?;
                active = Some(f);
                reconnecting = false;
                native::configure_reconnect(active.as_mut().context("property new fixture absent")?, old_pid).await?;
            }
        }
        let expected = if committed { Model::Applied } else { Model::Unapplied };
        ensure!(model == expected, "independent final generated property invariant");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("generated property primary5s")
    .and_then(|r| r);
    let end = tokio::time::Instant::now() + ACQUIRE_BOUND;
    let interrupted = match retiring.take() {
        None => Ok(()),
        Some(pid) if pid <= 0 => Err(anyhow::anyhow!(
            "property interrupted park unknown PID; native retirement unproved"
        )),
        Some(pid) => tokio::time::timeout_at(end, async {
            let mut c = pool.acquire().await?.detach();
            for n in 1..=60 {
                if sqlx::query_scalar::<_, bool>(native::GONE).bind(pid).fetch_one(&mut c).await? {
                    return Ok::<_, anyhow::Error>(());
                }
                if n < 60 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            anyhow::bail!("property interrupted park PID/locks not retired")
        })
        .await
        .context("property interrupted park retirement5s")
        .and_then(|r| r),
    };
    let cleanup =
        tokio::time::timeout_at(end, native::dispose_resources(active.take(), parked.take()))
            .await
            .context("property cleanup5s")
            .and_then(|r| r)
            .and(interrupted)
            .and(if reconnecting {
                Err(anyhow::anyhow!(
                    "property reconnect interrupted; new owner retirement unproved"
                ))
            } else {
                Ok(())
            });
    let closed = tokio::time::timeout_at(end, pool.close())
        .await
        .context("property pool close5s");
    eprintln!(
        "GENERATED_PROP_RESULT index={index} schedule={s:?} primary={} cleanup={} pool={}",
        cause(&primary),
        cause(&cleanup),
        cause(&closed)
    );
    match (primary, cleanup, closed) {
        (Ok(()), Ok(()), Ok(())) => Ok(()),
        (p, c, l) => Err(anyhow::anyhow!(
            "generated index={index} primary={} cleanup={} pool={}; the SQLx-owned database is retained",
            cause(&p),
            cause(&c),
            cause(&l)
        )),
    }
}

macro_rules! generated {
    ($($name:ident => $index:literal);+ $(;)?) => {$(
        #[sqlx::test(migrations = false)]
        #[ignore = "requires DATABASE_URL"]
        async fn $name(pool: PgPool) -> Result<()> {
            run_generated(pool, $index).await
        }
    )+};
}
// One SQLx-owned database per generated schedule; exact selectors run in order.
generated! {
    generated_schedule_00 => 0;
    generated_schedule_01 => 1;
    generated_schedule_02 => 2;
    generated_schedule_03 => 3;
    generated_schedule_04 => 4;
    generated_schedule_05 => 5;
    generated_schedule_06 => 6;
    generated_schedule_07 => 7;
    generated_schedule_08 => 8;
    generated_schedule_09 => 9;
    generated_schedule_10 => 10;
    generated_schedule_11 => 11;
    generated_schedule_12 => 12;
    generated_schedule_13 => 13;
    generated_schedule_14 => 14;
    generated_schedule_15 => 15;
}

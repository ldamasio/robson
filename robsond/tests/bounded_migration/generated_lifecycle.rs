//! Fixed generated schedules: observations resolve uncertainty, never retries.
use super::*;
#[path = "generated_lifecycle_engine.rs"]
mod engine;
#[path = "generated_lifecycle_native.rs"]
mod native;

#[derive(Clone, Copy, Debug)]
enum Profile {
    Normal,
    Cancel,
    Deferred,
    LostReply,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Model {
    Unapplied,
    Applied,
    Unresolved,
}
#[derive(Clone, Copy, Debug)]
struct Seed {
    id: u64,
    profile: Profile,
    cut: Cut,
}

// This table is independent of adapter status/phase and received SQL. Its
// observation argument is derived only AFTER native death + public lock.
fn resolve(model: Model, profile: Profile, second: bool, present: bool) -> Result<Model> {
    let expected = match (profile, second) {
        (Profile::Cancel | Profile::Deferred, false) | (Profile::Deferred, true) => false,
        _ => true,
    };
    ensure!(present == expected, "generated model/durable-state disagreement");
    ensure!(
        model == Model::Unresolved || (model == Model::Applied && present),
        "generated observation used invalid model transition"
    );
    Ok(if present {
        Model::Applied
    } else {
        Model::Unapplied
    })
}

async fn run(pool: PgPool, seed: Seed) -> Result<()> {
    let initial = Fixture::new(&pool).await; // Exactly once; fixed IDs.
    let mut active = match initial {
        Ok(f) => Some(f),
        Err(e) => {
            let end = tokio::time::Instant::now() + ACQUIRE_BOUND;
            let cleanup = native::dispose(None, None, pool, end).await;
            return Err(e.context(format!("generated initialization cleanup={}", cause(&cleanup))));
        },
    };
    let mut parked = None;
    // Outside the cancellable primary future: a park cancelled by the 5 s bound
    // must still have its native retirement proved before cleanup succeeds.
    let mut retiring = None;
    let primary = tokio::time::timeout(ACQUIRE_BOUND, async {
        let f = active.as_mut().context("initial fixture absent")?;
        let mut oracle = native::prepare(f, seed.profile).await?;
        let mut model = Model::Unapplied;
        for round in 0..2 {
            let f = active.as_mut().context("generated owner absent")?;
            ensure!(model != Model::Unresolved, "unresolved absence cannot authorize next operation");
            if model == Model::Applied {
                engine::ordinary(f).await?; // Original38 must SKIP, actual39 runs.
            } else if matches!(seed.profile, Profile::Cancel) && round == 0 {
                cancel_original(f, seed.cut).await?; // SAME owning Box drop.
            } else {
                engine::attempt(f, seed.profile, seed.id).await?;
            }
            // Even a returned success is checked independently below.
            model = if matches!(seed.profile, Profile::Normal) || round == 1 && !matches!(seed.profile, Profile::Deferred) {
                Model::Applied
            } else { Model::Unresolved };
            let f = active.take().context("retiring fixture absent")?;
            retiring = Some(f.pid);
            parked = Some(native::park(f).await?);
            retiring = None;
            let p = parked.as_ref().context("serialized observation absent")?;
            native::validate(p, &mut oracle, round == 1 && matches!(seed.profile, Profile::Normal | Profile::LostReply))?;
            model = resolve(model, seed.profile, round == 1, p.metadata.is_some())?;
            eprintln!("GENERATED_NATIVE seed={:016x} profile={:?} round={round} model={model:?} pid={} observed_after_native_retirement=true",seed.id,seed.profile,p.pid);
            if round == 0 {
                let p = parked.take().context("reconnect requires closed predecessor")?;
                let (f, old_pid) = native::reconnect(p, &pool).await?;
                active = Some(f);
                native::configure_reconnect(active.as_mut().context("new fixture absent")?, old_pid).await?;
            }
        }
        let expected = if matches!(seed.profile, Profile::Deferred) { Model::Unapplied } else { Model::Applied };
        ensure!(model == expected, "independent final generated invariant");
        Ok::<_, anyhow::Error>(())
    }).await.context("generated primary5s").and_then(|r| r);
    // Always drop actual ownership and finish native observations before errors
    // propagate; no assertion above can bypass this cleanup branch.
    let end = tokio::time::Instant::now() + ACQUIRE_BOUND;
    let interrupted = match retiring.take() {
        None => Ok(()),
        // Never issue GONE for an unknown PID.
        Some(pid) if pid <= 0 => Err(anyhow::anyhow!(
            "generated interrupted park unknown PID; native retirement unproved"
        )),
        Some(pid) => tokio::time::timeout_at(end, async {
            // Raw connection: a cancelled check never returns it to the pool.
            let mut c = pool.acquire().await?.detach();
            for n in 1..=60 {
                if sqlx::query_scalar::<_, bool>(native::GONE).bind(pid).fetch_one(&mut c).await? {
                    return Ok::<_, anyhow::Error>(());
                }
                if n < 60 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            anyhow::bail!("generated interrupted park PID/locks not retired")
        })
        .await
        .context("generated interrupted park retirement5s")
        .and_then(|r| r),
    };
    // One absolute deadline covers interrupted retirement, disposal and pool close.
    let cleanup = native::dispose(active.take(), parked.take(), pool, end).await.and(interrupted);
    eprintln!(
        "GENERATED_RESULT seed={:016x} profile={:?} primary={} cleanup={}",
        seed.id,
        seed.profile,
        cause(&primary),
        cause(&cleanup)
    );
    match (primary, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(p), c) => Err(p.context(format!("generated cleanup={}", cause(&c)))),
        (Ok(()), Err(c)) => Err(c.context("generated primary=ok")),
    }
}

macro_rules! seeds {
    ($($name:ident => ($id:literal,$profile:ident,$cut:expr));+ $(;)?) => {$ (
        #[sqlx::test(migrations = false)]
        #[ignore = "requires DATABASE_URL"]
        async fn $name(pool: PgPool) -> Result<()> {
            run(pool,Seed { id:$id, profile:Profile::$profile,cut:$cut }).await
        }
    )+};
}
// IDs are explicit replay seeds, not entropy from the database or wall clock.
seeds! {
    native_seed_0001 => (0x8a410001,Normal,Cut::BeforeCommit);
    native_seed_0002 => (0x8a410002,Normal,Cut::BeforeCommit);
    native_seed_0003 => (0x8a410003,Normal,Cut::BeforeCommit);
    native_seed_0004 => (0x8a410004,Normal,Cut::BeforeCommit);
    native_seed_0005 => (0x8a410005,Normal,Cut::BeforeCommit);
    native_seed_0006 => (0x8a410006,Normal,Cut::BeforeCommit);
    native_seed_0007 => (0x8a410007,Cancel,Cut::BeforeDdl(1));
    native_seed_0008 => (0x8a410008,Cancel,Cut::AfterDdl(13));
    native_seed_0009 => (0x8a410009,Cancel,Cut::BeforeDdl(27));
    native_seed_0010 => (0x8a41000a,Cancel,Cut::AfterDdl(41));
    native_seed_0011 => (0x8a41000b,Cancel,Cut::BeforeMetadata);
    native_seed_0012 => (0x8a41000c,Cancel,Cut::BeforeCommit);
    native_seed_0013 => (0x8a41000d,Deferred,Cut::BeforeCommit);
    native_seed_0014 => (0x8a41000e,Deferred,Cut::BeforeCommit);
    native_seed_0015 => (0x8a41000f,Deferred,Cut::BeforeCommit);
    native_seed_0016 => (0x8a410010,Deferred,Cut::BeforeCommit);
    native_seed_0017 => (0x8a410011,Deferred,Cut::BeforeCommit);
    native_seed_0018 => (0x8a410012,Deferred,Cut::BeforeCommit);
    native_seed_0019 => (0x8a410013,LostReply,Cut::BeforeCommit);
    native_seed_0020 => (0x8a410014,LostReply,Cut::BeforeCommit);
    native_seed_0021 => (0x8a410015,LostReply,Cut::BeforeCommit);
    native_seed_0022 => (0x8a410016,LostReply,Cut::BeforeCommit);
    native_seed_0023 => (0x8a410017,LostReply,Cut::BeforeCommit);
    native_seed_0024 => (0x8a410018,LostReply,Cut::BeforeCommit);
}

// Property-based native lifecycles (guardrail 1.2): proptest generates the
// milestone triple, the per-round operations and the cancellation cut; every
// case runs on real PostgreSQL and checks the final native catalog, lineage and
// retirement invariants through the same park/validate observers as the fixed
// controls above, which remain unchanged.
#[path = "generated_lifecycle_prop.rs"]
mod generated_prop;

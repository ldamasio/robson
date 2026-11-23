//! PS-2b: the observed pause in memory, its acknowledgement by observation,
//! the keeper's schedule (generated, on paused time with a scripted writer)
//! and, on PG16, the stop that waits for the row to land and generated pause
//! histories through the real keeper and `persist`.
use std::sync::atomic::{AtomicUsize, Ordering};

use proptest::prelude::*;
use reqwest::header::HeaderMap;
use robson_exec::{clock_records::RawClockCapture, venue_clock::ClockDomain};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::*;

fn domain() -> ClockDomain {
    ClockDomain::new(Uuid::from_u128(1), 0, 0).unwrap()
}
fn tick(seconds: u64) -> TickCapture {
    TickCapture::new(domain(), i64::try_from(seconds).unwrap(), 0).unwrap()
}
fn capture(seconds: u64) -> RawClockCapture {
    let ns = i128::from(seconds) * 1_000_000_000;
    RawClockCapture {
        epoch: Uuid::from_u128(2),
        wall_seconds: 1_794_614_400 + i64::try_from(seconds).unwrap(),
        wall_nanoseconds: 0,
        domain: domain(),
        before_ns: ns,
        after_ns: ns,
    }
}
fn budget() -> SharedBudget {
    SharedBudget::new(tick(0), 0, None, false).unwrap()
}
/// One response head at `seconds`: `status`, headers, a head capture or none.
fn observe(
    budget: &SharedBudget,
    seconds: u64,
    status: u16,
    headers: &[(&'static str, &str)],
    head: bool,
) {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        map.insert(*name, value.parse().unwrap());
    }
    budget
        .0
        .lock()
        .unwrap()
        .heads
        .observe(status, &map, head.then(|| capture(seconds)));
}
fn refused(budget: &SharedBudget, seconds: u64) -> bool {
    matches!(
        budget.try_reserve_time(tick(seconds), tick(seconds + 10)),
        Err(BudgetError::Deferred)
    )
}

/// v16 §3.14: the 418/429 pause is in memory when `observe` returns, before
/// any await or write; it holds every send to its end even once its row has
/// landed and `pending` is cleared.
#[test]
fn the_pause_is_in_memory_before_any_await() {
    let budget = budget();
    observe(&budget, 100, 429, &[("retry-after", "30")], true);
    assert!(refused(&budget, 100) && budget.pending_pause());
    let observed = budget.pending_observation().expect("the observation to persist");
    assert_eq!((observed.status, observed.seconds), (429, 30));
    assert_eq!((observed.observed, observed.end), (tick(100), tick(130)));
    budget.acknowledge_pause(observed.id);
    assert!(!budget.pending_pause() && budget.pending_observation().is_none());
    assert!(refused(&budget, 129), "the in-memory end still holds");
    assert!(!refused(&budget, 130));
}

/// Review r5 (PLAN §7.1, the 418/429 page): a landed, acknowledged pause
/// still reports restriction level 3 while it blocks every send, and the
/// level clears at its end. The gauge is process-wide and other tests move
/// it, so the assertions run alone in a child process of this test binary.
#[test]
fn a_landed_pause_keeps_level_3_until_its_end() {
    const CHILD: &str = "ROBSON_TEST_PS2B_LEVEL_CHILD";
    let level = || {
        prometheus::gather()
            .iter()
            .find(|family| family.get_name() == "robsond_spot_response_restriction_level")
            .map(|family| family.get_metric()[0].get_gauge().get_value() as i64)
    };
    if std::env::var_os(CHILD).is_some() {
        let budget = budget();
        observe(&budget, 100, 429, &[("retry-after", "30")], true);
        budget.acknowledge_pause(budget.pending_observation().unwrap().id);
        assert!(!budget.pending_pause());
        for (seconds, expected) in [(101, 3), (129, 3), (130, 0)] {
            assert_eq!(refused(&budget, seconds), expected == 3, "send at {seconds}");
            assert_eq!(level(), Some(expected), "level at {seconds}");
        }
        return;
    }
    let (_, module) = module_path!().split_once("::").unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("{module}::a_landed_pause_keeps_level_3_until_its_end"),
        ])
        .args(["--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&child.stdout);
    assert!(child.status.success(), "{stdout}{}", String::from_utf8_lossy(&child.stderr));
    assert!(stdout.contains("1 passed"), "the child ran no test: {stdout}");
}

/// PLAN §6.5: a malformed usage header is a level-3 restriction for 120 s
/// (weight) or 10 s (order counts), honoured in memory, neither `unresolved`
/// nor an observation to persist.
#[test]
fn malformed_usage_restrictions_keep_their_header_durations() {
    let weight = budget();
    observe(&weight, 100, 200, &[("x-mbx-used-weight-1m", "many")], true);
    assert!(refused(&weight, 219) && !refused(&weight, 220));
    let orders = budget();
    observe(&orders, 100, 200, &[("x-mbx-order-count-10s", "many")], true);
    assert!(refused(&orders, 109) && !refused(&orders, 110));
    for budget in [weight, orders] {
        assert!(!budget.pending_pause() && budget.pending_observation().is_none());
    }
}

/// PLAN §6.5: `unresolved` (no head capture, an uninterpretable
/// `Retry-After`) holds every send and is never given to the keeper.
#[test]
fn an_unresolved_restriction_is_never_written() {
    for (headers, head) in [
        (&[("retry-after", "30")][..], false),
        (&[("retry-after", "soon")][..], true),
    ] {
        let budget = budget();
        observe(&budget, 100, 429, headers, head);
        assert!(budget.pending_pause() && refused(&budget, 10_000));
        assert!(budget.pending_observation().is_none(), "{headers:?} {head}");
    }
}

/// `unresolved` keeps the stop unclean whatever lands: no acknowledgement
/// clears it, so the next start waits its 120 s.
#[test]
fn an_unresolved_restriction_makes_the_stop_unclean() {
    let budget = budget();
    observe(&budget, 100, 429, &[("retry-after", "30")], true);
    let observed = budget.pending_observation().unwrap();
    observe(&budget, 101, 429, &[], false);
    budget.acknowledge_pause(observed.id);
    assert!(budget.pending_pause(), "an unresolved restriction was cleared");
}

/// The scripted writes: (observation, completion, landed, started after
/// the closing).
type WriteLog = Arc<Mutex<Vec<(Uuid, tokio::time::Instant, bool, bool)>>>;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// PLAN §8 PS-2b property: interleaved 418/429 observations and
    /// acknowledgements of any id seen so far; `pending` is false only after
    /// the latest observation (the one ending last) was acknowledged, with no
    /// 429 since; an older acknowledgement never clears a newer pause; no
    /// send before the latest end.
    #[test]
    fn property_an_older_acknowledgement_never_clears_a_newer_pause(
        events in prop::collection::vec((0u8..3, 1u64..300), 1..64),
    ) {
        let budget = budget();
        let mut now = 100u64;
        let (mut seen, mut latest, mut end, mut pending) = (Vec::new(), None, 0u64, false);
        for (kind, value) in events {
            now += value % 7;
            match kind {
                0 | 1 => {
                    observe(&budget, now, 429, &[("retry-after", &value.to_string())], true);
                    pending = true;
                    if now + value >= end {
                        end = now + value;
                        latest = budget.pending_observation().map(|o| o.id);
                        prop_assert_eq!(budget.pending_observation().map(|o| o.end), Some(tick(end)));
                    }
                    seen.extend(latest);
                },
                _ => if let Some(id) = seen.get(value as usize % seen.len().max(1)).copied() {
                    budget.acknowledge_pause(id);
                    pending &= Some(id) != latest;
                },
            }
            prop_assert_eq!(budget.pending_pause(), pending);
            if !pending && now < end {
                prop_assert!(refused(&budget, now));
            }
        }
    }

    /// Root's PS-2b condition: generated keeper schedules on paused time with
    /// a scripted writer whose server-side work outlives a dropped future,
    /// 418s and 429s, and the closing at any point (review r2). One write in
    /// flight at a time; after an unlanded write the next starts no sooner
    /// than 5 s later; every write is of the latest observation; at most one
    /// write starts after the closing; `pending` is false only while the
    /// modelled stored row covers the latest observation; with no closing
    /// before the settle, the latest observation lands.
    #[test]
    fn property_the_keeper_writes_one_at_a_time_and_retries_after_5s(
        script in prop::collection::vec((0u8..3, 0u64..4, 1u64..120, any::<bool>()), 1..24),
        close in any::<prop::sample::Index>(),
    ) {
        let close = close.index(script.len() + 1);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time().start_paused(true).build().unwrap();
        runtime.block_on(async {
            let budget = budget();
            let cancel = CancellationToken::new();
            let in_flight = Arc::new(AtomicUsize::new(0));
            let log: WriteLog = Arc::default();
            let stored: Arc<Mutex<Option<TickCapture>>> = Arc::default();
            let outcomes = Arc::new(Mutex::new(script.iter().map(|s| s.0).collect::<Vec<_>>()));
            let (flight, writes, results, watched, landed, closing) = (
                Arc::clone(&in_flight), Arc::clone(&log), Arc::clone(&outcomes), budget.clone(),
                Arc::clone(&stored), cancel.clone(),
            );
            let keeper = tokio::spawn(crate::venue_budget::pause_store::keep(budget.clone(), cancel.clone(), move |observed| {
                let (flight, writes, results, watched, landed, closing) = (
                    Arc::clone(&flight), Arc::clone(&writes), Arc::clone(&results), watched.clone(),
                    Arc::clone(&landed), closing.clone(),
                );
                async move {
                    assert_eq!(flight.fetch_add(1, Ordering::SeqCst), 0, "a second write in flight");
                    let latest = watched.pending_observation().map(|o| o.id);
                    assert_eq!(latest, Some(observed.id), "a write of an older observation");
                    let after_close = closing.is_cancelled();
                    let outcome = results.lock().unwrap().pop().unwrap_or(0);
                    // The server-side work ends 1 s later whatever the caller
                    // does; the writer returns only after it ended.
                    let (done, ended) = tokio::sync::oneshot::channel();
                    let (release, store) = (Arc::clone(&flight), Arc::clone(&landed));
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        if outcome == 0 {
                            let mut stored = store.lock().unwrap();
                            if stored.is_none_or(|end| observed.end.elapsed_since(&end).is_ok()) {
                                *stored = Some(observed.end);
                            }
                        }
                        release.fetch_sub(1, Ordering::SeqCst);
                        let _ = done.send(());
                    });
                    let _ = ended.await;
                    writes.lock().unwrap().push((observed.id, tokio::time::Instant::now(), outcome == 0, after_close));
                    [Ok(true), Ok(false), Err(BudgetError::Backend)][usize::from(outcome)]
                }
            }));
            // `pending` false only while the stored row ends no earlier than
            // the latest observation (the one ending last); vacuous before
            // the first observation.
            let covered = |latest: Option<TickCapture>| {
                let stored = *stored.lock().unwrap();
                latest.is_none()
                    || budget.pending_pause()
                    || latest.zip(stored).is_some_and(|(end, row)| row.elapsed_since(&end).is_ok())
            };
            let (mut now, mut latest) = (100u64, None);
            for (index, (_, gap, seconds, teapot)) in script.iter().enumerate() {
                if index == close {
                    budget.begin_closing();
                    cancel.cancel();
                }
                tokio::time::sleep(Duration::from_secs(*gap)).await;
                prop_assert!(covered(latest), "pending cleared without a covering row");
                now += gap;
                let status = if *teapot { 418 } else { 429 };
                observe(&budget, now, status, &[("retry-after", &seconds.to_string())], true);
                latest = budget.pending_observation().map(|o| o.end);
            }
            tokio::time::sleep(Duration::from_secs(600)).await;
            prop_assert!(covered(latest), "pending cleared without a covering row");
            budget.begin_closing();
            cancel.cancel();
            prop_assert!(keeper.await.unwrap());
            prop_assert!(covered(latest), "pending cleared without a covering row");
            let log = log.lock().unwrap();
            for pair in log.windows(2) {
                if !pair[0].2 {
                    prop_assert!(pair[1].1 - pair[0].1 >= Duration::from_secs(6), "retried within 5 s");
                }
            }
            prop_assert!(log.iter().filter(|write| write.3).count() <= 1, "writes after the closing: {log:?}");
            if close == script.len() {
                // Unscripted writes land: by the end the latest observation has.
                prop_assert!(log.last().is_some_and(|write| write.2), "{log:?}");
                prop_assert!(!budget.pending_pause(), "the latest observation never landed");
            }
            Ok(())
        })?;
    }
}

/// Review r1: once the stop cancels the keeper, it makes at most one more
/// write (the latest pending observation) and ends; an observation arriving
/// during that write is not written.
#[test]
fn the_keeper_writes_at_most_once_after_cancellation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(async {
        let budget = budget();
        let cancel = CancellationToken::new();
        let written: Arc<Mutex<Vec<Uuid>>> = Arc::default();
        let log = Arc::clone(&written);
        let keeper = tokio::spawn(crate::venue_budget::pause_store::keep(
            budget.clone(),
            cancel.clone(),
            move |observed| {
                let log = Arc::clone(&log);
                async move {
                    log.lock().unwrap().push(observed.id);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    Ok(true)
                }
            },
        ));
        observe(&budget, 100, 429, &[("retry-after", "30")], true);
        tokio::time::sleep(Duration::from_millis(500)).await;
        observe(&budget, 101, 429, &[("retry-after", "60")], true);
        let last = budget.pending_observation().unwrap().id;
        cancel.cancel();
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        observe(&budget, 102, 429, &[("retry-after", "90")], true);
        assert!(keeper.await.unwrap(), "the keeper failed");
        let written = written.lock().unwrap().clone();
        assert_eq!(written.len(), 2, "{written:?}");
        assert_eq!(
            written[1], last,
            "the final write is the latest observation at the cancellation"
        );
    });
}

/// A stop marker that only counts.
#[derive(Default)]
struct Marker(AtomicUsize);

#[async_trait::async_trait]
impl crate::stop::StopMarker for Marker {
    async fn mark_clean(&self) -> Result<(), String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// PLAN §6.5, §6.10: a 429 (`Retry-After: 120`) on a non-trading owner. With
/// the database down the keeper cannot land it: the stop is `Unlanded`, no
/// marker is written (the next start is unclean). With it up the keeper
/// lands the row, `pending` clears and the stop is clean; a restart in this
/// clock domain then honours the remaining pause.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_stop_is_unclean_until_the_pause_lands(pool: sqlx::PgPool) -> anyhow::Result<()> {
    use std::time::Duration;

    use robson_connectors::BinanceRestClient;
    use robson_exec::venue_clock::NativeClock;
    use robson_testkit::db_proxy::{DbProxy, ProxyMode};
    use tokio::time::Instant;

    use crate::stop::{run_stop_with_clock, MarkerWrite, StopResult, StopTimings};

    for database_up in [false, true] {
        let clock = Arc::new(NativeClock::initialize()?);
        let tenant = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO process_epochs (epoch_id, tenant_id, binary_version, clean_stop_at, \
             marker_version) VALUES ($1, $2, 'ps-2b-predecessor', now(), 2)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant)
        .execute(&pool)
        .await?;
        let start = crate::process_epoch::start_epoch(&pool, tenant, "ps-2b-test").await?;
        let options = pool.connect_options();
        let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
            .await?
            .next()
            .ok_or_else(|| anyhow::anyhow!("no upstream"))?;
        let proxy = DbProxy::start(upstream).await?;
        let wired = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect_lazy_with(
            (*options)
                .clone()
                .host("127.0.0.1")
                .port(proxy.addr().port())
                .ssl_mode(sqlx::postgres::PgSslMode::Disable),
        );
        if !database_up {
            proxy.set_mode(ProxyMode::Blackhole);
        }
        let now = clock.capture()?;
        let past = TickCapture::new(
            now.domain(),
            i64::try_from(now.nanoseconds() / 1_000_000_000)? - 61,
            0,
        )?;
        let budget =
            SharedBudget::non_trading(past, None, false).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let client = Arc::new(BinanceRestClient::new(String::new(), String::new()));
        let owner = crate::venue_clock_owner::ClockOwner::non_trading(
            Arc::clone(&clock),
            client,
            &wired,
            tenant,
            &start,
            &budget,
        )
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let closing = owner.spawn();
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "120".parse()?);
        let head = clock.wall_bracket(start.epoch.epoch_id)?;
        budget.0 .0.lock().unwrap().heads.observe(429, &headers, Some(head));
        let shared = budget.time_owner();
        anyhow::ensure!(shared.pending_pause(), "the pause is in memory at once");
        // The stop at once: the keeper's write lands during the stop's join
        // (the database up) or cannot (down).
        let backend = Arc::new(robson_exec::order_journal::StubJournalBackend::default());
        let journal = robson_exec::order_journal::OrderJournal::new(
            backend,
            &robson_exec::venue::VenueId::new("test:venue"),
        );
        let marker = Marker::default();
        let report = run_stop_with_clock(
            &journal,
            Instant::now(),
            || {},
            Some(&marker),
            Some(&*closing),
            StopTimings::default(),
        )
        .await;
        let expected = if database_up {
            (StopResult::Clean, MarkerWrite::Written)
        } else {
            (StopResult::Unlanded, MarkerWrite::NotAttempted)
        };
        anyhow::ensure!(
            (report.result, report.marker) == expected,
            "{:?}",
            (report.result, report.marker)
        );
        anyhow::ensure!(shared.pending_pause() != database_up, "landed: database {database_up}");
        let stored: i64 =
            sqlx::query_scalar("SELECT count(*) FROM venue_pauses WHERE tenant_id = $1")
                .bind(tenant)
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(stored == i64::from(database_up), "rows stored: {stored}");
        if database_up {
            // A clean restart (the epoch's own marker): PS-2a's bootstrap reads
            // the row in this clock domain; past the 60 s window the pause
            // still holds to its end.
            crate::process_epoch::mark_clean_stop(&pool, start.epoch)
                .await
                .map_err(|e| anyhow::anyhow!(e))?;
            let restart = crate::process_epoch::start_epoch(&pool, tenant, "ps-2b-test").await?;
            anyhow::ensure!(restart.predecessor.unclean_reason().is_none());
            let deadline = Instant::now() + Duration::from_secs(5);
            let next = crate::venue_budget::bootstrap::non_trading(
                &pool,
                tenant,
                &restart,
                &clock,
                deadline,
                &crate::venue_budget::Trace::default(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("restart: {e:?}"))?;
            let end = head.end_tick()?.nanoseconds() + 120 * 1_000_000_000;
            let at = |ns: u128| {
                TickCapture::new(
                    now.domain(),
                    (ns / 1_000_000_000) as i64,
                    (ns % 1_000_000_000) as i64,
                )
            };
            let window = next.start().map_err(|e| anyhow::anyhow!("{e:?}"))?.nanoseconds()
                + 61 * 1_000_000_000;
            anyhow::ensure!(window < end, "the pause outlasts the restart's window");
            anyhow::ensure!(matches!(
                next.try_reserve_time(at(window)?, at(window + 10_000_000_000)?),
                Err(BudgetError::Deferred)
            ));
            anyhow::ensure!(matches!(
                next.try_reserve_time(at(end - 1)?, at(end + 10_000_000_000)?),
                Err(BudgetError::Deferred)
            ));
            anyhow::ensure!(next.try_reserve_time(at(end)?, at(end + 10_000_000_000)?).is_ok());
        }
    }
    Ok(())
}

/// PLAN §8 PS-2b property (review r2), on PG16 with the real keeper and the
/// real `persist` through a proxy, on a multithreaded runtime: generated
/// histories of 418/429 heads (durations, wall steps), another process's
/// stored row (this clock domain or another, `recorded_at` ahead or behind),
/// keeper writes that land, lose their COMMIT ACK (confirmed, or the
/// confirmation meets a silent database and the write is replayed), or meet
/// a database that is down, and `begin_closing` at any point. Throughout and
/// at the end: no send before the latest in-memory end; `pending` false only
/// while the stored row covers the latest observation (judged here from the
/// row, independently of `persist`); every write covers or expires on its
/// fault, so the never-shorten trigger never fires; the keeper ends within
/// 5 s of the closing (its write in flight and at most one final write);
/// with no closing before the settle, the latest lands. Real time: gaps,
/// write deadlines and the 5 s retry are the production ones. Two fixed
/// histories pin the never-shorten conditions (d) and (e) through the keeper
/// (a longer stored row, in this clock domain or another, kept by a skipped
/// update), which the seeded cases need not reach.
mod histories {
    use std::{collections::VecDeque, future::Future};

    use anyhow::{anyhow, ensure};
    use robson_exec::venue_clock::NativeClock;
    use robson_testkit::db_proxy::{DbProxy, ProxyMode};
    use sqlx::{
        postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
        PgPool,
    };
    use tokio::time::Instant;

    use super::*;
    use crate::venue_budget::{pause_store, Trace};

    const SECOND: u128 = 1_000_000_000;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Fault {
        Pass,
        LostAck,
        LostAckUnconfirmed,
        Down,
    }

    #[derive(Clone, Debug)]
    struct History {
        /// Another process's row: (this clock domain, anchored seconds ago,
        /// duration seconds, `recorded_at` step seconds).
        stored: Option<(bool, u64, u64, i64)>,
        /// Heads: (418, `Retry-After` seconds, wall step seconds, gap ms).
        heads: Vec<(bool, u64, i64, u64)>,
        /// `begin_closing` before this head; after the settle at the end.
        close: usize,
        /// The keeper's writes in order, then `Pass`.
        faults: Vec<Fault>,
    }

    fn history() -> impl Strategy<Value = History> {
        let fault = prop_oneof![
            2 => Just(Fault::Pass),
            2 => Just(Fault::LostAck),
            1 => Just(Fault::LostAckUnconfirmed),
            1 => Just(Fault::Down),
        ];
        (
            prop::option::weighted(0.9, (any::<bool>(), 0u64..300, 1u64..1_200, -3_600i64..3_600)),
            prop::collection::vec((any::<bool>(), 1u64..600, -3_600i64..3_600, 0u64..800), 1..6),
            any::<prop::sample::Index>(),
            prop::collection::vec(fault, 0..3),
        )
            .prop_map(|(stored, heads, close, faults)| History {
                close: close.index(heads.len() + 1),
                stored,
                heads,
                faults,
            })
    }

    struct Rig {
        direct: PgPool,
        wired: PgPool,
        proxy: Arc<DbProxy>,
        clock: Arc<NativeClock>,
    }

    impl Rig {
        async fn new(options: PgConnectOptions) -> anyhow::Result<Self> {
            let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
                .await?
                .next()
                .ok_or_else(|| anyhow!("no upstream"))?;
            let proxy = Arc::new(DbProxy::start(upstream).await?);
            let wired = PgPoolOptions::new().max_connections(1).connect_lazy_with(
                options
                    .clone()
                    .host("127.0.0.1")
                    .port(proxy.addr().port())
                    .ssl_mode(PgSslMode::Disable),
            );
            let direct = PgPoolOptions::new().max_connections(2).connect_with(options).await?;
            Ok(Self {
                direct,
                wired,
                proxy,
                clock: Arc::new(NativeClock::initialize()?),
            })
        }

        fn tick(&self, nanoseconds: u128) -> anyhow::Result<TickCapture> {
            let domain = self.clock.capture()?.domain();
            let seconds = i64::try_from(nanoseconds / SECOND)?;
            Ok(TickCapture::new(domain, seconds, (nanoseconds % SECOND) as i64)?)
        }

        /// The landing rule, judged from the stored row alone.
        async fn covered(&self, tenant: Uuid, latest: &PauseObservation) -> anyhow::Result<bool> {
            let now = self.clock.capture()?.nanoseconds();
            let row: Option<(String, Uuid, i64, i32, String, String)> = sqlx::query_as(
                "SELECT pause_state, (restoration_domain).boot_uuid,
                        (restoration_domain).offset_seconds, (restoration_domain).offset_nanoseconds,
                        deadline_tick::text, duration_ns::text
                 FROM venue_pauses WHERE tenant_id = $1 AND venue_id = 'binance:spot'",
            )
            .bind(tenant)
            .fetch_optional(&self.direct)
            .await?;
            let Some((state, boot, seconds, nanos, deadline, duration)) = row else {
                return Ok(false);
            };
            let domain = ClockDomain::new(boot, seconds, u32::try_from(nanos)?)?;
            let (deadline, duration) = (deadline.parse::<u128>()?, duration.parse::<u128>()?);
            let end = latest.end.nanoseconds();
            Ok(state == "bounded"
                && if domain == latest.end.domain() {
                    deadline >= end
                } else {
                    duration >= end.saturating_sub(now)
                })
        }

        /// No send before the latest end; `pending` false only while covered.
        async fn check(
            &self,
            budget: &SharedBudget,
            tenant: Uuid,
            latest: Option<&PauseObservation>,
        ) -> anyhow::Result<()> {
            let Some(latest) = latest else {
                return Ok(());
            };
            let now = self.clock.capture()?;
            let pending = budget.pending_pause();
            if latest.end.elapsed_since(&now).is_ok_and(|left| !left.is_zero()) {
                let deadline = self.tick(now.nanoseconds() + 10 * SECOND)?;
                ensure!(budget.try_reserve_time(now, deadline).is_err(), "a send before the end");
            }
            ensure!(
                pending || self.covered(tenant, latest).await?,
                "pending cleared without a covering row"
            );
            Ok(())
        }

        async fn run(&self, history: History) -> anyhow::Result<()> {
            let (tenant, epoch) = (Uuid::now_v7(), Uuid::now_v7());
            self.proxy.set_mode(ProxyMode::Pass);
            let now = self.clock.capture()?.nanoseconds();
            if let Some((here, ago, seconds, step)) = history.stored {
                let domain = if here {
                    self.clock.capture()?.domain()
                } else {
                    ClockDomain::new(Uuid::now_v7(), 7, 0)?
                };
                sqlx::query(
                    "INSERT INTO venue_pauses(tenant_id,venue_id,status,code,pause_state,
                         observation_id,original_domain,observed_tick,duration_ns,
                         restoration_domain,restoration_anchor,deadline_tick,until_at,recorded_at)
                     SELECT $1,'binance:spot',429,NULL,'bounded',$2,d,$6::text::numeric,
                            $7::text::numeric,d,$6::text::numeric,
                            $6::text::numeric+$7::text::numeric,NULL,$8
                     FROM (SELECT ROW('boottime'::text,$3::uuid,$4::bigint,$5::integer)
                                  ::budget_clock_domain_v38 AS d) v",
                )
                .bind(tenant)
                .bind(Uuid::now_v7())
                .bind(domain.boot())
                .bind(domain.offset().0)
                .bind(i32::try_from(domain.offset().1)?)
                .bind(now.saturating_sub(u128::from(ago) * SECOND).to_string())
                .bind((u128::from(seconds) * SECOND).to_string())
                .bind(chrono::Utc::now() + chrono::Duration::seconds(step))
                .execute(&self.direct)
                .await?;
            }
            let budget = SharedBudget::new(self.tick(now - 100 * SECOND)?, 0, None, false)
                .map_err(|e| anyhow!("{e:?}"))?;
            let cancel = CancellationToken::new();
            let faults = Arc::new(Mutex::new(VecDeque::from(history.faults.clone())));
            type Written = (Fault, bool, Result<bool, BudgetError>);
            let writes: Arc<Mutex<Vec<Written>>> = Arc::default();
            let (wired, proxy, clock, script, log, closing) = (
                self.wired.clone(),
                Arc::clone(&self.proxy),
                Arc::clone(&self.clock),
                Arc::clone(&faults),
                Arc::clone(&writes),
                cancel.clone(),
            );
            let write = move |observed: PauseObservation| {
                let (wired, proxy, clock, script, log, closing) = (
                    wired.clone(),
                    Arc::clone(&proxy),
                    Arc::clone(&clock),
                    Arc::clone(&script),
                    Arc::clone(&log),
                    closing.clone(),
                );
                async move {
                    let fault = script.lock().unwrap().pop_front().unwrap_or(Fault::Pass);
                    let after_close = closing.is_cancelled();
                    proxy.set_mode(match fault {
                        Fault::Pass => ProxyMode::Pass,
                        Fault::LostAck | Fault::LostAckUnconfirmed => ProxyMode::DropCommitAck,
                        Fault::Down => ProxyMode::Blackhole,
                    });
                    let dropped = proxy.stats().commit_acks_dropped;
                    let trace = Trace::default();
                    let mut write = std::pin::pin!(pause_store::persist(
                        &wired, tenant, &observed, &clock, &trace
                    ));
                    // An unconfirmed lost ACK: the confirmation meets a
                    // database that answers nothing.
                    let landed = std::future::poll_fn(|cx| {
                        if fault == Fault::LostAckUnconfirmed
                            && proxy.stats().commit_acks_dropped > dropped
                        {
                            proxy.set_mode(ProxyMode::Blackhole);
                        }
                        write.as_mut().poll(cx)
                    })
                    .await;
                    proxy.set_mode(ProxyMode::Pass);
                    log.lock().unwrap().push((fault, after_close, landed));
                    landed
                }
            };
            let keeper = tokio::spawn(pause_store::keep(budget.clone(), cancel.clone(), write));
            let (mut latest, mut closed) = (None, None);
            for (index, &(teapot, seconds, step, gap)) in history.heads.iter().enumerate() {
                if index == history.close {
                    budget.begin_closing();
                    cancel.cancel();
                    closed = Some(Instant::now());
                }
                tokio::time::sleep(Duration::from_millis(gap)).await;
                self.check(&budget, tenant, latest.as_ref()).await?;
                let mut head = self.clock.wall_bracket(epoch)?;
                head.wall_seconds += step;
                let mut headers = HeaderMap::new();
                headers.insert("retry-after", seconds.to_string().parse()?);
                {
                    let mut state = budget.0.lock().unwrap();
                    state.heads.observe(if teapot { 418 } else { 429 }, &headers, Some(head));
                    latest = state.heads.pending_observation();
                }
                self.check(&budget, tenant, latest.as_ref()).await?;
            }
            if history.close == history.heads.len() {
                let settle =
                    Instant::now() + Duration::from_secs(8 * (history.faults.len() as u64 + 1));
                while budget.pending_pause() && Instant::now() < settle {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    self.check(&budget, tenant, latest.as_ref()).await?;
                }
                budget.begin_closing();
                cancel.cancel();
                closed = Some(Instant::now());
            }
            // After the closing: the write in flight, then at most one final
            // write, each bounded by 2 s, and no retry.
            let ended = closed.ok_or_else(|| anyhow!("never closed"))? + Duration::from_secs(5);
            let keeper = tokio::time::timeout_at(ended, keeper).await;
            ensure!(matches!(keeper, Ok(Ok(true))), "the keeper did not end cleanly: {keeper:?}");
            self.check(&budget, tenant, latest.as_ref()).await?;
            let writes = writes.lock().unwrap().clone();
            // One write may start after the cancellation although the keeper
            // read `closing` before it (this runtime is multithreaded); the
            // exact bound of one is the paused-time tests'.
            ensure!(writes.iter().filter(|w| w.1).count() <= 2, "after the closing: {writes:?}");
            for (fault, _, landed) in &writes {
                let expired = *landed == Err(BudgetError::Expired)
                    && matches!(fault, Fault::Down | Fault::LostAckUnconfirmed);
                ensure!(*landed == Ok(true) || expired, "refused or uncovered: {writes:?}");
            }
            if history.close == history.heads.len() {
                ensure!(!budget.pending_pause(), "the latest never landed: {writes:?}");
            }
            Ok(())
        }
    }

    /// Runs `body` with a rig of this test's database on its own
    /// multithreaded runtime, off the test's thread (each history is one
    /// tenant of the database), then closes the rig's pools.
    fn on_rig(
        pool: &PgPool,
        body: impl FnOnce(&tokio::runtime::Runtime, &Rig) -> anyhow::Result<()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let options = (*pool.connect_options()).clone();
        std::thread::spawn(move || -> anyhow::Result<()> {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()?;
            let rig = runtime.block_on(Rig::new(options))?;
            let result = body(&runtime, &rig);
            runtime.block_on(async move {
                rig.direct.close().await;
                rig.wired.close().await;
            });
            result
        })
        .join()
        .map_err(|_| anyhow!("the history thread panicked"))?
    }

    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn property_pause_histories_land_without_firing_the_trigger(
        pool: PgPool,
    ) -> anyhow::Result<()> {
        on_rig(&pool, |runtime, rig| {
            let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
                cases: 12,
                max_shrink_iters: 16,
                rng_seed: proptest::test_runner::RngSeed::Fixed(0x2b5e_ed01),
                failure_persistence: None,
                ..ProptestConfig::default()
            });
            runner
                .run(&history(), |history| {
                    runtime
                        .block_on(rig.run(history))
                        .map_err(|e| TestCaseError::fail(format!("{e:#}")))
                })
                .map_err(|e| anyhow!("{e}"))
        })
    }

    /// Condition (d) through the keeper: another process's row in this clock
    /// domain ends 900 s from now; a 60 s 429 never shortens it. The update
    /// is skipped, never refused by the trigger, and the kept row, covering
    /// the observation, lands the pause.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_longer_row_of_this_domain_lands_without_firing_the_trigger(
        pool: PgPool,
    ) -> anyhow::Result<()> {
        let history = History {
            stored: Some((true, 0, 900, 0)),
            heads: vec![(false, 60, 0, 0)],
            close: 1,
            faults: Vec::new(),
        };
        on_rig(&pool, move |runtime, rig| runtime.block_on(rig.run(history)))
    }

    /// Condition (e) through the keeper: another process's row in another
    /// clock domain holds 900 s; a 60 s 429 never shortens it. The update is
    /// skipped, never refused by the trigger, and the kept row, its full
    /// duration covering what remains of the observation, lands the pause.
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_longer_row_of_another_domain_lands_without_firing_the_trigger(
        pool: PgPool,
    ) -> anyhow::Result<()> {
        let history = History {
            stored: Some((false, 0, 900, 0)),
            heads: vec![(false, 60, 0, 0)],
            close: 1,
            faults: Vec::new(),
        };
        on_rig(&pool, move |runtime, rig| runtime.block_on(rig.run(history)))
    }
}

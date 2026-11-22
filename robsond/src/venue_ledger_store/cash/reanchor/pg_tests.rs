//! 8l-5a proofs on PostgreSQL (PLAN-r4 §8 row 8l-5): O1 and O2 through
//! 8k-3a's capture on two statements and two fresh connections, and every
//! refusal before the transaction: the receipt, its reading and its ACCOUNT-
//! only classification, the 5 s monotonic spacing, the 0.01 agreement, the
//! ±1 h month boundary, and a change of the version, the invalidity
//! generation, the clock or the ledger from before O1 to after O2.
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

use anyhow::{ensure, Context, Result};
use robson_exec::{
    clock_records::RawClockCapture,
    gate::IncidentReason,
    order_journal::{OrderJournal, StubJournalBackend},
    venue::VenueId,
    venue_clock::ClockDomain,
};
use robson_testkit::{daemon_process::database_url_through, db_proxy::DbProxy};
use rust_decimal_macros::dec;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

use super::*;
use crate::balance_accounting::GenerationCell;

const UID: &str = "4242";
/// Every receipt's u2: u = 150 ms.
const U2: i128 = 300_000_000;
/// Monotonic ticks are the wall's Unix nanoseconds plus this, in one domain.
const TICK_BASE: i128 = 1_000_000_000_000_000;

fn utc(text: &str) -> DateTime<Utc> {
    text.parse().expect("RFC 3339 fixture time")
}

/// `ms` milliseconds after 2026-11-10T12:00:00Z, far from a month boundary.
fn at(ms: i64) -> DateTime<Utc> {
    utc("2026-11-10T12:00:00Z") + TimeDelta::milliseconds(ms)
}

fn domain() -> ClockDomain {
    ClockDomain::new(Uuid::from_u128(7), 0, 0).expect("domain")
}

/// One balance-account tenant bound to UID 4242 on its live epoch, with an
/// empty Spot ledger and one clock sample.
struct Fixture {
    tenant: Uuid,
    scope: String,
    epoch: Uuid,
    sample: Uuid,
}

impl Fixture {
    async fn new(pool: &PgPool) -> Result<Self> {
        let (tenant, epoch, sample) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let scope = format!("ra-{tenant}");
        sqlx::query("INSERT INTO accounting_state (tenant_id,venue_id,budget_model) VALUES ($1,'binance:spot','net_from_start_non_expanding_v1')")
            .bind(tenant).execute(pool).await?;
        sqlx::query("INSERT INTO venue_accounts (venue_id,account_scope,tenant_id,uid,bound_at,cutover_id) VALUES ('binance:spot',$1,$2,$3,now(),$4)")
            .bind(&scope).bind(tenant).bind(UID).bind(Uuid::now_v7()).execute(pool).await?;
        sqlx::query("INSERT INTO process_epochs (epoch_id,tenant_id,binary_version) VALUES ($1,$2,'8l-5-fixture')")
            .bind(epoch).bind(tenant).execute(pool).await?;
        sqlx::query("INSERT INTO clock_samples (sample_id,tenant_id,venue_id,process_epoch,clock_domain,send_wall_seconds,send_wall_nanoseconds,send_tick_before,send_tick_after,receive_wall_seconds,receive_wall_nanoseconds,receive_tick_before,receive_tick_after,server_ms,theta2_ns,r_low_ns,r_high_ns,u2_ns,numeric_verdict,numeric_failures,algorithm_version,recorded_at) VALUES ($1,$2,'binance:spot',$3,ROW('boottime',$4::uuid,0,0)::budget_clock_domain_v38,1794614400,0,0,0,1794614400,0,0,0,0,-3589228800000000000,0,0,3589228800200000000,'invalid',1,1,now())")
            .bind(sample).bind(tenant).bind(epoch).bind(Uuid::now_v7())
            .execute(pool).await?;
        Ok(Self { tenant, scope, epoch, sample })
    }

    async fn version(&self, pool: &PgPool) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT version FROM accounting_state WHERE tenant_id=$1")
            .bind(self.tenant)
            .fetch_one(pool)
            .await?)
    }

    /// A clock capture of this process at wall `wall`, its ticks the wall's.
    fn capture(&self, wall: DateTime<Utc>) -> RawClockCapture {
        let tick = i128::from(wall.timestamp_nanos_opt().expect("ns")) + TICK_BASE;
        RawClockCapture {
            epoch: self.epoch,
            wall_seconds: wall.timestamp(),
            wall_nanoseconds: wall.timestamp_subsec_nanos(),
            domain: domain(),
            before_ns: tick,
            after_ns: tick + 1_000,
        }
    }

    /// An ACCOUNT receipt of UID 4242 sent at `sent` and received 300 ms
    /// later, admitted on this fixture's sample, with `balances` as listed.
    fn receipt(&self, sent: DateTime<Utc>, balances: serde_json::Value) -> AccountReceipt {
        let body = json!({
            "uid": 4242, "canTrade": true, "accountType": "fixture", "permissions": [],
            "balances": balances,
        });
        AccountReceipt {
            sample: Some((self.sample, U2)),
            send: self.capture(sent),
            receive: self.capture(sent + TimeDelta::milliseconds(300)),
            body: serde_json::from_value(body).expect("an ACCOUNT body"),
        }
    }

    /// The quote row `free` + `locked` beside the base's empty row.
    fn usdt_split(&self, sent: DateTime<Utc>, free: &str, locked: &str) -> AccountReceipt {
        self.receipt(
            sent,
            json!([
                { "asset": "BTC", "free": "0", "locked": "0" },
                { "asset": "USDT", "free": free, "locked": locked },
            ]),
        )
    }

    fn usdt(&self, sent: DateTime<Utc>, total: &str) -> AccountReceipt {
        self.usdt_split(sent, total, "0")
    }
}

/// What happens while a receipt is in flight.
#[derive(Debug, Clone, Copy)]
enum During {
    Nothing,
    /// A ledger write bumps the version.
    Bump,
    /// The gate records an occurrence: the invalidity generation moves.
    Occur,
    /// The venue never answers.
    Stall,
}

/// A scripted venue: each request takes the next receipt, after its event;
/// receipts whose send tick is listed fail their revalidation.
struct Fake {
    pool: PgPool,
    tenant: Uuid,
    gate: Arc<OperationGate>,
    script: Mutex<VecDeque<(Option<AccountReceipt>, During)>>,
    stale: Vec<i128>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ReanchorVenue for Fake {
    async fn account(&self) -> Option<AccountReceipt> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let (receipt, during) = self.script.lock().expect("script").pop_front()?;
        match during {
            During::Nothing => {},
            During::Bump => {
                sqlx::query("UPDATE accounting_state SET version = version + 1 WHERE tenant_id=$1")
                    .bind(self.tenant)
                    .execute(&self.pool)
                    .await
                    .expect("bump");
            },
            During::Occur => {
                self.gate.record(IncidentReason::VenueDiscrepancy);
            },
            During::Stall => std::future::pending::<()>().await,
        }
        receipt
    }

    fn revalidate(&self, receipt: &AccountReceipt) -> bool {
        !self.stale.contains(&receipt.send.before_ns)
    }
}

/// The re-anchor service of `f`'s daemon on `pool`, with its scripted venue.
fn service(
    pool: &PgPool,
    f: &Fixture,
    script: Script,
    stale: Vec<i128>,
) -> (Arc<CashReanchor>, Arc<Fake>) {
    let spot = VenueId::new(VenueId::BINANCE_SPOT);
    let gate = OrderJournal::new(Arc::new(StubJournalBackend::default()), &spot).gate().clone();
    let venue = Arc::new(Fake {
        pool: pool.clone(),
        tenant: f.tenant,
        gate: Arc::clone(&gate),
        script: Mutex::new(script.into()),
        stale,
        calls: AtomicUsize::new(0),
    });
    let accounting = BalanceAccounting {
        tenant_id: f.tenant,
        venue: spot,
        account_scope: f.scope.clone(),
        generation: GenerationCell::default(),
    };
    let reanchor = CashReanchor {
        pool: pool.clone(),
        accounting,
        gate,
        epoch: f.epoch,
        uid: UID.into(),
        pair: CommissionAssets {
            symbol: "BTCUSDT",
            base: "BTC",
            quote: "USDT",
        },
        venue: Arc::clone(&venue) as Arc<dyn ReanchorVenue>,
        read_only: false,
        flight: Default::default(),
    };
    (Arc::new(reanchor), venue)
}

type Script = Vec<(Option<AccountReceipt>, During)>;
/// One case of [`observe_all`]: its venue script and stale send ticks.
type Case = Box<dyn FnOnce(&Fixture) -> (Script, Vec<i128>)>;

/// O1 sent at `at(0)` and O2 at `at(6000)`, both reading `total`.
fn agreeing(f: &Fixture, total: &str) -> Script {
    vec![
        (Some(f.usdt(at(0), total)), During::Nothing),
        (Some(f.usdt(at(6_000), total)), During::Nothing),
    ]
}

/// Runs every case at once (each sleeps 5 s between O1 and O2), each on its
/// own fixture, and returns the observations in order.
async fn observe_all(
    pool: &PgPool,
    cases: Vec<Case>,
) -> Result<Vec<(Result<Evidence, ReanchorRefusal>, usize)>> {
    let mut running = Vec::new();
    for case in cases {
        let f = Fixture::new(pool).await?;
        let (script, stale) = case(&f);
        let (reanchor, venue) = service(pool, &f, script, stale);
        running.push(tokio::spawn(async move {
            let observed = reanchor.observe().await;
            (observed, venue.calls.load(Ordering::Relaxed))
        }));
    }
    let mut out = Vec::new();
    for handle in running {
        out.push(handle.await?);
    }
    Ok(out)
}

/// Connect options through a counting proxy.
async fn proxied(pool: &PgPool) -> Result<(DbProxy, PgPool)> {
    let options = pool.connect_options();
    let upstream = tokio::net::lookup_host((options.get_host(), options.get_port()))
        .await?
        .next()
        .context("host")?;
    let proxy = DbProxy::start(upstream).await?;
    let database = options.get_database().context("database")?;
    let url = database_url_through(&std::env::var("DATABASE_URL")?, proxy.addr(), database)?;
    let through = PgPoolOptions::new().max_connections(2).connect_lazy(&url)?;
    Ok((proxy, through))
}

/// O1 and O2 qualify under one capture: two autocommit statements on two
/// fresh connections and nothing else (PLAN-r4 §6.12's two pre-reads), `v₀`
/// and the marks read before O1, each observation's interval, `u`, sample,
/// epoch and total, the stored microsecond times. 8l-5a stops before the
/// transaction: nothing is written.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn o1_and_o2_qualify_on_two_statements(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    sqlx::query("UPDATE accounting_state SET version = 3 WHERE tenant_id=$1")
        .bind(f.tenant)
        .execute(&pool)
        .await?;
    let (proxy, through) = proxied(&pool).await?;
    // O1 and O2 for `observe`, then again for `run`.
    let script = [agreeing(&f, "1000.005"), agreeing(&f, "1000.005")].concat();
    let (reanchor, venue) = service(&through, &f, script, vec![]);
    proxy.enable_exact_query_trace(8);
    let start = proxy.stats();
    let evidence = reanchor.observe().await.map_err(|r| anyhow::anyhow!("{r:?}"))?;
    let end = proxy.stats();
    ensure!(
        (end.statements - start.statements, end.connections - start.connections) == (2, 2),
        "{start:?} {end:?}"
    );
    let trace = proxy.query_trace();
    ensure!(
        trace.len() == 2 && trace.iter().all(|s| s.contains("AS uid_bound")),
        "{trace:?}"
    );
    ensure!(venue.calls.load(Ordering::Relaxed) == 2);
    ensure!(evidence.version == 3 && evidence.marks == reanchor.gate.invalidity_marks());
    ensure!(evidence.requested.elapsed() >= Duration::from_secs(5));
    for (o, sent) in [(evidence.o1, at(0)), (evidence.o2, at(6_000))] {
        let u = TimeDelta::milliseconds(150);
        let expected = Observed {
            cut: sent - u,
            send_at: sent,
            receive_at: sent + TimeDelta::milliseconds(300),
            end: sent + TimeDelta::milliseconds(450),
        };
        ensure!(o.observed == expected, "{o:?}");
        ensure!(o.reading.u_ns == 150_000_000 && o.reading.sample_id == f.sample);
        ensure!(o.reading.epoch_id == f.epoch && o.total == dec!(1000.005));
    }
    let refused = reanchor.run("operator@example.test", "withdrew 100").await;
    ensure!(refused == Err(ReanchorRefusal::Uncommitted), "{refused:?}");
    ensure!(f.version(&pool).await? == 3);
    println!("REANCHOR_PRE_READS statements=2 connections=2 trace={}", trace.len());
    Ok(())
}

/// Each observation's own refusals, for O1 (O2 never requested) and for O2:
/// no receipt, another UID, no clock sample, a base the ACCOUNT-only check
/// does not find clean, and no single well-formed quote row.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_missing_unclean_or_malformed_receipt_refuses(pool: PgPool) -> Result<()> {
    type Bad = fn(&Fixture, DateTime<Utc>) -> Option<AccountReceipt>;
    let bad: [(Bad, ReanchorRefusal); 7] = [
        (|_, _| None, ReanchorRefusal::Venue),
        // No base row at all: the ACCOUNT-only classification is incomplete.
        (
            |f, sent| {
                Some(f.receipt(sent, json!([{ "asset": "USDT", "free": "1000", "locked": "0" }])))
            },
            ReanchorRefusal::Base,
        ),
        (
            |f, sent| {
                let mut receipt = f.usdt(sent, "1000");
                receipt.body = serde_json::from_value(json!({
                    "uid": 4243, "canTrade": true, "accountType": "fixture", "permissions": [],
                    "balances": [
                        { "asset": "BTC", "free": "0", "locked": "0" },
                        { "asset": "USDT", "free": "1000", "locked": "0" },
                    ],
                }))
                .ok()?;
                Some(receipt)
            },
            ReanchorRefusal::Unqualified(UnqualifiedReason::Scope),
        ),
        (
            |f, sent| Some(AccountReceipt { sample: None, ..f.usdt(sent, "1000") }),
            ReanchorRefusal::Unqualified(UnqualifiedReason::Clock),
        ),
        (
            |f, sent| {
                Some(f.receipt(
                    sent,
                    json!([
                        { "asset": "USDT", "free": "1000", "locked": "0" },
                        { "asset": "BTC", "free": "0.5", "locked": "0" },
                    ]),
                ))
            },
            ReanchorRefusal::Base,
        ),
        (
            |f, sent| {
                Some(f.receipt(
                    sent,
                    json!([
                        { "asset": "BTC", "free": "0", "locked": "0" },
                        { "asset": "USDT", "free": "600", "locked": "0" },
                        { "asset": "USDT", "free": "400", "locked": "0" },
                    ]),
                ))
            },
            ReanchorRefusal::Quote,
        ),
        (
            |f, sent| {
                Some(f.receipt(
                    sent,
                    json!([
                        { "asset": "BTC", "free": "0", "locked": "0" },
                        { "asset": "BNB", "free": "1", "locked": "0" },
                    ]),
                ))
            },
            ReanchorRefusal::Quote,
        ),
    ];
    let mut cases: Vec<Case> = Vec::new();
    for (make, _) in bad {
        cases.push(Box::new(move |f: &Fixture| (vec![(make(f, at(0)), During::Nothing)], vec![])));
        cases.push(Box::new(move |f: &Fixture| {
            let o1 = (Some(f.usdt(at(0), "1000")), During::Nothing);
            (vec![o1, (make(f, at(6_000)), During::Nothing)], vec![])
        }));
    }
    let outcomes = observe_all(&pool, cases).await?;
    for (n, (outcome, calls)) in outcomes.into_iter().enumerate() {
        let expected = bad[n / 2].1;
        ensure!(outcome == Err(expected), "case {n}: {outcome:?}");
        ensure!(calls == 1 + n % 2, "case {n}: {calls} requests");
    }
    Ok(())
}

/// O2 is sent at least 5 s after O1's receipt on the receipts' monotonic
/// ticks, in one clock domain: exactly 5 s passes, a nanosecond less or
/// another domain refuses `spacing`.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn o2_follows_o1_by_five_monotonic_seconds(pool: PgPool) -> Result<()> {
    let spaced = |gap: i128, other_domain: bool| {
        Box::new(move |f: &Fixture| {
            let o1 = f.usdt(at(0), "1000");
            let mut o2 = f.usdt(at(6_000), "1000");
            o2.send.before_ns = o1.receive.after_ns + gap;
            o2.send.after_ns = o2.send.before_ns + 1_000;
            if other_domain {
                o2.send.domain = ClockDomain::new(Uuid::from_u128(8), 0, 0).expect("domain");
            }
            (vec![(Some(o1), During::Nothing), (Some(o2), During::Nothing)], vec![])
        }) as Case
    };
    let cases = vec![
        spaced(5_000_000_000, false),
        spaced(4_999_999_999, false),
        spaced(5_000_000_000, true),
        spaced(-1, false),
    ];
    let outcomes: Vec<_> = observe_all(&pool, cases).await?.into_iter().map(|(o, _)| o).collect();
    ensure!(outcomes[0].is_ok(), "{:?}", outcomes[0]);
    for outcome in &outcomes[1..] {
        ensure!(*outcome == Err(ReanchorRefusal::Spacing), "{outcome:?}");
    }
    Ok(())
}

/// O1's and O2's totals agree within 0.01 (free + locked each): exactly
/// 0.01 apart passes, more refuses `disagree`.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn o1_and_o2_agree_within_a_cent(pool: PgPool) -> Result<()> {
    let pair = |o2: &'static str| {
        Box::new(move |f: &Fixture| {
            let o2 = f.usdt_split(at(6_000), o2, "0.5");
            (
                vec![
                    (Some(f.usdt(at(0), "1000")), During::Nothing),
                    (Some(o2), During::Nothing),
                ],
                vec![],
            )
        }) as Case
    };
    let cases = vec![
        pair("999.51"),
        pair("999.49"),
        pair("999.510001"),
        pair("999.489999"),
    ];
    let outcomes: Vec<_> = observe_all(&pool, cases).await?.into_iter().map(|(o, _)| o).collect();
    ensure!(outcomes[0].is_ok() && outcomes[1].is_ok(), "{outcomes:?}");
    ensure!(outcomes[0].as_ref().map(|e| e.o2.total) == Ok(dec!(1000.01)), "{outcomes:?}");
    ensure!(
        outcomes[2..].iter().all(|o| *o == Err(ReanchorRefusal::Disagree)),
        "{outcomes:?}"
    );
    Ok(())
}

/// No UTC month boundary within 1 h of [O1's cut, O2's end], both ends
/// included: a boundary exactly 1 h before O1's cut or after O2's end
/// refuses, one a microsecond further passes, one 30 min away refuses.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_month_boundary_within_an_hour_refuses(pool: PgPool) -> Result<()> {
    let b = utc("2026-12-01T00:00:00Z");
    let (u, us) = (TimeDelta::milliseconds(150), TimeDelta::microseconds(1));
    // O1 sent at `o1`, O2 6 s later; O1's cut is o1 - u, O2's end o1 + 6 s + 300 ms
    // + u.
    let span = TimeDelta::milliseconds(6_450);
    let starting = |o1: DateTime<Utc>| {
        Box::new(move |f: &Fixture| {
            let o2 = o1 + TimeDelta::seconds(6);
            let script = vec![
                (Some(f.usdt(o1, "1000")), During::Nothing),
                (Some(f.usdt(o2, "1000")), During::Nothing),
            ];
            (script, vec![])
        }) as Case
    };
    let hour = TimeDelta::hours(1);
    let cases = vec![
        starting(b + hour + u),
        starting(b + hour + u + us),
        starting(b - hour - span),
        starting(b - hour - span - us),
        starting(b + TimeDelta::minutes(30)),
        starting(b - TimeDelta::minutes(30) - span),
        starting(b - TimeDelta::seconds(3)),
    ];
    let outcomes: Vec<_> = observe_all(&pool, cases).await?.into_iter().map(|(o, _)| o).collect();
    let near = Err(ReanchorRefusal::NearBoundary);
    ensure!(outcomes[0] == near && outcomes[2] == near, "{outcomes:?}");
    ensure!(outcomes[1].is_ok() && outcomes[3].is_ok(), "{outcomes:?}");
    ensure!(outcomes[4..].iter().all(|o| *o == near), "{outcomes:?}");
    Ok(())
}

/// A change from before O1 to after O2 refuses through the capture: a
/// version bump or an occurrence while O2 is in flight, either receipt's
/// clock failing its revalidation after the post; a Spot ledger row before
/// O1 is not at rest and asks the venue nothing.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_change_from_before_o1_to_after_o2_refuses(pool: PgPool) -> Result<()> {
    let during = |event: During| {
        Box::new(move |f: &Fixture| {
            let mut script = agreeing(f, "1000");
            script[1].1 = event;
            (script, vec![])
        }) as Case
    };
    let stale = |which: usize| {
        Box::new(move |f: &Fixture| {
            let script = agreeing(f, "1000");
            let tick = script[which].0.as_ref().map_or(0, |r| r.send.before_ns);
            (script, vec![tick])
        }) as Case
    };
    let cases = vec![
        during(During::Bump),
        during(During::Occur),
        stale(0),
        stale(1),
    ];
    let outcomes: Vec<_> = observe_all(&pool, cases).await?.into_iter().map(|(o, _)| o).collect();
    let unqualified = |reason| Err(ReanchorRefusal::Unqualified(reason));
    ensure!(outcomes[0] == unqualified(UnqualifiedReason::VersionChanged), "{outcomes:?}");
    ensure!(outcomes[1] == unqualified(UnqualifiedReason::GenerationChanged), "{outcomes:?}");
    ensure!(outcomes[2] == unqualified(UnqualifiedReason::Clock), "{outcomes:?}");
    ensure!(outcomes[3] == unqualified(UnqualifiedReason::Clock), "{outcomes:?}");

    // An intent of the scope: the Spot ledger is not empty, so not at rest.
    let f = Fixture::new(&pool).await?;
    sqlx::query("INSERT INTO order_intents (intent_id,tenant_id,venue_id,account_scope,client_order_id,kind,params,created_at) VALUES ($1,$2,'binance:spot',$3,$4,'entry','{}',now())")
        .bind(Uuid::now_v7()).bind(f.tenant).bind(&f.scope).bind(Uuid::now_v7().simple().to_string())
        .execute(&pool).await?;
    let (reanchor, venue) = service(&pool, &f, agreeing(&f, "1000"), vec![]);
    let refused = reanchor.observe().await;
    ensure!(refused == unqualified(UnqualifiedReason::LedgerNotEmpty), "{refused:?}");
    ensure!(venue.calls.load(Ordering::Relaxed) == 0);
    Ok(())
}

/// Review r1: an ACCOUNT request the venue never answers is bounded: the
/// run refuses `venue` and frees the flight for the next one.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_stalled_account_request_refuses_and_frees_the_flight(pool: PgPool) -> Result<()> {
    let f = Fixture::new(&pool).await?;
    let mut script = agreeing(&f, "1000");
    script[0].1 = During::Stall;
    let (reanchor, venue) = service(&pool, &f, script, vec![]);
    let run = reanchor.run("operator@example.test", "withdrew 100");
    let refused = tokio::time::timeout(Duration::from_secs(15), run).await;
    ensure!(refused == Ok(Err(ReanchorRefusal::Venue)), "{refused:?}");
    ensure!(venue.calls.load(Ordering::Relaxed) == 1 && reanchor.flight.try_lock().is_ok());
    ensure!(f.version(&pool).await? == 0);
    Ok(())
}

/// The boundary test itself, both ends included.
#[test]
fn near_boundary_includes_both_ends() {
    let b = utc("2026-12-01T00:00:00Z");
    let (us, hour) = (TimeDelta::microseconds(1), TimeDelta::hours(1));
    // [cut, end] with the boundary exactly 1 h before the cut or after the end.
    assert!(near_boundary(b + hour, b + hour + us) && near_boundary(b - hour - us, b - hour));
    assert!(near_boundary(b, b) && near_boundary(b - hour, b + hour));
    assert!(!near_boundary(b + hour + us, b + TimeDelta::days(20)));
    assert!(!near_boundary(b - TimeDelta::days(20), b - hour - us));
    assert!(near_boundary(utc("2026-12-31T23:30:00Z"), utc("2026-12-31T23:30:00Z")));
    assert!(near_boundary(utc("2026-11-10T00:00:00Z"), utc("2027-02-10T00:00:00Z")));
    // Review r2: an unrepresentable edge refuses and never panics.
    let (min, max) = (DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC);
    assert!(near_boundary(min + hour, min + hour + us) && near_boundary(min, min));
    assert!(near_boundary(max - hour, max - hour + us) && near_boundary(max, max));
}

/// The endpoint's 409 body is the refusal as `{error, detail}`; its 200 body
/// is the result with every field.
#[test]
fn refusals_and_results_serialize_as_the_endpoint_answers() {
    let unqualified = ReanchorRefusal::Unqualified(UnqualifiedReason::VersionChanged);
    let value = serde_json::to_value(unqualified).expect("json");
    assert_eq!(value, json!({ "error": "unqualified", "detail": "version_changed" }));
    let value = serde_json::to_value(ReanchorRefusal::NearBoundary).expect("json");
    assert_eq!(value, json!({ "error": "near_boundary" }));
    let done = Reanchored {
        reanchor_id: Uuid::from_u128(1),
        mode: "attested",
        version: 4,
        delta: Some(dec!(-100)),
        withdrawal: Some(dec!(100)),
        contribution: Some(dec!(0)),
        flows: 1,
    };
    let decimal = |d: Decimal| serde_json::to_value(d).expect("json");
    assert_eq!(
        serde_json::to_value(&done).expect("json"),
        json!({
            "reanchor_id": Uuid::from_u128(1), "mode": "attested", "version": 4,
            "delta": decimal(dec!(-100)), "withdrawal": decimal(dec!(100)),
            "contribution": decimal(dec!(0)), "flows": 1,
        })
    );
}

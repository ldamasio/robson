#![cfg(all(test, feature = "crash-points"))]
//! PS-5b's read owner end to end on loopback (PLAN §8 PS-5): this module's
//! own fake Spot venue answers each read and checks each signature (SD12:
//! robsond's sources name no test venue of robson-testkit); a throttled
//! endless listener feeds the cap. No database: the TIME owner is built
//! offline, its budget past the grant window, and each test publishes its
//! own samples.
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

use proptest::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::{
    balance_reconciliation::ObservationInterval,
    process_epoch::{Epoch, Predecessor},
    venue_budget::state::SharedBudget,
};

const SECOND: u128 = 1_000_000_000;

struct Rig {
    reads: SpotReads,
    handle: ClockHandle,
    budget: SharedBudget,
    _owner: ClockOwner,
}

/// The read owner of a non-trading start whose budget began 61 s ago, on a
/// closed client bound to `url`, with no sample yet.
fn rig(url: String) -> Rig {
    let clock = Arc::new(NativeClock::initialize().unwrap());
    let now = clock.capture().unwrap();
    let start = now.nanoseconds() - 61 * SECOND;
    let (seconds, nanoseconds) = (start / SECOND, start % SECOND);
    let start = TickCapture::new(now.domain(), seconds as i64, nanoseconds as i64).unwrap();
    let budget = SharedBudget::non_trading(start, None, false).unwrap();
    let client = BinanceRestClient::with_harness_base_url(url, API_KEY.into(), API_SECRET.into());
    let client = Arc::new(client);
    client.close_unbudgeted_sends();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://offline@127.0.0.1:1/none")
        .unwrap();
    let start = EpochStart {
        epoch: Epoch {
            epoch_id: Uuid::from_u128(5),
            epoch_seq: 1,
        },
        predecessor_epoch_id: None,
        predecessor: Predecessor::None,
        unclean_generation: None,
        stamped_intents: 0,
        open_incidents: Vec::new(),
    };
    let tenant = Uuid::from_u128(4);
    let owner = ClockOwner::non_trading(clock, Arc::clone(&client), &pool, tenant, &start, &budget)
        .unwrap();
    let handle = owner.handle();
    Rig {
        reads: SpotReads::new(handle.clone(), client, budget.clone()),
        handle,
        budget: budget.time_owner(),
        _owner: owner,
    }
}

impl Rig {
    /// A fresh sample of this epoch, published at the current generation.
    fn publish(&self, id: u128) {
        let handle = &self.handle;
        let mut state = handle.state.lock().unwrap();
        let send = state.capture_native(&handle.clock, handle.epoch).unwrap();
        let receive = state.capture_native(&handle.clock, handle.epoch).unwrap();
        let received = receive.end_tick().unwrap();
        let server_ms =
            receive.wall_seconds * 1_000 + i64::from(receive.wall_nanoseconds / 1_000_000);
        let raw = RawClockSample {
            id: Uuid::from_u128(id),
            tenant: Uuid::from_u128(4),
            venue: VenueId::new("binance:spot"),
            send,
            receive,
            server_ms,
        };
        let sample = Published {
            id: raw.id,
            arithmetic: raw.arithmetic().unwrap(),
            u2: raw.numbers().unwrap().u2,
        };
        let generation = state.generation;
        state.publish(generation, sample, received).unwrap();
    }

    /// The rolling minute's weight (charged and reserved) and the handoffs.
    fn used(&self) -> (u64, u64) {
        loop {
            if let Some((weight, _)) = self.budget.usage(|| self.handle.clock.capture().ok()) {
                return (weight, self.budget.handoffs().unwrap());
            }
            std::thread::yield_now();
        }
    }
}

fn soon(ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(ms)
}

/// The test client's key and secret (the secret within one HMAC block).
const API_KEY: &str = "ps5b-loopback-key";
const API_SECRET: &str = "ps5b-loopback-secret";
/// Binance's documented bodies, as the closed set's own decode tests read
/// them (robson-connectors' Spot test data).
const ACCOUNT: &str =
    include_str!("../../../robson-connectors/src/binance/spot/testdata/account.json");
const EXCHANGE_INFO: &str =
    include_str!("../../../robson-connectors/src/binance/spot/testdata/exchange_info_btcusdt.json");

/// One request as the venue read it: its path, its query as it arrived and,
/// for a signed one, whether it carried the test key and a valid signature
/// of that query (`None`: unsigned).
#[derive(Clone)]
struct Seen {
    path: String,
    query: String,
    signature: Option<bool>,
}

#[derive(Default)]
struct Wire {
    seen: Vec<Seen>,
    scripts: Vec<(String, u16, String)>,
    holds: Vec<String>,
    held: usize,
    unserved: u64,
    used_weight: Option<u32>,
}

/// The fake Spot venue of these tests, one request per connection: the
/// triple and the rules answered 200 with Binance's documented bodies, a
/// signed one only with the test key and a valid signature (else 400,
/// -1022); any other path answered 400 and counted unserved. A scripted
/// answer or a hold (answered 503 at shutdown) takes its path's next
/// request; a usage head goes on every answer once set.
struct Venue {
    url: String,
    wire: Arc<Mutex<Wire>>,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl Venue {
    fn url(&self) -> String {
        self.url.clone()
    }

    fn wire(&self) -> std::sync::MutexGuard<'_, Wire> {
        self.wire.lock().unwrap()
    }

    fn requests(&self) -> Vec<Seen> {
        self.wire().seen.clone()
    }

    fn requests_total(&self) -> u64 {
        self.wire().seen.len() as u64
    }

    fn unserved_total(&self) -> u64 {
        self.wire().unserved
    }

    /// Requests held so far.
    fn held(&self) -> usize {
        self.wire().held
    }

    fn script_next(&self, path: &str, status: u16, body: &str) {
        self.wire().scripts.push((path.to_owned(), status, body.to_owned()));
    }

    fn hold_next(&self, path: &str) {
        self.wire().holds.push(path.to_owned());
    }

    fn report_used_weight(&self, used: Option<u32>) {
        self.wire().used_weight = used;
    }

    /// Stop: the held requests are answered and every connection ends,
    /// within 5 s.
    async fn shutdown(self) -> Result<(), tokio::time::error::Elapsed> {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .map(|joined| joined.unwrap())
    }
}

async fn venue() -> Venue {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (wire, stop) = (Arc::new(Mutex::new(Wire::default())), CancellationToken::new());
    let (shared, stopped) = (Arc::clone(&wire), stop.clone());
    let task = tokio::spawn(async move {
        let mut exchanges = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = stopped.cancelled() => break,
                accepted = listener.accept() => {
                    let Ok((socket, _)) = accepted else { break };
                    exchanges.spawn(exchange(socket, Arc::clone(&shared), stopped.clone()));
                },
            }
        }
        while exchanges.join_next().await.is_some() {}
    });
    Venue { url, wire, stop, task }
}

/// A request head up to its blank line; `None` when the peer left first.
async fn read_head(socket: &mut tokio::net::TcpStream) -> Option<String> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(socket.read_u8().await.ok()?);
    }
    String::from_utf8(head).ok()
}

async fn exchange(
    mut socket: tokio::net::TcpStream,
    wire: Arc<Mutex<Wire>>,
    stop: CancellationToken,
) {
    let head = tokio::select! {
        _ = stop.cancelled() => None,
        head = read_head(&mut socket) => head,
    };
    let Some(head) = head else { return };
    let mut lines = head.lines();
    let target = lines.next().and_then(|line| line.split(' ').nth(1)).unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let key = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("x-mbx-apikey").then_some(value.trim())
    });
    let signature = query.rsplit_once("&signature=").map(|(payload, signature)| {
        key == Some(API_KEY) && signature.eq_ignore_ascii_case(&hmac_hex(API_SECRET, payload))
    });
    let (answer, used) = {
        let mut wire = wire.lock().unwrap();
        let (on, asked) = (path.to_owned(), query.to_owned());
        wire.seen.push(Seen { path: on, query: asked, signature });
        let served = match path {
            "/api/v3/account" => Some((true, ACCOUNT)),
            "/api/v3/openOrders" | "/api/v3/openOrderList" => Some((true, "[]")),
            "/api/v3/exchangeInfo" => Some((false, EXCHANGE_INFO)),
            _ => None,
        };
        let held = wire.holds.iter().position(|on| on == path);
        let scripted = wire.scripts.iter().position(|(on, ..)| on == path);
        let answer = match (served, held, scripted) {
            (None, ..) => {
                wire.unserved += 1;
                Some((400, r#"{"code":-1100,"msg":"unserved"}"#.to_owned()))
            },
            (Some((true, _)), ..) if signature != Some(true) => {
                Some((400, r#"{"code":-1022,"msg":"Signature is not valid."}"#.to_owned()))
            },
            (Some(_), Some(at), _) => {
                wire.holds.remove(at);
                wire.held += 1;
                None
            },
            (Some(_), None, Some(at)) => {
                let (_, status, body) = wire.scripts.remove(at);
                Some((status, body))
            },
            (Some((_, body)), None, None) => Some((200, body.to_owned())),
        };
        (answer, wire.used_weight)
    };
    let (status, body) = match answer {
        Some(answer) => answer,
        None => {
            stop.cancelled().await;
            (503, r#"{"code":-1007,"msg":"held until shutdown"}"#.to_owned())
        },
    };
    let usage = used.map_or_else(String::new, |used| format!("x-mbx-used-weight-1m: {used}\r\n"));
    let answer = format!(
        "HTTP/1.1 {status} Fake\r\ncontent-type: application/json\r\n{usage}content-length: \
         {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(answer.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// A listener answering every request with a 1 GiB head, then 64 KiB every
/// 10 ms until the reader hangs up.
async fn endless() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let _ = socket.read(&mut [0; 4096]).await;
                let mut out = b"HTTP/1.1 200 OK\r\ncontent-length: 1073741824\r\n\r\n".to_vec();
                while socket.write_all(&out).await.is_ok() {
                    out = vec![b' '; 64 << 10];
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            });
        }
    });
    url
}

async fn until(mut done: impl FnMut() -> bool) {
    let wait = async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await.expect("within 10 s");
}

/// Each read end to end, charged at its handoff and only then: the triple
/// (32), the rules (20), a kline (2; the fake's 400 is the caller's) and the
/// commission (20; the fake's 400 fails it). Every signed request carries a
/// valid signature; the ACCOUNT receipt feeds the observation interval.
#[tokio::test(flavor = "multi_thread")]
async fn every_spot_read_is_charged_before_its_handoff() {
    let venue = venue().await;
    let owner = rig(venue.url());
    owner.publish(1);
    let triple = owner.reads.inventory(soon(15_000)).await.unwrap();
    assert_eq!(owner.used(), (32, 3));
    let triple_reads = [
        SpotPreparedRead::Account,
        SpotPreparedRead::OperatedOpenOrders,
        SpotPreparedRead::OpenOrderLists,
    ];
    assert!(triple.iter().map(|receipt| receipt.read).eq(triple_reads.map(Read::Closed)));
    let account = &triple[0];
    assert!(matches!(account.body, ReadBody::Closed(SpotReadBody::Account(_))));
    let (_, u2) = account.sample.unwrap();
    assert!(ObservationInterval::from_account(&account.send, &account.receive, u2).is_some());
    let rules = owner.reads.rules(soon(10_000)).await.unwrap();
    assert!(matches!(rules.body, ReadBody::Closed(SpotReadBody::Rules(_))));
    assert_eq!(owner.used(), (52, 4));
    let kline = owner.reads.kline_at(1_800_000_000_000, soon(10_000)).await.unwrap();
    assert!(kline.status == 400 && matches!(kline.body, ReadBody::Closed(SpotReadBody::Kline(_))));
    assert_eq!(owner.used(), (54, 5));
    assert_eq!(owner.reads.commission(soon(10_000)).await.err(), Some(TimeError::Venue));
    assert_eq!(owner.used(), (74, 6));
    // The fake serves neither the kline nor the commission: it answers them
    // 400, the commission's signature checked all the same.
    let requests = venue.requests();
    let seen: Vec<_> = requests.iter().map(|r| (r.path.clone(), r.signature)).collect();
    let (signed, unsigned) = (Some(true), None);
    assert_eq!(seen, [
        ("/api/v3/account".to_owned(), signed),
        ("/api/v3/openOrders".to_owned(), signed),
        ("/api/v3/openOrderList".to_owned(), signed),
        ("/api/v3/exchangeInfo".to_owned(), unsigned),
        ("/api/v3/klines".to_owned(), unsigned),
        ("/api/v3/account/commission".to_owned(), signed),
    ]);
    let commission = &requests[5].query;
    assert!(commission.contains("timestamp=") && commission.contains("&signature="));
    assert_eq!(venue.unserved_total(), 2);
    venue.shutdown().await.unwrap();
}

/// One signing time for the triple: one timestamp on the wire, one sample
/// and generation in its receipts, which hold again after the triple. A
/// sample rotated after a triple's signing time refuses its next first poll
/// (unsent, its reservation refunded): here the next triple has signed and
/// reserved its ACCOUNT and waits for a worker permit when the sample rotates.
/// The earlier receipt then no longer holds.
#[tokio::test(flavor = "multi_thread")]
async fn an_inventory_triple_uses_one_sample() {
    let venue = venue().await;
    let owner = rig(venue.url());
    owner.publish(1);
    let triple = owner.reads.inventory(soon(15_000)).await.unwrap();
    let marks: Vec<_> = triple.iter().map(|r| (r.generation, r.sample)).collect();
    assert_eq!(marks, vec![marks[0]; 3]);
    let stamp =
        |query: &str| query.split('&').find(|p| p.starts_with("timestamp=")).map(str::to_owned);
    let stamps: Vec<_> = venue.requests().iter().map(|r| stamp(&r.query).unwrap()).collect();
    assert_eq!(stamps, vec![stamps[0].clone(); 3]);
    assert_eq!(owner.reads.still_holds(&triple[2]), Ok(()));
    let workers = Arc::clone(&owner.reads.workers).try_acquire_many_owned(2).unwrap();
    let reads = owner.reads.clone();
    let next = tokio::spawn(async move { reads.inventory(soon(15_000)).await.map(drop) });
    until(|| owner.used() == (52, 3)).await;
    owner.publish(2);
    drop(workers);
    assert_eq!(next.await.unwrap(), Err(TimeError::Budget(BudgetError::Deferred)));
    assert_eq!(owner.used(), (32, 3));
    assert_eq!(venue.requests().len(), 3);
    assert_eq!(owner.reads.still_holds(&triple[2]), Err(TimeError::Native));
    venue.shutdown().await.unwrap();
}

/// A body is refused once it would pass its cap, never read on: an endless
/// rules body fails `Payload` long before its deadline (charged), and an
/// open-orders body one byte over its cap ends the triple there.
#[tokio::test(flavor = "multi_thread")]
async fn a_body_over_its_cap_is_refused_before_decode() {
    let flood = rig(endless().await);
    // `Payload`, not `Expired`: refused at its cap, before its deadline; a
    // read that went on would end only at that deadline, the body endless.
    // The deadline is a read's own bound, so nothing shorter than what a
    // read may take races the transport's preparation.
    let read = flood.reads.rules(Instant::now() + REQUEST_BOUND).await;
    assert_eq!(read.err(), Some(TimeError::Payload));
    assert_eq!(flood.used(), (20, 1));
    let venue = venue().await;
    let owner = rig(venue.url());
    owner.publish(1);
    let over = " ".repeat((256 << 10) + 1);
    venue.script_next("/api/v3/openOrders", 200, &over);
    assert_eq!(owner.reads.inventory(soon(15_000)).await.err(), Some(TimeError::Payload));
    assert_eq!(owner.used(), (26, 2));
    assert_eq!(venue.requests().len(), 2);
    venue.shutdown().await.unwrap();
}

/// A deadline before the handoff is a proven no-send; one passed after it
/// leaves the weight charged and fails the read. The triple's subreads share
/// its one deadline: a held third read ends with it, never 10 s later; and a
/// deadline that passes before the third read is reserved sends no third.
#[tokio::test(flavor = "multi_thread")]
async fn a_composite_never_resets_its_deadline() {
    let venue = venue().await;
    let owner = rig(venue.url());
    owner.publish(1);
    let late = owner.reads.rules(Instant::now()).await.err();
    assert!(matches!(
        late,
        Some(TimeError::Expired | TimeError::Budget(BudgetError::Expired))
    ));
    assert_eq!(owner.used(), (0, 0));
    venue.hold_next("/api/v3/openOrderList");
    let began = Instant::now();
    let deadline = began + Duration::from_millis(1_500);
    let held = owner.reads.inventory(deadline).await;
    let ended = Instant::now();
    // Which subread the deadline found (the held third, or an earlier one
    // when the first two took longer) depends on the host, not on the code;
    // either way the triple ends at its one deadline. A deadline reset for a
    // later subread would let it run REQUEST_BOUND from its own start, which
    // comes after `began`, and a third read is always sent then.
    assert!(matches!(
        held.err(),
        Some(TimeError::Expired | TimeError::Budget(BudgetError::Expired))
    ));
    assert!(ended >= deadline && ended < began + REQUEST_BOUND, "{:?}", ended - began);
    // Each subread handed off is charged at its weight, in order, and none
    // reaches the venue without its handoff.
    let (weight, handoffs) = owner.used();
    assert!(handoffs <= 3, "{handoffs} handoffs");
    assert_eq!((weight, handoffs), [(0, 0), (20, 1), (26, 2), (32, 3)][handoffs as usize]);
    assert!(venue.requests().len() as u64 <= handoffs);
    venue.shutdown().await.unwrap();
}

/// A 429 during the triple sets the pause from its head: the triple fails
/// there, its remaining reads unsent, and no read is handed off after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_429_during_the_triple_pauses_the_rest() {
    let venue = venue().await;
    let owner = rig(venue.url());
    owner.publish(1);
    venue.script_next("/api/v3/account", 429, "{}");
    assert_eq!(owner.reads.inventory(soon(15_000)).await.err(), Some(TimeError::Venue));
    assert!(owner.budget.pending_pause());
    let paused = owner.reads.rules(soon(10_000)).await.err();
    assert_eq!(paused, Some(TimeError::Budget(BudgetError::Deferred)));
    assert_eq!(owner.used(), (20, 1));
    assert_eq!(venue.requests().len(), 1);
    venue.shutdown().await.unwrap();
}

/// A signed read needs a published sample that fits: without one it fails
/// before any reservation, nothing sent. An unsigned read needs none, and
/// names one only when it fits. A closing owner refuses every read.
#[tokio::test(flavor = "multi_thread")]
async fn no_signed_read_without_valid_clock_evidence() {
    let venue = venue().await;
    let owner = rig(venue.url());
    assert_eq!(owner.reads.inventory(soon(15_000)).await.err(), Some(TimeError::Native));
    assert_eq!(owner.reads.commission(soon(10_000)).await.err(), Some(TimeError::Native));
    assert_eq!(owner.used(), (0, 0));
    assert!(venue.requests().is_empty() && venue.unserved_total() == 0);
    assert_eq!(owner.reads.rules(soon(10_000)).await.unwrap().sample, None);
    owner.publish(1);
    assert!(owner.reads.rules(soon(10_000)).await.unwrap().sample.is_some());
    owner.handle.cancel.cancel();
    assert_eq!(owner.reads.inventory(soon(15_000)).await.err(), Some(TimeError::Closing));
    assert_eq!(owner.reads.rules(soon(10_000)).await.err(), Some(TimeError::Closing));
    assert_eq!(owner.used(), (40, 2));
    venue.shutdown().await.unwrap();
}

/// With four reads in flight a fifth is refused at once, `Busy`: nothing
/// reserved, prepared or sent, no queue.
#[tokio::test(flavor = "multi_thread")]
async fn a_fifth_reader_is_refused_not_queued() {
    let venue = venue().await;
    let owner = rig(venue.url());
    owner.publish(1);
    let rules = |reads: SpotReads| async move { reads.rules(soon(10_000)).await };
    let mut held = Vec::new();
    for reader in 1..=4 {
        venue.hold_next("/api/v3/exchangeInfo");
        held.push(tokio::spawn(rules(owner.reads.clone())));
        until(|| venue.held() == reader).await;
    }
    // At once means on its first poll, before any await: no clock decides it.
    let fifth = owner.reads.inventory(soon(15_000));
    let mut fifth = std::pin::pin!(fifth);
    let polled = std::future::poll_fn(|cx| Poll::Ready(fifth.as_mut().poll(cx))).await;
    let Poll::Ready(fifth) = polled else {
        panic!("a fifth reader was queued, not refused")
    };
    assert_eq!(fifth.err(), Some(TimeError::Busy));
    assert_eq!(owner.used(), (80, 4));
    venue.shutdown().await.unwrap();
    for read in held {
        assert_eq!(read.await.unwrap().err(), Some(TimeError::Venue));
    }
    assert_eq!(owner.reads.slots.available_permits(), 4);
}

/// A reader dropped while it waits for a worker permit returns its slot and
/// its refunded reservation; dropped after its handoff, its weight stays
/// charged and its slot and permits still return.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_reader_releases_its_slot_and_charge() {
    let venue = venue().await;
    let owner = rig(venue.url());
    let workers = Arc::clone(&owner.reads.workers).try_acquire_many_owned(2).unwrap();
    let reads = owner.reads.clone();
    let waiting = tokio::spawn(async move { reads.rules(soon(10_000)).await });
    until(|| owner.reads.slots.available_permits() == 3 && owner.used() == (20, 0)).await;
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    assert_eq!(owner.reads.slots.available_permits(), 4);
    assert_eq!(owner.used(), (0, 0));
    drop(workers);
    venue.hold_next("/api/v3/exchangeInfo");
    let reads = owner.reads.clone();
    let sent = tokio::spawn(async move { reads.rules(soon(10_000)).await });
    until(|| venue.held() == 1).await;
    sent.abort();
    assert!(sent.await.unwrap_err().is_cancelled());
    assert_eq!(owner.used(), (20, 1));
    assert_eq!(owner.reads.slots.available_permits(), 4);
    assert_eq!(owner.reads.workers.available_permits(), 2);
    venue.shutdown().await.unwrap();
}

/// O1/O2's observation: the read owner asks for an on-demand sample, which
/// the sampler (here the test) takes, publishes and answers; ACCOUNT is then
/// signed on that sample, and its receipt names it.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_observation_reads_on_its_own_sample() {
    let venue = venue().await;
    let owner = rig(venue.url());
    owner.publish(1);
    let sampler = async {
        let request = owner.handle.requests.next().await;
        owner.publish(2);
        request.answer(Ok(Uuid::from_u128(2)));
    };
    let (observed, ()) = tokio::join!(owner.reads.account_observation(soon(12_000)), sampler);
    let observed = observed.unwrap();
    assert_eq!(observed.sample.map(|(id, _)| id), Some(Uuid::from_u128(2)));
    assert!(matches!(observed.body, ReadBody::Closed(SpotReadBody::Account(_))));
    assert_eq!(owner.used(), (20, 1));
    assert_eq!(venue.requests()[0].signature, Some(true));
    venue.shutdown().await.unwrap();
}

const COMMISSION: &str = r#"{"symbol":"BTCUSDT","standardCommission":{"maker":"0.001","taker":"0.001","buyer":"0","seller":"0"},"specialCommission":{"maker":"0","taker":"0","buyer":"0","seller":"0"},"taxCommission":{"maker":"0","taker":"0","buyer":"0","seller":"0"},"discount":{"enabledForAccount":false,"enabledForSymbol":false,"discountAsset":"BNB","discount":"0.25"}}"#;

/// HMAC-SHA256 in hex, independent of the client's signer (the fixed test
/// secret fits one block).
fn hmac_hex(secret: &str, payload: &str) -> String {
    use sha2::{Digest, Sha256};
    let (mut inner, mut outer) = ([0x36_u8; 64], [0x5c_u8; 64]);
    for (index, byte) in secret.bytes().enumerate() {
        inner[index] ^= byte;
        outer[index] ^= byte;
    }
    let inner = Sha256::new().chain_update(inner).chain_update(payload).finalize();
    let outer = Sha256::new().chain_update(outer).chain_update(inner).finalize();
    outer.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The commission end to end: signed with the owner's timestamp (a listener
/// checks the HMAC of the exact wire parameters with the fixed test secret),
/// answered 200, decoded for the operated pair, charged 20 at its handoff.
#[tokio::test(flavor = "multi_thread")]
async fn the_commission_is_signed_and_decoded() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let owner = rig(format!("http://{}", listener.local_addr().unwrap()));
    owner.publish(1);
    let served = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut chunk = [0; 1024];
            let read = socket.read(&mut chunk).await.unwrap();
            assert!(read > 0, "the request head ended early");
            head.extend_from_slice(&chunk[..read]);
        }
        let line = String::from_utf8_lossy(&head).lines().next().unwrap().to_owned();
        let answer = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{COMMISSION}",
            COMMISSION.len()
        );
        socket.write_all(answer.as_bytes()).await.unwrap();
        line
    });
    let receipt = owner.reads.commission(soon(10_000)).await.unwrap();
    let line = served.await.unwrap();
    let query = line.strip_prefix("GET /api/v3/account/commission?").unwrap();
    let (payload, signature) =
        query.strip_suffix(" HTTP/1.1").unwrap().split_once("&signature=").unwrap();
    assert!(payload.starts_with("symbol=BTCUSDT&timestamp="), "{payload}");
    assert_eq!(signature, hmac_hex(API_SECRET, payload));
    assert!(matches!(&receipt.body, ReadBody::Commission(c) if c.symbol == "BTCUSDT"));
    assert!(receipt.status == 200 && receipt.sample.is_some());
    assert_eq!(owner.used(), (20, 1));
}

/// One random interleaving: callers with random reads, deadlines, repeats
/// and cancellations; an optional 429 on ACCOUNT, an optional usage level
/// on every answer, samples rotating on demand and on a random schedule (the
/// test is the sampler); an optional closing.
async fn interleave(
    callers: Vec<(u8, u16, Option<u16>, u8)>,
    throttled: bool,
    usage: Option<u32>,
    rotate: Option<u16>,
    closing: Option<u16>,
) -> Result<(), TestCaseError> {
    let venue = venue().await;
    let owner = rig(venue.url());
    owner.publish(1);
    if throttled {
        venue.script_next("/api/v3/account", 429, "{}");
    }
    venue.report_used_weight(usage);
    let quiet = callers.iter().all(|caller| caller.2.is_none()) && closing.is_none();
    // A read that expired or failed in transport may or may not have reached
    // the venue after its handoff.
    let ambiguous = Arc::new(AtomicBool::new(false));
    let tasks: Vec<_> = callers
        .into_iter()
        .map(|(read, ms, cancel, repeat)| {
            let (reads, ambiguous) = (owner.reads.clone(), Arc::clone(&ambiguous));
            tokio::spawn(async move {
                for _ in 0..repeat {
                    let deadline = soon(u64::from(ms));
                    let call = async {
                        match read {
                            0 => reads.inventory(deadline).await.map(drop),
                            1 => reads.rules(deadline).await.map(drop),
                            2 => reads.kline_at(0, deadline).await.map(drop),
                            3 => reads.commission(deadline).await.map(drop),
                            _ => reads.account_observation(deadline).await.map(drop),
                        }
                    };
                    let outcome = match cancel {
                        Some(ms) => {
                            tokio::time::timeout(Duration::from_millis(ms.into()), call).await.ok()
                        },
                        None => Some(call.await),
                    };
                    if matches!(outcome, Some(Err(TimeError::Expired | TimeError::Transport))) {
                        ambiguous.store(true, SeqCst);
                    }
                }
            })
        })
        .collect();
    let (mut closed, mut paused, mut sample) = (None, None, 1);
    let began = Instant::now();
    let mut rotated = began;
    while tasks.iter().any(|task| !task.is_finished()) {
        if closed.is_none() && closing.is_some_and(|ms| began.elapsed().as_millis() >= ms.into()) {
            owner.budget.begin_closing();
            owner.handle.cancel.cancel();
            closed = Some(owner.used().1);
        }
        if paused.is_none() && owner.budget.pending_pause() {
            paused = Some(owner.used().1);
        }
        let due = rotate.is_some_and(|ms| rotated.elapsed().as_millis() >= ms.into());
        let request = owner.handle.requests.try_take();
        if closed.is_none() && (due || request.is_some()) {
            sample += 1;
            owner.publish(sample);
            rotated = Instant::now();
        }
        if let Some(request) = request {
            request.answer(match closed {
                None => Ok(Uuid::from_u128(sample)),
                Some(_) => Err(TimeError::Closing),
            });
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    for task in tasks {
        task.await.unwrap();
    }
    let (weight, handoffs) = owner.used();
    prop_assert!(weight <= 400, "{weight} weight in one minute");
    for mark in [closed, paused].into_iter().flatten() {
        prop_assert_eq!(handoffs, mark, "a handoff after closing or during a pause");
    }
    prop_assert_eq!(owner.reads.slots.available_permits(), 4);
    // A dropped caller's preparation or decode job keeps its worker permit
    // until the job itself ends: a bounded drain, then every permit is back.
    let drain = async {
        while owner.reads.workers.available_permits() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    let drained = tokio::time::timeout(Duration::from_secs(10), drain).await.is_ok();
    prop_assert!(drained, "a worker permit never came back");
    // Every request the fake received (unserved ones included) was handed
    // off and charged its read's weight; with no caller dropped and no
    // ambiguous failure, every handoff reached it and the charge is theirs.
    let weight_of = |path: &str| match path {
        "/api/v3/openOrders" | "/api/v3/openOrderList" => 6,
        "/api/v3/klines" => 2,
        _ => 20,
    };
    let sent = venue.requests_total();
    let sent_weight: u64 = venue.requests().iter().map(|r| weight_of(&r.path)).sum();
    prop_assert!(sent <= handoffs && sent_weight <= weight);
    if quiet && !ambiguous.load(SeqCst) {
        prop_assert_eq!((sent, sent_weight), (handoffs, weight), "a handoff not at the venue");
    }
    venue.shutdown().await.unwrap();
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 12,
        max_shrink_iters: 8,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// PLAN §8 PS-5: random interleavings of up to eight callers, a 429,
    /// usage-level heads, rotating samples and a closing never pass 400
    /// weight in a minute, hand nothing off after closing or during a pause,
    /// return every slot and permit, and charge exactly the handed-off reads
    /// (every one reaching the venue when no caller was dropped). The test
    /// runs within one minute: the rolling window's own advance is the
    /// budget's (PS-2a) and its model's.
    #[test]
    fn property_reads_never_exceed_the_budget_or_outlive_closing(
        callers in prop::collection::vec(
            (0u8..5, 50u16..2_000, prop::option::of(0u16..400), 1u8..4),
            1..=8,
        ),
        throttled in any::<bool>(),
        usage in prop::option::of(prop::sample::select(vec![3_000u32, 4_800, 5_700])),
        rotate in prop::option::of(20u16..400),
        closing in prop::option::of(0u16..600),
    ) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(interleave(callers, throttled, usage, rotate, closing))?;
    }
}

/// 8p-4's mark of November 2026's boundary, asked on a valid clock long
/// after B + 60 s, and the kline open time it asks for (B − 60 s).
fn mark_month() -> (chrono::NaiveDate, robson_exec::boundary_carry::RobsonTime, u64) {
    let month = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let b = month.and_hms_opt(0, 0, 0).unwrap().and_utc();
    let verdict = Some(robson_exec::venue_ledger::ClockVerdict::Valid);
    let now = robson_exec::boundary_carry::RobsonTime { at: chrono::Utc::now(), verdict };
    (month, now, u64::try_from(b.timestamp_millis() - 60_000).unwrap())
}

/// 5a: the read owner as 8p-4's `MarkVenue` reserves only the kline's
/// weight (2), before anything is prepared; a dropped reservation is
/// refunded, and nothing is sent.
#[tokio::test(flavor = "multi_thread")]
async fn the_mark_venue_reserves_only_the_klines_weight() {
    use crate::boundary::mark::{MarkVenue as _, KLINE_WEIGHT};
    let venue = venue().await;
    let owner = rig(venue.url());
    for weight in [0, 1, 3, 6, 20] {
        assert!(owner.reads.try_reserve(weight).is_err(), "weight {weight}");
    }
    assert_eq!(owner.used(), (0, 0));
    let reserved = owner.reads.try_reserve(KLINE_WEIGHT).unwrap();
    assert_eq!(owner.used(), (2, 0), "reserved, not handed off");
    drop(reserved);
    assert_eq!(owner.used(), (0, 0), "refunded unsent");
    assert_eq!(venue.requests_total(), 0);
    venue.shutdown().await.unwrap();
}

/// 5a: one boundary mark is one charge of the kline's weight, then one
/// unsigned send on that reservation through the single read path (no second
/// reservation); the venue's status and body come back as they came. This
/// venue answers every kline 400 with an error object, which the mark refuses
/// (K stays pending).
#[tokio::test(flavor = "multi_thread")]
async fn one_mark_kline_is_one_charge_and_one_unsigned_send() {
    use crate::boundary::mark::{kline_at, MarkVenue as _, KLINE_WEIGHT};
    let venue = venue().await;
    let owner = rig(venue.url());
    let reserved = owner.reads.try_reserve(KLINE_WEIGHT).unwrap();
    let (status, body) = owner.reads.read_kline(reserved, 1_800_000_000_000).await.unwrap();
    assert_eq!((status, body.as_str()), (400, r#"{"code":-1100,"msg":"unserved"}"#));
    assert_eq!(owner.used(), (2, 1));
    let (month, now, open) = mark_month();
    assert!(kline_at(&owner.reads, month, now).await.is_err());
    assert_eq!(owner.used(), (4, 2));
    let requests = venue.requests();
    assert_eq!(requests.len(), 2);
    for (seen, start) in requests.iter().zip([1_800_000_000_000, open]) {
        assert_eq!((seen.path.as_str(), seen.signature), ("/api/v3/klines", None));
        assert!(seen.query.contains(&format!("&startTime={start}&")), "{}", seen.query);
    }
    venue.shutdown().await.unwrap();
}

/// 5a: a paused budget defers the mark's reservation at once: `kline_at`
/// fails with nothing charged and nothing sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_deferred_budget_sends_no_mark_kline() {
    use crate::boundary::mark::{kline_at, MarkVenue as _, KLINE_WEIGHT};
    let venue = venue().await;
    let owner = rig(venue.url());
    owner.publish(1);
    venue.script_next("/api/v3/account", 429, "{}");
    assert_eq!(owner.reads.inventory(soon(15_000)).await.err(), Some(TimeError::Venue));
    assert!(owner.budget.pending_pause());
    let deferred = owner.reads.try_reserve(KLINE_WEIGHT).err().unwrap();
    assert!(deferred.to_string().contains("Deferred"), "{deferred}");
    let (month, now, _) = mark_month();
    assert!(kline_at(&owner.reads, month, now).await.is_err());
    assert_eq!(owner.used(), (20, 1));
    assert_eq!(venue.requests().len(), 1);
    venue.shutdown().await.unwrap();
}

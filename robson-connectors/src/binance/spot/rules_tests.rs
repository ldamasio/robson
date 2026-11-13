//! ADR-0056 step 8c: the projection of a symbol's spot rules onto the
//! domain's, and the rules cache, on the wire against a local stand-in.
//! Nothing reaches the network.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};

use robson_domain::{Symbol, SymbolTradingRules};
use rust_decimal_macros::dec;
use serde_json::Value;

use super::{
    cache::{SpotRulesCache, SpotRulesError},
    rules::parse_symbol_rules,
    stand_in, SpotReadError, SpotRestReader, SpotSymbolRules,
};

const EXCHANGE_INFO: &str = include_str!("testdata/exchange_info_btcusdt.json");

fn symbol(pair: &str) -> Symbol {
    Symbol::from_pair(pair).unwrap()
}

/// The BTCUSDT entry after `edit`.
fn edited(edit: impl FnOnce(&mut Value)) -> String {
    let mut info: Value = serde_json::from_str(EXCHANGE_INFO).unwrap();
    edit(&mut info["symbols"][0]);
    info.to_string()
}

fn filter<'a>(entry: &'a mut Value, kind: &str) -> &'a mut Value {
    entry["filters"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["filterType"] == kind)
        .unwrap()
}

fn without(entry: &mut Value, kind: &str) {
    entry["filters"].as_array_mut().unwrap().retain(|f| f["filterType"] != kind);
}

fn rules_of(body: &str) -> SpotSymbolRules {
    parse_symbol_rules(body, "BTCUSDT").unwrap()
}

fn refusal(body: &str) -> String {
    match rules_of(body).trading_rules(&symbol("BTCUSDT")) {
        Err(SpotReadError::InvalidRules(why)) => why,
        other => panic!("expected InvalidRules, got {other:?}"),
    }
}

#[test]
fn btcusdt_projects_its_price_grid_market_lot_and_notional() {
    let spot = rules_of(EXCHANGE_INFO);
    let rules = spot.trading_rules(&symbol("BTCUSDT")).unwrap();
    assert_eq!((rules.tick_size(), rules.min_price()), (dec!(0.01), dec!(0.01)));
    // LOT_SIZE and MARKET_LOT_SIZE together: MARKET_LOT_SIZE's zero step and
    // minimum add nothing, its maximum binds.
    assert_eq!(
        (rules.step_size(), rules.min_qty(), rules.max_qty()),
        (dec!(0.00001), dec!(0.00001), dec!(108.95042220))
    );
    assert_eq!(rules.min_notional(), dec!(5));
    assert_eq!((rules.price_precision(), rules.quantity_precision()), (8, 8));
    assert_eq!(rules.symbol(), &symbol("BTCUSDT"));
    assert_eq!(spot.allowed_stp_modes, [
        "EXPIRE_TAKER",
        "EXPIRE_MAKER",
        "EXPIRE_BOTH",
        "DECREMENT",
        "TRANSFER"
    ]);
}

#[test]
fn a_zero_min_price_is_on_every_grid_and_min_notional_binds_whatever_market_says() {
    let body = edited(|entry| {
        filter(entry, "PRICE_FILTER")["minPrice"] = "0.00000000".into();
        filter(entry, "NOTIONAL")["applyMinToMarket"] = false.into();
    });
    let rules = rules_of(&body).trading_rules(&symbol("BTCUSDT")).unwrap();
    assert_eq!((rules.min_price(), rules.min_notional()), (dec!(0), dec!(5)));
}

#[test]
fn rules_without_a_usable_grid_lot_or_notional_are_refused() {
    let no_price = edited(|entry| without(entry, "PRICE_FILTER"));
    assert!(refusal(&no_price).contains("no PRICE_FILTER"));
    let zero_tick = edited(|entry| filter(entry, "PRICE_FILTER")["tickSize"] = "0".into());
    assert!(refusal(&zero_tick).contains("no price grid"));
    // Binance's grid is price % tickSize == 0; the domain's is anchored at
    // minPrice. 0.015 on a 0.01 tick would round onto prices Binance refuses.
    let off_grid = edited(|entry| filter(entry, "PRICE_FILTER")["minPrice"] = "0.01500000".into());
    assert!(refusal(&off_grid).contains("off the tickSize"));
    let no_lot = edited(|entry| without(entry, "LOT_SIZE"));
    assert!(refusal(&no_lot).contains("no LOT_SIZE"));
    let crossed =
        edited(|entry| filter(entry, "MARKET_LOT_SIZE")["stepSize"] = "0.00001500".into());
    assert!(refusal(&crossed).contains("do not nest"));
    let no_notional = edited(|entry| without(entry, "NOTIONAL"));
    assert!(refusal(&no_notional).contains("no NOTIONAL"));
    // What the domain refuses: no minimum lot.
    let no_minimum = edited(|entry| filter(entry, "LOT_SIZE")["minQty"] = "0".into());
    assert!(refusal(&no_minimum).contains("min_qty"));
}

#[test]
fn rules_of_another_symbol_are_inconsistent_and_the_stp_modes_are_required() {
    assert!(matches!(
        rules_of(EXCHANGE_INFO).trading_rules(&Symbol::new("BTCU".into(), "SDT".into()).unwrap()),
        Err(SpotReadError::Inconsistent(_))
    ));
    // The venue's own symbol must be the one asked for, even when its assets
    // match.
    let renamed = edited(|entry| entry["symbol"] = "BTCUSDTX".into());
    assert!(matches!(
        parse_symbol_rules(&renamed, "BTCUSDTX")
            .unwrap()
            .trading_rules(&symbol("BTCUSDT")),
        Err(SpotReadError::Inconsistent(_))
    ));
    let no_modes = edited(|entry| {
        entry.as_object_mut().unwrap().remove("allowedSelfTradePreventionModes");
    });
    assert!(matches!(parse_symbol_rules(&no_modes, "BTCUSDT"), Err(SpotReadError::Parse(_))));
}

// ---- The cache ----

/// A stand-in answering exchangeInfo for whatever symbol is asked (BTCUSDT's
/// rules under that name), or with `status` when `failing` says so; counts
/// the requests.
async fn rules_venue(
    failing: Arc<AtomicUsize>,
) -> (SpotRestReader, Arc<AtomicUsize>, stand_in::Seens) {
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    let (client, seen) = stand_in::serve(move |request| {
        counter.fetch_add(1, Ordering::SeqCst);
        if failing.load(Ordering::SeqCst) > 0 {
            return (500, "{\"code\":-1001,\"msg\":\"Internal error.\"}".to_string());
        }
        let pair = request.param("symbol").unwrap_or("BTCUSDT").to_string();
        let base = pair.strip_suffix("USDT").unwrap().to_string();
        (
            200,
            edited(|entry| {
                entry["symbol"] = pair.into();
                entry["baseAsset"] = base.into();
            }),
        )
    })
    .await;
    (SpotRestReader::new(client), asked, seen)
}

fn requests(asked: &AtomicUsize) -> usize {
    asked.load(Ordering::SeqCst)
}

#[tokio::test]
async fn validated_rules_are_served_from_the_cache_until_their_ttl() {
    let failing = Arc::new(AtomicUsize::new(0));
    let (reader, asked, seen) = rules_venue(Arc::clone(&failing)).await;
    let cache = SpotRulesCache::new();
    let btc = symbol("BTCUSDT");
    assert!(cache.refresh_due(&btc));
    assert_eq!(cache.last_validated(&btc), None);
    let rules = cache.within_ttl(&reader, &btc).await.unwrap();
    assert_eq!(cache.cached(&reader, &btc).await.unwrap(), rules);
    assert_eq!(cache.within_ttl(&reader, &btc).await.unwrap(), rules);
    assert_eq!(cache.last_validated(&btc), Some(rules.clone()));
    assert!(!cache.refresh_due(&btc));
    assert_eq!(requests(&asked), 1, "one exchangeInfo");
    assert_eq!(seen.lock().unwrap()[0].identity(), "GET /api/v3/exchangeInfo?symbol=BTCUSDT");
    assert_eq!(cache.spot_rules(&btc).unwrap().symbol, "BTCUSDT");

    cache.age_entry("BTCUSDT", Duration::from_secs(6 * 60 * 60));
    assert!(cache.refresh_due(&btc));
    cache.within_ttl(&reader, &btc).await.unwrap();
    assert_eq!(requests(&asked), 2, "refreshed past the TTL");
}

#[tokio::test]
async fn a_failed_refresh_keeps_stale_rules_for_reduction_and_fails_admission_for_five_minutes() {
    let failing = Arc::new(AtomicUsize::new(0));
    let (reader, asked, _) = rules_venue(Arc::clone(&failing)).await;
    let cache = SpotRulesCache::new();
    let btc = symbol("BTCUSDT");
    let rules = cache.within_ttl(&reader, &btc).await.unwrap();
    cache.age_entry("BTCUSDT", Duration::from_secs(6 * 60 * 60));
    failing.store(1, Ordering::SeqCst);

    // Admission fails closed; reduction is served the stale rules.
    assert!(matches!(cache.within_ttl(&reader, &btc).await, Err(SpotRulesError::Read(_))));
    assert_eq!(cache.cached(&reader, &btc).await.unwrap(), rules);
    assert_eq!(cache.last_validated(&btc), Some(rules.clone()));
    assert_eq!(requests(&asked), 2, "the failure is not retried at once");
    match cache.within_ttl(&reader, &btc).await {
        Err(SpotRulesError::RecentFailure { floor, .. }) => {
            assert_eq!(floor, Duration::from_secs(300))
        },
        other => panic!("expected RecentFailure, got {other:?}"),
    }
    cache.age_failure("BTCUSDT", Duration::from_secs(4 * 60));
    assert!(cache.within_ttl(&reader, &btc).await.is_err());
    assert_eq!(requests(&asked), 2, "still within five minutes");
    failing.store(0, Ordering::SeqCst);
    cache.age_failure("BTCUSDT", Duration::from_secs(5 * 60));
    assert_eq!(cache.within_ttl(&reader, &btc).await.unwrap(), rules);
    assert_eq!(requests(&asked), 3, "retried after five minutes");
}

#[tokio::test]
async fn with_nothing_cached_a_failure_is_retried_after_thirty_seconds() {
    let failing = Arc::new(AtomicUsize::new(1));
    let (reader, asked, _) = rules_venue(Arc::clone(&failing)).await;
    let cache = SpotRulesCache::new();
    let btc = symbol("BTCUSDT");
    assert!(cache.cached(&reader, &btc).await.is_err(), "nothing stale to serve");
    match cache.within_ttl(&reader, &btc).await {
        Err(SpotRulesError::RecentFailure { floor, .. }) => assert_eq!(floor.as_secs(), 30),
        other => panic!("expected RecentFailure, got {other:?}"),
    }
    assert_eq!(requests(&asked), 1);
    cache.age_failure("BTCUSDT", Duration::from_secs(31));
    failing.store(0, Ordering::SeqCst);
    cache.within_ttl(&reader, &btc).await.unwrap();
    assert_eq!(requests(&asked), 2);
    assert_eq!(cache.sizes(), (1, 0), "a success clears the failure");
}

#[tokio::test]
async fn the_refresh_after_a_rejection_is_bounded_per_symbol() {
    let failing = Arc::new(AtomicUsize::new(0));
    let (reader, asked, _) = rules_venue(Arc::clone(&failing)).await;
    let cache = SpotRulesCache::new();
    let btc = symbol("BTCUSDT");
    cache.refresh_after_rejection(&reader, &btc).await.unwrap();
    cache.refresh_after_rejection(&reader, &btc).await.unwrap();
    assert_eq!(requests(&asked), 1, "rules this recent are served");
    cache.age_entry("BTCUSDT", Duration::from_secs(31));
    failing.store(1, Ordering::SeqCst);
    assert!(cache.refresh_after_rejection(&reader, &btc).await.is_err());
    assert!(matches!(
        cache.refresh_after_rejection(&reader, &btc).await,
        Err(SpotRulesError::RecentFailure { .. })
    ));
    assert_eq!(requests(&asked), 2, "a failure this recent is repeated");
}

#[tokio::test]
async fn a_preview_reads_without_touching_the_cache() {
    let failing = Arc::new(AtomicUsize::new(0));
    let (reader, asked, _) = rules_venue(failing).await;
    let cache = SpotRulesCache::new();
    let btc = symbol("BTCUSDT");
    cache.preview(&reader, &btc).await.unwrap();
    assert_eq!(cache.sizes(), (0, 0), "nothing cached or throttled");
    assert_eq!(cache.last_validated(&btc), None);
    cache.within_ttl(&reader, &btc).await.unwrap();
    cache.preview(&reader, &btc).await.unwrap();
    assert_eq!(requests(&asked), 2, "a fresh entry serves the preview");
}

#[tokio::test]
async fn the_cache_and_its_failures_stay_within_sixty_four_symbols() {
    let failing = Arc::new(AtomicUsize::new(0));
    let (reader, asked, _) = rules_venue(Arc::clone(&failing)).await;
    let cache = SpotRulesCache::new();
    let pairs: Vec<Symbol> = (0..65).map(|i| symbol(&format!("X{i}USDT"))).collect();
    for pair in &pairs {
        cache.within_ttl(&reader, pair).await.unwrap();
    }
    assert_eq!(cache.sizes().0, 64);
    assert_eq!(cache.last_validated(&pairs[0]), None, "the least recently used went");
    failing.store(1, Ordering::SeqCst);
    for i in 0..65 {
        let _ = cache.within_ttl(&reader, &symbol(&format!("Y{i}USDT"))).await;
    }
    assert!(cache.sizes().1 <= 64, "{:?}", cache.sizes());
    assert_eq!(requests(&asked), 130);
}

/// A stand-in whose exchangeInfo answers, after the first, wait until it is
/// opened; it counts every request. The wait moves the stand-in's worker out
/// of the runtime (`block_in_place`), so no other task is held behind it.
#[derive(Default)]
struct Held {
    asked: AtomicUsize,
    open: (Mutex<bool>, Condvar),
}

impl Held {
    fn answer(&self) -> (u16, String) {
        if self.asked.fetch_add(1, Ordering::SeqCst) > 0 {
            tokio::task::block_in_place(|| {
                let (lock, opened) = &self.open;
                let mut open = lock.lock().unwrap();
                while !*open {
                    open = opened.wait(open).unwrap();
                }
            });
        }
        (200, EXCHANGE_INFO.to_string())
    }

    fn release(&self) {
        *self.open.0.lock().unwrap() = true;
        self.open.1.notify_all();
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

/// Opens the stand-in when dropped, so a failing test never leaves it waiting.
struct OpenOnDrop(Arc<Held>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// Rules loaded through a held stand-in, then aged past their TTL.
async fn held_cache() -> (Arc<Held>, Arc<SpotRestReader>, Arc<SpotRulesCache>, SymbolTradingRules) {
    let held = Arc::new(Held::default());
    let answering = Arc::clone(&held);
    let (client, _) = stand_in::serve(move |_| answering.answer()).await;
    let reader = Arc::new(SpotRestReader::new(client));
    let cache = Arc::new(SpotRulesCache::new());
    let stale = cache.within_ttl(&reader, &symbol("BTCUSDT")).await.unwrap();
    cache.age_entry("BTCUSDT", Duration::from_secs(6 * 60 * 60));
    (held, reader, cache, stale)
}

/// Wait, at most five seconds, until `ready`: an arrival, not a sleep.
async fn until(what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "never reached: {what}");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn refresh(
    cache: &Arc<SpotRulesCache>,
    reader: &Arc<SpotRestReader>,
) -> tokio::task::JoinHandle<Result<SymbolTradingRules, SpotRulesError>> {
    let (cache, reader) = (Arc::clone(cache), Arc::clone(reader));
    tokio::spawn(async move { cache.within_ttl(&reader, &symbol("BTCUSDT")).await })
}

/// Two callers find the rules past their TTL: one refreshes, the other waits
/// at the gate, and while the refresh's answer is held the last rules are
/// served at once. One request then serves both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_in_flight_is_single_and_never_holds_up_the_last_rules() {
    let (held, reader, cache, stale) = held_cache().await;
    let _open = OpenOnDrop(Arc::clone(&held));
    let btc = symbol("BTCUSDT");
    let callers = [refresh(&cache, &reader), refresh(&cache, &reader)];
    until("one refresh at the venue and one caller at the gate", || {
        held.asked() == 2 && cache.probe().gate_waiters.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(cache.last_validated(&btc), Some(stale), "served while the answer is held");
    assert_eq!(held.asked(), 2, "the waiting caller sent nothing");
    held.release();
    for caller in callers {
        caller.await.unwrap().unwrap();
    }
    assert_eq!(held.asked(), 2, "one refresh served both callers");
    assert_eq!(cache.probe().max_in_flight.load(Ordering::SeqCst), 1);
}

/// A refresh cancelled with its request at the venue frees the gate, counts
/// against the five-minute floor as a failure would, and leaves the stale
/// rules to reductions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_refresh_frees_the_gate_and_counts_against_the_floor() {
    let (held, reader, cache, stale) = held_cache().await;
    let _open = OpenOnDrop(Arc::clone(&held));
    let btc = symbol("BTCUSDT");
    let cancelled = refresh(&cache, &reader);
    until("the refresh at the venue", || held.asked() == 2).await;
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    until("the gate free", || cache.probe().gate_waiters.load(Ordering::SeqCst) == 0).await;
    match cache.within_ttl(&reader, &btc).await {
        Err(SpotRulesError::RecentFailure { floor, .. }) => assert_eq!(floor.as_secs(), 300),
        other => panic!("expected RecentFailure, got {other:?}"),
    }
    assert_eq!(cache.cached(&reader, &btc).await.unwrap(), stale);
    assert_eq!(held.asked(), 2, "nothing more was sent");
    held.release();
    cache.age_failure("BTCUSDT", Duration::from_secs(5 * 60));
    cache.within_ttl(&reader, &btc).await.unwrap();
    assert_eq!(held.asked(), 3, "retried after the floor");
}

/// xorshift64: the generated interleavings need no dependency.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

const SEEDS: u64 = 30;

/// Seeded interleavings: four tasks run random cache operations on three
/// symbols while the stand-in fails a quarter of its answers at random. At
/// most one gated fetch is ever in flight; each symbol is fetched at most once
/// more than the times its rules or its failure were aged, so waiting callers
/// never stampede; the cache stays bounded; whatever is served is the symbol's
/// projection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn random_interleavings_keep_one_refresh_at_a_time() {
    let pairs = ["BTCUSDT", "ETHUSDT", "X1USDT"];
    for seed in 1..=SEEDS {
        let mut venue_rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let venue_rng = Arc::new(Mutex::new({
            next(&mut venue_rng);
            venue_rng
        }));
        let (client, _) = stand_in::serve(move |request| {
            if next(&mut venue_rng.lock().unwrap()).is_multiple_of(4) {
                return (500, "{\"code\":-1001,\"msg\":\"Internal error.\"}".to_string());
            }
            let pair = request.param("symbol").unwrap().to_string();
            let base = pair.strip_suffix("USDT").unwrap().to_string();
            (
                200,
                edited(|entry| {
                    entry["symbol"] = pair.into();
                    entry["baseAsset"] = base.into();
                }),
            )
        })
        .await;
        let reader = Arc::new(SpotRestReader::new(client));
        let cache = Arc::new(SpotRulesCache::new());
        let aged: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
        let tasks: Vec<_> = (0..4u64)
            .map(|task| {
                let (reader, cache, aged) =
                    (Arc::clone(&reader), Arc::clone(&cache), Arc::clone(&aged));
                tokio::spawn(async move {
                    let mut rng = (seed << 8 | task).wrapping_mul(0xD1B5_4A32_D192_ED03) | 1;
                    for _ in 0..25 {
                        let pair = pairs[(next(&mut rng) % 3) as usize];
                        let at = symbol(pair);
                        for _ in 0..next(&mut rng) % 3 {
                            tokio::task::yield_now().await;
                        }
                        match next(&mut rng) % 7 {
                            0 => drop(cache.within_ttl(&reader, &at).await),
                            1 => drop(cache.cached(&reader, &at).await),
                            2 => drop(cache.refresh_after_rejection(&reader, &at).await),
                            3 => drop(cache.preview(&reader, &at).await),
                            4 => {
                                if let Some(rules) = cache.last_validated(&at) {
                                    assert_eq!(rules.symbol(), &at);
                                }
                            },
                            5 => {
                                if cache.try_age_entry(pair, Duration::from_secs(6 * 60 * 60)) {
                                    *aged.lock().unwrap().entry(pair.to_string()).or_default() += 1;
                                }
                            },
                            _ => {
                                if cache.try_age_failure(pair, Duration::from_secs(5 * 60)) {
                                    *aged.lock().unwrap().entry(pair.to_string()).or_default() += 1;
                                }
                            },
                        }
                    }
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let probe = cache.probe();
        assert!(
            probe.max_in_flight.load(Ordering::SeqCst) <= 1,
            "seed {seed}: two fetches at once"
        );
        let aged = aged.lock().unwrap();
        for (pair, fetches) in probe.fetches.lock().unwrap().iter() {
            let allowed = 1 + aged.get(pair).copied().unwrap_or(0);
            assert!(
                *fetches <= allowed,
                "seed {seed}: {pair} fetched {fetches} times, {allowed} allowed"
            );
        }
        assert!(cache.sizes().0 <= 3 && cache.sizes().1 <= 3, "seed {seed}");
        for pair in pairs {
            if let Some(rules) = cache.last_validated(&symbol(pair)) {
                assert_eq!(rules.symbol(), &symbol(pair), "seed {seed}");
                assert_eq!(rules.tick_size(), dec!(0.01), "seed {seed}");
            }
        }
    }
}

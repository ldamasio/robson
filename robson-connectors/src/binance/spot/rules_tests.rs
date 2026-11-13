//! ADR-0056 step 8c: the projection of a symbol's spot rules onto the
//! domain's, and the rules cache, on the wire against a local stand-in.
//! Nothing reaches the network.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use robson_domain::Symbol;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_in_flight_is_single_and_never_holds_up_the_last_rules() {
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    let (client, _) = stand_in::serve(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(400));
        (200, EXCHANGE_INFO.to_string())
    })
    .await;
    let reader = Arc::new(SpotRestReader::new(client));
    let cache = Arc::new(SpotRulesCache::new());
    let btc = symbol("BTCUSDT");
    let stale = cache.within_ttl(&reader, &btc).await.unwrap();
    cache.age_entry("BTCUSDT", Duration::from_secs(6 * 60 * 60));

    let refreshes: Vec<_> = (0..2)
        .map(|_| {
            let (cache, reader, btc) = (Arc::clone(&cache), Arc::clone(&reader), btc.clone());
            tokio::spawn(async move { cache.within_ttl(&reader, &btc).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    assert_eq!(cache.last_validated(&btc), Some(stale));
    assert!(started.elapsed() < Duration::from_millis(100), "{:?}", started.elapsed());
    for refresh in refreshes {
        refresh.await.unwrap().unwrap();
    }
    assert_eq!(asked.load(Ordering::SeqCst), 2, "one load, then one refresh for both");
}

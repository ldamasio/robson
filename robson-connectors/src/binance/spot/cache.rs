//! The spot trading-rules cache (ADR-0056 step 8c): the USD-M adapter's
//! contract (`usdm/exchange.rs`, issue #154 and ADR-0056 §5, §10), copied
//! with its constants so USD-M stays untouched (8c plan, Q14).
//!
//! - A bounded TTL cache of 64 symbols, least recently used out.
//! - One refresh at a time (single-flight), the cache lock never held across a
//!   request. [`SpotRulesCache::last_validated`] takes only the cache lock, so
//!   it never waits for a refresh in flight; [`SpotRulesCache::cached`] can
//!   wait for one before it falls back to stale rules.
//! - A failed refresh keeps the last validated rules: admission
//!   ([`SpotRulesCache::within_ttl`]) fails closed, risk reduction is served
//!   stale ([`SpotRulesCache::cached`], [`SpotRulesCache::last_validated`]).
//! - A failed refresh is not retried for five minutes with stale rules to
//!   serve, thirty seconds without; a refresh after a rules rejection serves or
//!   repeats anything thirty seconds old.
//!
//! Each entry keeps the whole [`SpotSymbolRules`] beside its projection, for
//! the STP modes and precisions later steps read.

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use robson_domain::{Symbol, SymbolTradingRules};
use thiserror::Error;
use tracing::warn;

use super::{
    rest::{SpotReadError, SpotRestReader},
    rules::SpotSymbolRules,
};

/// TTL of validated rules, as on USD-M.
const TRADING_RULES_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// A failed refresh with stale rules to serve is not retried sooner.
const RULES_REFRESH_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// A refresh after a rules rejection serves rules this recent, or repeats a
/// failure this recent, without another request.
const RULES_REJECTION_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// The retry floor for a symbol with nothing cached.
const RULES_LOOKUP_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// Symbols cached at most.
const TRADING_RULES_CACHE_CAPACITY: usize = 64;

/// Why the cache served no rules.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SpotRulesError {
    /// The read or the projection failed.
    #[error(transparent)]
    Read(#[from] SpotReadError),
    /// A refresh began less than `floor` ago and did not succeed.
    #[error("Trading rules for {pair} unavailable: a refresh less than {} s ago did not succeed", .floor.as_secs())]
    RecentFailure { pair: String, floor: Duration },
}

struct CachedRules {
    spot: SpotSymbolRules,
    rules: SymbolTradingRules,
    fetched_at: Instant,
    last_used_at: Instant,
}

/// The validated rules of the symbols traded on the spot venue.
pub(super) struct SpotRulesCache {
    entries: Mutex<HashMap<String, CachedRules>>,
    /// Held across a refresh, never by a reader of the cache.
    refresh_gate: tokio::sync::Mutex<()>,
    /// When the last refresh that has not succeeded began, per symbol.
    failed_at: Mutex<HashMap<String, Instant>>,
    /// Callers waiting for the refresh gate, fetches in flight and the most
    /// ever in flight at once, for the concurrency tests.
    #[cfg(test)]
    probe: Probe,
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct Probe {
    pub(super) gate_waiters: std::sync::atomic::AtomicUsize,
    in_flight: std::sync::atomic::AtomicUsize,
    pub(super) max_in_flight: std::sync::atomic::AtomicUsize,
    /// Gated fetches begun, per symbol.
    pub(super) fetches: Mutex<HashMap<String, usize>>,
}

/// Counts a waiter or a fetch for as long as it lives, cancellation included.
#[cfg(test)]
struct Counted<'a>(&'a std::sync::atomic::AtomicUsize);

#[cfg(test)]
impl<'a> Counted<'a> {
    fn new(counter: &'a std::sync::atomic::AtomicUsize) -> Self {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(counter)
    }
}

#[cfg(test)]
impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl SpotRulesCache {
    pub(super) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            refresh_gate: tokio::sync::Mutex::new(()),
            failed_at: Mutex::new(HashMap::new()),
            #[cfg(test)]
            probe: Probe::default(),
        }
    }

    /// Take the refresh gate: one refresh at a time.
    async fn gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        #[cfg(test)]
        let _waiting = Counted::new(&self.probe.gate_waiters);
        self.refresh_gate.lock().await
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<String, CachedRules>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn failures(&self) -> MutexGuard<'_, HashMap<String, Instant>> {
        self.failed_at.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The cached rules when fetched within `max_age`, marked as used.
    fn within(&self, pair: &str, max_age: Duration) -> Option<SymbolTradingRules> {
        let mut entries = self.entries();
        let cached = entries.get_mut(pair).filter(|c| c.fetched_at.elapsed() < max_age)?;
        cached.last_used_at = Instant::now();
        Some(cached.rules.clone())
    }

    fn failed_within(&self, pair: &str, window: Duration) -> Result<(), SpotRulesError> {
        if self.failures().get(pair).is_some_and(|at| at.elapsed() < window) {
            return Err(SpotRulesError::RecentFailure { pair: pair.to_string(), floor: window });
        }
        Ok(())
    }

    /// Note an attempt for `pair`, bounded like the cache it throttles.
    fn record_attempt(&self, pair: &str) {
        let mut failures = self.failures();
        if failures.len() >= TRADING_RULES_CACHE_CAPACITY && !failures.contains_key(pair) {
            failures.retain(|_, at| at.elapsed() < RULES_REFRESH_RETRY_INTERVAL);
            if failures.len() >= TRADING_RULES_CACHE_CAPACITY {
                let oldest = failures.iter().min_by_key(|(_, at)| **at).map(|(p, _)| p.clone());
                if let Some(oldest) = oldest {
                    failures.remove(&oldest);
                }
            }
        }
        failures.insert(pair.to_string(), Instant::now());
    }

    /// Read, project and store. The caller holds the refresh gate; the
    /// attempt is noted before the request and cleared on success, so a
    /// dropped refresh counts against the floor as a failure does.
    async fn fetch_and_store(
        &self,
        reader: &SpotRestReader,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, SpotRulesError> {
        let pair = symbol.as_pair();
        self.record_attempt(&pair);
        #[cfg(test)]
        let _fetching = {
            let fetching = Counted::new(&self.probe.in_flight);
            let now = self.probe.in_flight.load(std::sync::atomic::Ordering::SeqCst);
            self.probe.max_in_flight.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
            *self.probe.fetches.lock().unwrap().entry(pair.clone()).or_default() += 1;
            fetching
        };
        let spot = reader.symbol_rules(symbol).await?;
        let rules = spot.trading_rules(symbol)?;
        self.failures().remove(&pair);
        let mut entries = self.entries();
        if entries.len() >= TRADING_RULES_CACHE_CAPACITY && !entries.contains_key(&pair) {
            let lru = entries.iter().min_by_key(|(_, c)| c.last_used_at).map(|(p, _)| p.clone());
            if let Some(lru) = lru {
                warn!(symbol = %lru, capacity = TRADING_RULES_CACHE_CAPACITY, "Spot trading-rules cache full; evicting least recently used entry");
                entries.remove(&lru);
            }
        }
        let now = Instant::now();
        entries.insert(pair, CachedRules {
            spot,
            rules: rules.clone(),
            fetched_at: now,
            last_used_at: now,
        });
        Ok(rules)
    }

    /// Rules validated within the TTL, refreshed now when older, never stale
    /// (admission).
    pub(super) async fn within_ttl(
        &self,
        reader: &SpotRestReader,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, SpotRulesError> {
        let pair = symbol.as_pair();
        if let Some(rules) = self.within(&pair, TRADING_RULES_TTL) {
            return Ok(rules);
        }
        let _gate = self.gate().await;
        if let Some(rules) = self.within(&pair, TRADING_RULES_TTL) {
            return Ok(rules);
        }
        let floor = if self.entries().contains_key(&pair) {
            RULES_REFRESH_RETRY_INTERVAL
        } else {
            RULES_LOOKUP_RETRY_INTERVAL
        };
        self.failed_within(&pair, floor)?;
        self.fetch_and_store(reader, symbol).await
    }

    /// As [`Self::within_ttl`], but a failed refresh serves the last
    /// validated rules, with a warning.
    pub(super) async fn cached(
        &self,
        reader: &SpotRestReader,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, SpotRulesError> {
        match self.within_ttl(reader, symbol).await {
            Ok(rules) => Ok(rules),
            Err(error) => match self.last_validated(symbol) {
                Some(stale) => {
                    warn!(symbol = %symbol.as_pair(), %error, "Spot trading-rules refresh failed; serving stale snapshot");
                    Ok(stale)
                },
                None => Err(error),
            },
        }
    }

    /// For a preview: fresh cached rules, or a read that touches no entry.
    pub(super) async fn preview(
        &self,
        reader: &SpotRestReader,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, SpotRulesError> {
        let pair = symbol.as_pair();
        let cached = self
            .entries()
            .get(&pair)
            .filter(|c| c.fetched_at.elapsed() < TRADING_RULES_TTL)
            .map(|c| c.rules.clone());
        match cached {
            Some(rules) => Ok(rules),
            None => Ok(reader.symbol_rules(symbol).await?.trading_rules(symbol)?),
        }
    }

    /// The last validated rules of any age, marked as used. No request.
    pub(super) fn last_validated(&self, symbol: &Symbol) -> Option<SymbolTradingRules> {
        let mut entries = self.entries();
        let cached = entries.get_mut(&symbol.as_pair())?;
        cached.last_used_at = Instant::now();
        Some(cached.rules.clone())
    }

    /// Whether the rules are missing or past their TTL.
    pub(super) fn refresh_due(&self, symbol: &Symbol) -> bool {
        self.entries()
            .get(&symbol.as_pair())
            .is_none_or(|c| c.fetched_at.elapsed() >= TRADING_RULES_TTL)
    }

    /// Re-read after a rules rejection, within the thirty-second floors.
    pub(super) async fn refresh_after_rejection(
        &self,
        reader: &SpotRestReader,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, SpotRulesError> {
        let pair = symbol.as_pair();
        let _gate = self.gate().await;
        if let Some(rules) = self.within(&pair, RULES_REJECTION_REFRESH_MIN_INTERVAL) {
            return Ok(rules);
        }
        self.failed_within(&pair, RULES_REJECTION_REFRESH_MIN_INTERVAL)?;
        self.fetch_and_store(reader, symbol).await
    }

    /// The whole venue rules last validated for `symbol`. No request.
    pub(super) fn spot_rules(&self, symbol: &Symbol) -> Option<SpotSymbolRules> {
        self.entries().get(&symbol.as_pair()).map(|c| c.spot.clone())
    }

    /// Age the entry of `pair` by `by`, for the TTL tests.
    #[cfg(test)]
    pub(super) fn age_entry(&self, pair: &str, by: Duration) {
        self.entries().get_mut(pair).unwrap().fetched_at = Instant::now().checked_sub(by).unwrap();
    }

    /// Age the entry of `pair` by `by` when there is one.
    #[cfg(test)]
    pub(super) fn try_age_entry(&self, pair: &str, by: Duration) -> bool {
        let mut entries = self.entries();
        let Some(cached) = entries.get_mut(pair) else {
            return false;
        };
        cached.fetched_at = Instant::now().checked_sub(by).unwrap();
        true
    }

    /// Age the failure of `pair` by `by` when there is one.
    #[cfg(test)]
    pub(super) fn try_age_failure(&self, pair: &str, by: Duration) -> bool {
        let mut failures = self.failures();
        let Some(at) = failures.get_mut(pair) else {
            return false;
        };
        *at = Instant::now().checked_sub(by).unwrap();
        true
    }

    /// Age the failure of `pair` by `by`, for the floor tests.
    #[cfg(test)]
    pub(super) fn age_failure(&self, pair: &str, by: Duration) {
        *self.failures().get_mut(pair).unwrap() = Instant::now().checked_sub(by).unwrap();
    }

    /// The concurrency counters.
    #[cfg(test)]
    pub(super) fn probe(&self) -> &Probe {
        &self.probe
    }

    /// How many symbols are cached and how many failures are remembered.
    #[cfg(test)]
    pub(super) fn sizes(&self) -> (usize, usize) {
        (self.entries().len(), self.failures().len())
    }
}

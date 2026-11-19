//! Dormant positive-only selection for one independently verified owner.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use super::{receipt_elapsed, ClaimResult, ColdReceipt, ReceiptState, VerifiedHistory};
use crate::{
    balance_costs::{
        age::sealed::Sealed, exact::ExactDecimal, select_costs, AgeError, AgeInterval,
        CostAgeAuthority, CostError, CostIdentity, CostTerms, DatedOverrideEvidence,
        EvidenceIdentity, VenueCostEvidence,
    },
    venue_clock::{ClockError, NativeClock, TickCapture},
};

struct Key {
    identity: CostIdentity,
    outcome: Option<ClaimResult>,
    receipt: ReceiptState,
}

struct Shared {
    clock: Arc<NativeClock>,
    keys: Mutex<Option<BTreeMap<String, Key>>>,
}

/// Holds the lifetime of one locally verified generation. The lifecycle has
/// no replacement path. Its readers stay closed after this owner retires.
pub struct PreparedFeeOwner {
    shared: Arc<Shared>,
}

#[derive(Clone)]
pub struct PreparedFeeSelection {
    shared: Arc<Shared>,
}

impl PreparedFeeOwner {
    /// Consume native history verified inside this crate. Nothing in this
    /// constructor opens entry or makes an old claim eligible for a request.
    pub fn prepare(history: VerifiedHistory, clock: Arc<NativeClock>) -> Result<Self, AgeError> {
        let VerifiedHistory { entries, expected, domain } = history;
        if clock.capture().map_err(|_| AgeError::Unknown)?.domain() != domain {
            return Err(AgeError::Unknown);
        }
        let account = expected.first().ok_or(AgeError::Identity)?;
        if expected.iter().any(|identity| {
            identity.tenant() != account.tenant()
                || identity.venue() != account.venue()
                || identity.scope() != account.scope()
                || identity.uid() != account.uid()
        }) {
            return Err(AgeError::Identity);
        }
        let mut expected: BTreeMap<_, _> = expected
            .into_iter()
            .map(|identity| (identity.symbol().as_pair(), identity))
            .collect();
        let mut keys = BTreeMap::new();
        for loaded in entries {
            let symbol = loaded.key.symbol;
            let identity = expected.remove(&symbol).ok_or(AgeError::Identity)?;
            keys.insert(
                symbol,
                Key {
                    identity,
                    outcome: loaded.prior.and_then(|claim| claim.result),
                    receipt: loaded.receipt,
                },
            );
        }
        if !expected.is_empty() {
            return Err(AgeError::Identity);
        }
        Ok(Self {
            shared: Arc::new(Shared { clock, keys: Mutex::new(Some(keys)) }),
        })
    }

    pub fn selection(&self) -> PreparedFeeSelection {
        PreparedFeeSelection { shared: Arc::clone(&self.shared) }
    }
}

impl Drop for PreparedFeeOwner {
    fn drop(&mut self) {
        // A poisoned slot is already closed to every reader. In the normal
        // path retire under the same bounded in-memory lock as selection.
        if let Ok(mut keys) = self.shared.keys.lock() {
            *keys = None;
        }
    }
}

impl PreparedFeeSelection {
    /// Read exact immutable terms for the configured account and symbol.
    /// Holding the view across native time capture and hashing linearizes a
    /// successful result with owner retirement. This path performs no I/O
    /// beyond the native clock read, and does not wait on a contended slot.
    pub fn select(
        &self,
        identity: &CostIdentity,
        gap: ExactDecimal,
    ) -> Result<CostTerms, CostError> {
        let slot = self.shared.keys.try_lock().map_err(|_| AgeError::Unknown)?;
        let keys = slot.as_ref().ok_or(AgeError::Unresolved)?;
        let key = keys.get(&identity.symbol().as_pair()).ok_or(AgeError::Identity)?;
        if key.identity != *identity {
            return Err(AgeError::Identity.into());
        }
        let receipt = match &key.receipt {
            ReceiptState::Current(receipt) => receipt,
            ReceiptState::Stale => return Err(CostError::Stale),
            ReceiptState::Unproven => {
                return Err(match key.outcome {
                    Some(ClaimResult::PolicyMismatch) => CostError::FeePolicyMismatch,
                    Some(ClaimResult::InvalidPayload) => CostError::Identity,
                    _ => AgeError::Unresolved.into(),
                })
            },
        };
        let authority = ReceiptAge { clock: &self.shared.clock, receipt };
        select_costs(
            &authority,
            identity,
            VenueCostEvidence::Available(&receipt.candidate),
            DatedOverrideEvidence::Absent,
            gap,
        )
    }
}

struct ReceiptAge<'a> {
    clock: &'a NativeClock,
    receipt: &'a ColdReceipt,
}

fn age_at(receipt: &ColdReceipt, now: TickCapture) -> Result<AgeInterval, AgeError> {
    let elapsed = receipt_elapsed(now, &receipt.original, receipt.observed).map_err(|error| {
        if matches!(error, ClockError::Reversed) {
            AgeError::Future
        } else {
            AgeError::Unknown
        }
    })?;
    Ok(AgeInterval { minimum: elapsed, maximum: elapsed })
}

impl Sealed for ReceiptAge<'_> {}

impl CostAgeAuthority for ReceiptAge<'_> {
    fn age(&self, evidence: &EvidenceIdentity) -> Result<AgeInterval, AgeError> {
        if self.receipt.evidence() != evidence {
            return Err(AgeError::Identity);
        }
        let now = self.clock.capture().map_err(|_| AgeError::Unknown)?;
        age_at(self.receipt, now)
    }

    fn confirm_absence(&self, _: &CostIdentity) -> Result<(), AgeError> {
        Err(AgeError::Unresolved)
    }
}

#[cfg(test)]
mod tests {
    use proptest::{
        collection::vec,
        prelude::*,
        test_runner::{Config, RngSeed, TestRunner},
    };
    use robson_domain::Symbol;
    use uuid::Uuid;

    use super::*;
    use crate::{
        balance_costs::{
            native_n5::{tick, FrequencyKey, LoadedKey, DAY_NS},
            retained_n5::encode_retained,
        },
        venue::VenueId,
    };

    const FEES: &[u8] = br#"{"symbol":"ETHUSDT","standardCommission":{"maker":"0.001","taker":"0.002","buyer":"0","seller":"0"},"specialCommission":{"maker":"0","taker":"0","buyer":"0","seller":"0"},"taxCommission":{"maker":"0","taker":"0","buyer":"0","seller":"0"},"discount":{"enabledForAccount":false,"enabledForSymbol":true,"discountAsset":"BNB","discount":"0.25"}}"#;

    fn account(uid: u64) -> CostIdentity {
        CostIdentity::new(
            Uuid::from_u128(1),
            VenueId::new(VenueId::BINANCE_SPOT),
            "native-n5-fixture".to_owned(),
            uid,
            Symbol::from_pair("ETHUSDT").unwrap(),
        )
        .unwrap()
    }

    fn receipt(clock: &NativeClock, identity: &CostIdentity) -> ColdReceipt {
        let handoff = clock.capture().unwrap();
        let original = clock.wall_bracket(Uuid::from_u128(2)).unwrap();
        let encoded = encode_retained(FEES, original, identity, handoff).unwrap();
        ColdReceipt {
            original,
            observed: encoded.observed,
            candidate: encoded.candidate,
            payload_digest: [0; 32],
            age: std::time::Duration::ZERO,
        }
    }

    fn history(clock: &NativeClock, identity: &CostIdentity) -> VerifiedHistory {
        VerifiedHistory {
            entries: vec![LoadedKey {
                key: FrequencyKey {
                    symbol: identity.symbol().as_pair(),
                    initialization_id: Uuid::from_u128(8),
                    recovery_generation: 1,
                    registration_id: Uuid::from_u128(9),
                },
                prior: None,
                receipt: ReceiptState::Current(receipt(clock, identity)),
            }],
            expected: vec![identity.clone()],
            domain: clock.initial_capture().domain(),
        }
    }

    #[test]
    fn selection_age_uses_the_original_bracket_even_at_strict_day_boundary() {
        let clock = NativeClock::initialize().unwrap();
        let original = receipt(&clock, &account(7));
        let mark = original.original;
        let first = original.observed.nanoseconds();
        let now = |elapsed| tick(mark.domain, first + elapsed).unwrap();
        assert!(age_at(&original, now(DAY_NS - 1)).unwrap().fresh().unwrap());
        assert!(
            !age_at(&original, now(DAY_NS)).unwrap().fresh().unwrap(),
            "exactly 24h must fail warm selection"
        );
        let before = tick(mark.domain, u128::try_from(mark.after_ns).unwrap() - 1).unwrap();
        assert!(matches!(age_at(&original, before), Err(AgeError::Future)));
        assert_eq!(
            ReceiptAge { clock: &clock, receipt: &original }.confirm_absence(&account(7)),
            Err(AgeError::Unresolved),
        );
    }

    #[test]
    fn generated_retirement_never_reopens_a_retained_selection() {
        let clock = Arc::new(NativeClock::initialize().unwrap());
        let identity = account(7);
        let gap = "1.25".parse().unwrap();
        let config = Config {
            cases: 32,
            failure_persistence: None,
            rng_seed: RngSeed::Fixed(20261119),
            ..Config::default()
        };
        TestRunner::new(config)
            .run(&vec(0_u8..4, 4..32), |events| {
                let mut owner = Some(
                    PreparedFeeOwner::prepare(history(&clock, &identity), Arc::clone(&clock))
                        .unwrap(),
                );
                let selection = owner.as_ref().unwrap().selection();
                let original = selection.select(&identity, gap).unwrap();
                for event in events {
                    match event {
                        0 => {
                            drop(owner.take());
                        },
                        1 if owner.is_some() => {
                            let terms = selection.clone().select(&identity, gap).unwrap();
                            prop_assert_eq!(terms.hash(), original.hash());
                        },
                        2 if owner.is_some() => {
                            let slot = &owner.as_ref().unwrap().shared.keys;
                            let _held = slot.lock().unwrap();
                            let result = std::thread::scope(|thread| {
                                thread.spawn(|| selection.select(&identity, gap)).join().unwrap()
                            });
                            prop_assert!(matches!(result, Err(CostError::Age(AgeError::Unknown))));
                        },
                        3 if owner.is_some() => {
                            prop_assert!(matches!(
                                selection.select(&account(9), gap),
                                Err(CostError::Age(AgeError::Identity))
                            ));
                        },
                        _ => {},
                    }
                    if owner.is_none() {
                        prop_assert!(matches!(
                            selection.select(&identity, gap),
                            Err(CostError::Age(AgeError::Unresolved))
                        ));
                    }
                }
                drop(owner);
                prop_assert!(matches!(
                    selection.select(&identity, gap),
                    Err(CostError::Age(AgeError::Unresolved))
                ));
                Ok(())
            })
            .unwrap();
    }
}

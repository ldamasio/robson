//! Pure 8k-2 classification of one qualified attempt (v16 §3.10). Nothing is
//! zero-filled: what the ledger cannot establish is Incomplete.
use std::collections::HashSet;

use robson_connectors::binance::spot::SpotAccount;
use robson_exec::{
    balance_liability::{partition_facts, FactError, FeeEvidence, RecordedPartition, Remainder},
    venue_ledger::{exact_add, exact_sub},
};
use rust_decimal::Decimal;

use super::{
    Attribution, Consequence, DetectorScope, Finding, IncompleteReason, IntentRow, LegRow,
    LinkedOrder, Listing, PreSnapshot, ScanScope,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Classification {
    Clean,
    Discrepancy(Vec<Finding>),
    Incomplete(IncompleteReason),
}

/// Every finding's consequence. No variant sells or closes anything.
pub(crate) fn consequence(finding: &Finding) -> Consequence {
    match finding {
        Finding::BaseExcess { .. }
        | Finding::LockedForeign { .. }
        | Finding::BaseDeficit { .. }
        | Finding::OperatedOrderList { .. } => Consequence::FenceAndPage,
        Finding::LockedShort { .. } | Finding::NeedsExactEvidence { .. } => {
            Consequence::ExactLookup
        },
        Finding::UnlinkedOperatedOrder { .. } => Consequence::StrictRecovery,
        Finding::WithinClaims { .. } => Consequence::Hold,
    }
}

/// The interval `[Σ G + W_pool, Σ (Q − consumed) + W_pool]` plus open entry
/// claims `[0, requested]`. W_pool is 0 because no leg may be closed.
#[derive(Default)]
struct Bounds {
    lower: Decimal,
    upper: Decimal,
    claims: Decimal,
    claimed: bool,
}

type Step<T> = Result<T, IncompleteReason>;

fn add(left: Decimal, right: Decimal) -> Step<Decimal> {
    exact_add(left, right).map_err(|_| IncompleteReason::Unrepresentable)
}

fn sub(left: Decimal, right: Decimal) -> Step<Decimal> {
    exact_sub(left, right).map_err(|_| IncompleteReason::Unrepresentable)
}

/// The accepted exact ledger linkage (c8 attribution): owner `leg`, the kind
/// mapping, the bound tenant/venue/scope, the intent's own client and
/// position, sent, and no definite failure.
fn ledger_backed<'a>(
    scope: DetectorScope<'_>,
    intent: &'a IntentRow,
) -> Option<Step<&'a LinkedOrder>> {
    let linked = intent.linked.as_ref()?;
    let e = scope.expected;
    let kinds = matches!(
        (intent.kind.as_str(), linked.kind.as_deref()),
        ("entry", Some("entry")) | ("protection", Some("stop")) | ("exit", Some("sell"))
    );
    let exact = linked.owner == "leg"
        && kinds
        && linked.tenant_id == e.tenant_id
        && linked.venue_id == e.venue.as_str()
        && linked.account_scope == e.account_scope
        && linked.client_order_id == intent.client_order_id
        && linked.position_id.is_some()
        && linked.position_id == intent.position_id
        && !intent.not_sent
        && matches!(intent.venue_outcome.as_deref(), None | Some("success"));
    Some(if exact {
        Ok(linked)
    } else {
        Err(IncompleteReason::AttributionContradiction)
    })
}

/// One open leg's contribution, judged from its own row and its current
/// admission intent (8k-plan §5 for an unknown entry).
fn leg(
    scope: DetectorScope<'_>,
    row: &LegRow,
    admission: Option<&IntentRow>,
    bounds: &mut Bounds,
) -> Step<()> {
    if matches!(
        row.recovery_state.as_deref(),
        Some("unresolved" | "unresolved_locked" | "cancel_uncertain")
    ) {
        return Err(IncompleteReason::RecoveryState);
    }
    if row.entry_orders > 1 || !row.entry_linked {
        return Err(IncompleteReason::AttributionContradiction);
    }
    match (row.gross, row.base_fee, row.executable) {
        (Some(gross), Some(base_fee), Some(owned)) => {
            // The recorded entry is the leg's; its admission's linked order
            // must still pass the exact linkage.
            if let Some(Err(reason)) = admission.and_then(|intent| ledger_backed(scope, intent)) {
                return Err(reason);
            }
            let fee_evidence = if row.base_fee_final {
                FeeEvidence::FinalCommission
            } else {
                FeeEvidence::ProvisionalBound
            };
            let facts = partition_facts(RecordedPartition {
                gross,
                base_fee,
                owned,
                consumed: row.consumed,
                fee_evidence,
            })
            .map_err(|error| match error {
                FactError::Unrepresentable => IncompleteReason::Unrepresentable,
                _ => IncompleteReason::Malformed,
            })?;
            let Remainder::Quantity(remainder) = facts.remainder() else {
                return Err(IncompleteReason::Deficit);
            };
            bounds.lower = add(bounds.lower, remainder)?;
            bounds.upper = add(bounds.upper, sub(gross, row.consumed)?)?;
            Ok(())
        },
        (None, None, None) => entry_claim(scope, row, admission, bounds),
        _ => Err(IncompleteReason::Malformed),
    }
}

/// A leg with no partition, by its entry evidence: cases (a)–(f).
fn entry_claim(
    scope: DetectorScope<'_>,
    row: &LegRow,
    admission: Option<&IntentRow>,
    bounds: &mut Bounds,
) -> Step<()> {
    let admission = admission.ok_or(IncompleteReason::Malformed)?;
    let mut claim = || {
        let requested = row.requested.ok_or(IncompleteReason::Malformed)?;
        bounds.claims = add(bounds.claims, requested)?;
        bounds.claimed = true;
        Ok(())
    };
    match ledger_backed(scope, admission) {
        Some(linked) => {
            let linked = linked?;
            match linked.phase.as_str() {
                // (a) a ledger-backed entry still live or unknown.
                "live" | "unknown" => claim(),
                // (d) the EP-1 unfilled path: [0, 0].
                "terminal" if linked.executed.is_zero() => Ok(()),
                // (e) executed but no partition: EP-1 refused or quote unknown.
                "terminal" => Err(IncompleteReason::UndefinedPartition),
                _ => Err(IncompleteReason::Malformed),
            }
        },
        None if admission.not_sent => Ok(()),
        None if matches!(
            admission.venue_outcome.as_deref(),
            Some("rejected" | "gone_terminal")
        ) =>
        {
            Ok(())
        },
        None if admission.disposition_kind.is_some() => Err(IncompleteReason::UncertainIntent),
        // (b) sent with no definite outcome, (c) success with no order row yet.
        None => claim(),
    }
}

/// Every candidate that is not an open leg's admission intent.
fn intent(scope: DetectorScope<'_>, intent: &IntentRow) -> Step<()> {
    if intent.kind == "cancel" {
        // Candidates are only cancels with no definite outcome.
        return match intent.target_terminal_proof {
            Some(true) => Ok(()),
            _ => Err(IncompleteReason::UncertainCancel),
        };
    }
    match ledger_backed(scope, intent) {
        Some(linked) => match linked?.phase.as_str() {
            "terminal" => Ok(()),
            // A live stop counts through its order row; a live sell or any
            // unknown phase is in flight.
            "live" if intent.kind == "protection" => Ok(()),
            _ => Err(IncompleteReason::InFlightOrder),
        },
        None => Err(IncompleteReason::UncertainIntent),
    }
}

/// Classifies one qualified attempt's pre rows, ACCOUNT body and listing.
pub(crate) fn classify(
    scope: DetectorScope<'_>,
    pre: &PreSnapshot,
    account: &SpotAccount,
    listing: &Listing,
) -> Classification {
    verdict(judge(scope, pre, account, Some(listing)))
}

/// The ACCOUNT-only classification of O1/O2 (8k-3a, PLAN-r2 §6.7): the base
/// and locked-base checks alone, Incomplete exactly as [`classify`]. Clean
/// only with no open claim and the locked base equal to Robson's live stop
/// remainders. Orders and lists are not visible to an ACCOUNT read and are
/// not claimed to be checked.
pub(crate) fn classify_account(
    scope: &ScanScope<'_>,
    pre: &PreSnapshot,
    account: &SpotAccount,
) -> Classification {
    verdict(judge(scope.detector, pre, account, None))
}

fn verdict(judged: Step<Vec<Finding>>) -> Classification {
    match judged {
        Ok(findings) if findings.is_empty() => Classification::Clean,
        Ok(findings) => Classification::Discrepancy(findings),
        Err(reason) => Classification::Incomplete(reason),
    }
}

/// `listing` is `None` for an ACCOUNT-only classification: no order is
/// listed and no stop is judged missing from a listing.
fn judge(
    scope: DetectorScope<'_>,
    pre: &PreSnapshot,
    account: &SpotAccount,
    listing: Option<&Listing>,
) -> Step<Vec<Finding>> {
    let mut balances = account.balances.iter().filter(|b| b.asset == scope.base_asset);
    let (Some(balance), None) = (balances.next(), balances.next()) else {
        return Err(IncompleteReason::MalformedAccount);
    };
    if balance.free < Decimal::ZERO || balance.locked < Decimal::ZERO {
        return Err(IncompleteReason::MalformedAccount);
    }
    let holdings = add(balance.free, balance.locked)?;
    if pre.closed_legs != 0 {
        return Err(IncompleteReason::ClosedLegWithoutWriteOff);
    }

    let mut bounds = Bounds::default();
    for row in &pre.legs {
        let admission = pre.intents.iter().find(|i| i.admission_of == Some(row.position_id));
        leg(scope, row, admission, &mut bounds)?;
    }

    // Robson's sell-capable orders by their own records.
    let mut robson_locked = Decimal::ZERO;
    let mut live_stops = Vec::new();
    for order in &pre.orders {
        match (order.kind.as_str(), order.phase.as_str()) {
            ("stop", "live") => {
                let remainder = sub(order.orig_qty, order.executed)?;
                if remainder < Decimal::ZERO {
                    return Err(IncompleteReason::Malformed);
                }
                robson_locked = add(robson_locked, remainder)?;
                live_stops.push(order.exchange_order_id.as_str());
            },
            _ => return Err(IncompleteReason::InFlightOrder),
        }
    }

    for candidate in pre.intents.iter().filter(|i| i.admission_of.is_none()) {
        intent(scope, candidate)?;
    }

    let mut findings = Vec::new();
    let mut listed = HashSet::new();
    for order in listing.iter().flat_map(|listing| &listing.orders) {
        match order.attribution {
            Attribution::Ledger { .. } => {},
            Attribution::Absent | Attribution::Discovered | Attribution::Unauthored => {
                findings.push(Finding::UnlinkedOperatedOrder { order_id: order.order_id });
            },
            Attribution::Contradiction => return Err(IncompleteReason::AttributionContradiction),
        }
        listed.insert(order.order_id.to_string());
    }
    for stop in live_stops.iter().filter(|_| listing.is_some()) {
        if !listed.contains(*stop) {
            findings.push(Finding::NeedsExactEvidence { exchange_order_id: (*stop).to_owned() });
        }
    }
    for list in listing.iter().flat_map(|listing| &listing.lists) {
        findings.push(Finding::OperatedOrderList { order_list_id: list.order_list_id });
    }

    let upper = add(bounds.upper, bounds.claims)?;
    if holdings < bounds.lower {
        findings.push(Finding::BaseDeficit { holdings, lower: bounds.lower });
    } else if holdings > upper {
        findings.push(Finding::BaseExcess { holdings, upper });
    } else if bounds.claimed {
        findings.push(Finding::WithinClaims { holdings, lower: bounds.lower, upper });
    }
    if balance.locked > robson_locked {
        findings.push(Finding::LockedForeign {
            locked: balance.locked,
            robson: robson_locked,
        });
    } else if balance.locked < robson_locked {
        findings.push(Finding::LockedShort {
            locked: balance.locked,
            robson: robson_locked,
            stops: live_stops.iter().map(|s| (*s).to_owned()).collect(),
        });
    }
    Ok(findings)
}

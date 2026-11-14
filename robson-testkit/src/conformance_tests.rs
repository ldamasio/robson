//! `conformance::check` and `run_table` on synthetic runs (8d plan §2.4,
//! §7.5): what makes a run unacceptable.

use std::cell::RefCell;

use robson_exec::venue::{
    AccountKind, MarketKind, ProtectionKind, ReplacementProtocol, VenueCapabilities,
    VenueFinalityRule,
};

use crate::{
    balance_stub::spot_capabilities,
    conformance::{
        applies, check, run_table, Coverage, Event, Expected, Outcome, Property, Property::*,
        Trace, Verdict, INVENTORY,
    },
};

fn position() -> VenueCapabilities {
    VenueCapabilities {
        market_kind: MarketKind::PerpUsdM,
        account_kind: AccountKind::PositionAccount,
        supports_short: true,
        reduce_only_wire_flag: true,
        protection: ProtectionKind::MarketOnTrigger,
        replacement: ReplacementProtocol::PlaceThenCancel,
        has_funding: false,
        order_finality: VenueFinalityRule::EveryAttemptRejected,
        one_attempt_per_client_id: false,
    }
}

/// Every clause Verified where it applies, NotApplicable elsewhere.
fn table(caps: &VenueCapabilities) -> Vec<Expected> {
    INVENTORY
        .iter()
        .map(|&property| Expected {
            property,
            coverage: if applies(property, caps) {
                Coverage::Verified
            } else {
                Coverage::NotApplicable("not this account kind")
            },
        })
        .collect()
}

fn passed(expected: Expected) -> Outcome {
    if !expected.coverage.runs() {
        return Outcome::not_run(expected);
    }
    Outcome {
        property: expected.property,
        coverage: expected.coverage,
        verdict: Verdict::Pass,
        trace: Trace(vec![Event::State { what: "evidence".into() }]),
    }
}

fn outcomes(table: &[Expected]) -> Vec<Outcome> {
    table.iter().map(|e| passed(*e)).collect()
}

fn refused(caps: &VenueCapabilities, table: &[Expected], outcomes: &[Outcome], expected: &str) {
    let problems = check(caps, table, outcomes).unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains(expected)),
        "{expected:?} not in {problems:?}"
    );
}

fn at(table: &[Expected], property: Property) -> usize {
    table.iter().position(|e| e.property == property).unwrap()
}

#[test]
fn a_complete_run_passes_on_both_account_kinds() {
    for caps in [position(), spot_capabilities()] {
        let table = table(&caps);
        check(&caps, &table, &outcomes(&table)).unwrap();
    }
}

#[test]
fn an_omitted_duplicated_or_unknown_outcome_is_refused() {
    let caps = spot_capabilities();
    let table = table(&caps);
    let mut omitted = outcomes(&table);
    omitted.remove(at(&table, B4));
    refused(&caps, &table, &omitted, "B4 is missing from the outcomes");
    let mut doubled = outcomes(&table);
    doubled.push(passed(table[at(&table, N2)]));
    refused(&caps, &table, &doubled, "N2 is reported 2 times");
    let mut short_table = table.clone();
    short_table.remove(at(&table, A4));
    refused(&caps, &short_table, &outcomes(&table), "A4 is not in the table");
    refused(&caps, &short_table, &outcomes(&table), "the table has 0 entries for A4");
    let mut long_table = table.clone();
    long_table.push(table[at(&table, F1)]);
    refused(&caps, &long_table, &outcomes(&table), "the table has 2 entries for F1");
}

#[test]
fn an_outcome_of_another_coverage_is_refused() {
    let caps = position();
    let mut table = table(&caps);
    let a2 = at(&table, A2);
    table[a2].coverage = Coverage::Scripted;
    let mut run = outcomes(&table);
    run[a2].coverage = Coverage::Verified;
    refused(&caps, &table, &run, "A2 ran as Verified, the table says Scripted");
    run[a2].coverage = Coverage::Characterized;
    refused(&caps, &table, &run, "A2 ran as Characterized, the table says Scripted");
}

#[test]
fn a_failed_unrun_or_unevidenced_scenario_is_refused() {
    let caps = spot_capabilities();
    let table = table(&caps);
    let b5 = at(&table, B5);
    let mut run = outcomes(&table);
    run[b5].verdict = Verdict::Fail(vec!["balances differ".into()]);
    refused(&caps, &table, &run, "B5 failed: balances differ");
    let mut run = outcomes(&table);
    run[b5] = Outcome::not_run(table[b5]);
    refused(&caps, &table, &run, "B5 was not run");
    let mut run = outcomes(&table);
    run[b5].trace = Trace::default();
    refused(&caps, &table, &run, "B5 passed with no evidence");
    let mut run = outcomes(&table);
    run[b5] = Outcome::no_scenario(B5);
    refused(&caps, &table, &run, "B5 has no scenario");
}

/// NotApplicable exactly where the clause does not apply, whatever the
/// table says: a run cannot drop a clause by calling it not applicable.
#[test]
fn not_applicable_follows_the_capabilities() {
    let caps = spot_capabilities();
    let mut table = table(&caps);
    let r2 = at(&table, R2);
    table[r2].coverage = Coverage::NotApplicable("dropped");
    refused(
        &caps,
        &table,
        &outcomes(&table),
        "R2 is classified NotApplicable(\"dropped\"), but on this venue it applies",
    );
    let mut table = self::table(&caps);
    let r1 = at(&table, R1);
    table[r1].coverage = Coverage::Verified;
    refused(
        &caps,
        &table,
        &outcomes(&table),
        "R1 is classified Verified, but on this venue it does not apply",
    );
    let mut table = self::table(&position());
    let p = at(&table, A5p);
    table[p].coverage = Coverage::Pending("not modeled");
    check(&position(), &table, &outcomes(&table)).unwrap();
    let mut run = outcomes(&table);
    run[p] = passed(Expected {
        property: A5p,
        coverage: Coverage::Pending("not modeled"),
    });
    run[p].verdict = Verdict::Pass;
    refused(&position(), &table, &run, "A5p ran though the table does not run it");
}

#[test]
fn run_table_runs_only_what_the_table_runs() {
    let caps = position();
    let mut table = table(&caps);
    let n3b = at(&table, N3b);
    table[n3b].coverage = Coverage::Pending("no control");
    let ran = RefCell::new(Vec::new());
    let outcomes = futures_executor(run_table(&table, |property| {
        ran.borrow_mut().push(property);
        let expected = table.iter().find(|e| e.property == property).copied().unwrap();
        async move { passed(expected) }
    }));
    let ran = ran.into_inner();
    assert!(!ran.contains(&N3b) && !ran.contains(&B1), "{ran:?}");
    assert_eq!(ran.len(), table.iter().filter(|e| e.coverage.runs()).count());
    assert_eq!(outcomes[n3b].verdict, Verdict::NotRun);
    check(&caps, &table, &outcomes).unwrap();
}

/// Poll a future that never waits to completion.
fn futures_executor<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(future)
}

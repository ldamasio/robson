//! PS-2a: the non-trading budget's surface and its closed read weights, on
//! injected ticks (no real time waited).
use robson_exec::venue_clock::{ClockDomain, TickCapture};
use uuid::Uuid;

use super::*;

fn tick(seconds: u64, nanos: i64) -> TickCapture {
    let domain = ClockDomain::new(Uuid::from_u128(7), 0, 0).unwrap();
    TickCapture::new(domain, i64::try_from(seconds).unwrap(), nanos).unwrap()
}

/// The body of `impl <name> {` in production `state.rs`, to its closing brace.
fn impl_body<'a>(source: &'a str, name: &str) -> &'a str {
    let open = source.find(&format!("impl {name} {{")).expect("impl block") + name.len() + 7;
    let mut depth = 1;
    for (at, byte) in source[open..].bytes().enumerate() {
        depth += match byte {
            b'{' => 1,
            b'}' => -1,
            _ => 0,
        };
        if depth == 0 {
            return &source[open..open + at];
        }
    }
    panic!("unclosed impl {name}");
}

/// PLAN §6.4: the newtype exposes TIME and the closed reads only; nothing
/// it offers can carry an order, a protective request, an emergency
/// allocation or a weight its caller chooses.
#[test]
fn the_non_trading_budget_has_no_order_path() {
    let source = include_str!("state.rs");
    let production = &source[..source.find("#[cfg(test)]\n#[path = \"state_tests.rs\"]").unwrap()];
    let body = impl_body(production, "NonTradingBudget");
    let methods: Vec<&str> = body
        .match_indices("fn ")
        .map(|(at, _)| {
            let name = &body[at + 3..];
            &name[..name.find(['(', '<']).unwrap()]
        })
        .collect();
    assert_eq!(
        methods,
        [
            "start",
            "time_owner",
            "try_reserve_time",
            "reserve_read_now",
            "poll_read_first",
            "poll_send",
            "usage",
            "try_reserve_read"
        ],
        "{body}"
    );
    for forbidden in [
        "order",
        "protective",
        "Protective",
        "emergency",
        "weight:",
        "u32",
        "Class",
    ] {
        assert!(!body.contains(forbidden), "{forbidden} in the non-trading budget: {body}");
    }
    assert!(
        body.contains(".reserve_steady(&mut state, now, deadline, read.weight())"),
        "{body}"
    );
    // Its one field is private and nothing else is implemented for it: no
    // Deref or conversion reaches the shared handle's other methods; the TIME
    // owner's handle (`time_owner`) has its one production caller (T8).
    assert_eq!(production.matches("NonTradingBudget(").count(), 1, "{production}");
    assert!(production.contains("pub(crate) struct NonTradingBudget(SharedBudget);"));
    assert_eq!(production.matches("map(NonTradingBudget)").count(), 1, "{production}");
    assert!(production.contains("Self::new(start, 0, pause, unclean).map(NonTradingBudget)"));
    assert!(!production.contains("for NonTradingBudget"), "{production}");
    // The closed reads' weights are fixed (PLAN §6.7).
    for (read, weight) in [
        (ReadKind::Account, 20),
        (ReadKind::OperatedOpenOrders, 6),
        (ReadKind::OpenOrderLists, 6),
        (ReadKind::ExchangeInfo, 20),
        (ReadKind::Kline, 2),
        (ReadKind::Commission, 20),
    ] {
        assert_eq!(read.weight(), weight, "{read:?}");
    }
}

/// The unknown-history window and the steady 400-per-minute cap hold for
/// the closed reads as for TIME; a dropped reservation is refunded.
#[test]
fn closed_reads_are_steady_and_capped_after_the_window() {
    let budget = SharedBudget::non_trading(tick(0, 0), None, false).unwrap();
    assert!(matches!(
        budget.try_reserve_read(ReadKind::Kline, tick(59, 999_999_999), tick(70, 0)),
        Err(BudgetError::Deferred)
    ));
    assert!(matches!(
        budget.try_reserve_time(tick(59, 999_999_999), tick(70, 0)),
        Err(BudgetError::Deferred)
    ));
    let held: Vec<PendingRead> = (0..20)
        .map(|_| budget.try_reserve_read(ReadKind::Account, tick(61, 0), tick(71, 0)).unwrap())
        .collect();
    assert!(held
        .iter()
        .all(|pending| pending.read() == ReadKind::Account && pending.0.charge.is_some()));
    assert!(matches!(
        budget.try_reserve_read(ReadKind::Kline, tick(61, 0), tick(71, 0)),
        Err(BudgetError::Deferred)
    ));
    assert!(matches!(
        budget.try_reserve_time(tick(61, 0), tick(71, 0)),
        Err(BudgetError::Deferred)
    ));
    drop(held);
    let kline = budget.try_reserve_read(ReadKind::Kline, tick(61, 0), tick(71, 0)).unwrap();
    assert_eq!(kline.read(), ReadKind::Kline);
    assert!(budget.try_reserve_time(tick(61, 0), tick(71, 0)).is_ok());
    assert_eq!(budget.start().unwrap(), tick(0, 0));
}

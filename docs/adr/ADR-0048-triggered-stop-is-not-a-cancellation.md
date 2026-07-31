ADR-0048: A triggered insurance stop is not a cancellation

Status: Accepted
Date: 2026-07-31

Context

Every Active position carries a robsond-authored, reduce-only conditional
stop on the exchange (ADR-0039). When the software monitor decides to exit,
it first cancels that stop, then places the market exit.

`ExchangePort::cancel_stop_market_order` returned `Result<(), ExecError>`.
The Binance adapter treated "unknown order" replies (`-2011`, `-2013`, and
message variants containing "unknown", "not exist", "already") as success
and returned `Ok(())`. The executor therefore could not distinguish two
materially different outcomes:

1. The exchange cancelled a live stop that never executed.
2. The order is gone because it **triggered** and closed the position.

Both produced `Event::InsuranceStopCancelled`, which clears
`insurance_stop_id` in the projection.

Case (2) is not an edge case; it is the normal path whenever price reaches
the stop between two monitor ticks. Recording it as a cancellation has two
consequences, both harmful:

- **The audit trail lies.** The eventlog asserts an operator-invisible
  cancellation that never happened, on a position the exchange actually
  closed on its own.
- **It destroys the evidence for its own recovery.** `insurance_stop_id` is
  the pointer `gather_order_fill_evidence` uses to resolve the real fill via
  `get_stop_order_fill`. Clearing it removes the highest-fidelity evidence
  source at the exact moment reverse reconciliation will need it.

On 2026-07-31 this fired in production. Position
`019fb87a-1c1f-7961-b26d-617ce520ec1d` was closed by its own insurance stop
at 14:04:21.987 UTC. At 14:04:38 the monitor triggered its exit, cancelled a
stop that had already fired, got `-2011`, recorded a cancellation, and
dropped the stop id. The subsequent market exit was rejected with `-2022
ReduceOnly Order is rejected` and retried 352 times, while reverse
reconciliation had no order id to work with and could not close the
position. See `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md` §C.1.

Decision

`cancel_stop_market_order` returns `Result<StopCancelOutcome, ExecError>`:

```rust
pub enum StopCancelOutcome {
    /// The exchange acknowledged the cancel; the stop never executed.
    Cancelled,
    /// The exchange no longer knows the order. It was most likely triggered
    /// and filled.
    AlreadyGone,
}
```

The adapter still tolerates the same error codes, but now reports which
outcome occurred instead of flattening both into success. The decision of
what that means is made where the context exists, not in the adapter.

On `AlreadyGone` the executor:

- does NOT emit `InsuranceStopCancelled`;
- preserves `insurance_stop_id` so reverse reconciliation can resolve the
  real fill;
- returns `ActionResult::Skipped` naming the stop id;
- logs at WARN.

Orphan-stop cancellation in the reconciliation worker accepts either
outcome. An orphan stop is not attached to a tracked position, so the order
being gone is the desired end state regardless of how it got there.

Consequences

Positive

- The eventlog stops asserting cancellations that did not happen.
- The order-fill evidence path keeps its pointer through the exact scenario
  it was designed for, so reverse reconciliation can close the position
  without falling back to user-trade matching.
- The adapter no longer makes a domain decision it lacks the context for.

Negative / Trade-offs

- `insurance_stop_id` now survives on an Active position whose stop no
  longer exists on the exchange. The ADR-0039 startup heal may therefore
  observe a stop id with no live order and re-place the protective stop.
  That is the same behaviour the heal already had for a position whose stop
  vanished for any other reason, and re-placing a protective stop is the
  safe direction, but it is a real behavioural surface worth watching after
  rollout.
- Six `ExchangePort` implementations had to change signature. The churn is
  mechanical and compile-enforced.

Alternatives

- **Keep `Result<(), ExecError>` and stop swallowing the tolerated errors in
  the adapter**, letting the executor classify through its existing
  `is_unknown_order_error` branch. Smaller diff, but it conflates a genuine
  transport or API failure with the expected "already triggered" case, and
  it makes the orphan-cancel path in the reconciliation worker log spurious
  failures for orders that are correctly gone.
- **Query the order before cancelling.** Costs an extra round trip on every
  exit and still races: the stop can trigger between the query and the
  cancel. It does not remove the need to interpret the cancel reply.
- **Emit a distinct `InsuranceStopTriggered` event.** Cleaner semantically,
  but it requires a new event type, a projector route, and a migration for
  a case where the position close event already records the outcome through
  reconciled evidence. Deferred; revisit if the audit trail needs to
  distinguish a triggered stop from a stop that vanished for other reasons.

Implementation Notes

- `robson-exec/src/ports.rs`: `StopCancelOutcome`, trait signature.
- `robson-exec/src/executor.rs`: `execute_cancel_insurance_stop` branch.
- `robsond/src/binance_exchange.rs`: `is_tolerated_algo_cancel_error` now
  maps to `AlreadyGone` instead of `Ok(())`.
- `robsond/src/reconciliation_worker.rs`: orphan cancel accepts either
  outcome.
- Test: `test_cancel_insurance_stop_already_gone_does_not_record_a_cancellation`
  asserts `Skipped` and the absence of `InsuranceStopCancelled`. The stub
  exchange reports `AlreadyGone` for an unknown algo id, which is the same
  answer the real exchange gives for a stop that fired.
- PR: ldamasio/robson#130. Deployed as `sha-d5803c47` on 2026-07-31.
- Related: ADR-0039 (exchange-side insurance stop),
  `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md` §C.1.

Validation status

The `AlreadyGone` branch has not yet executed against the live exchange.
It is covered by unit tests only. The validating event is the first real
stop-out after deploy: the position must close through reverse
reconciliation with reconciled evidence, and the eventlog must contain no
`InsuranceStopCancelled` for it.

---

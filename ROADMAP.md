# Public Roadmap: v2.5 → v2.6 Spot-first

Status as of **2026-11-15**. Robson executes operator-authored trading decisions and manages their risk. **The v2.6 target is Binance Spot, LONG-only. The transition is in progress and is not ready for real capital.** A merged adapter or passing component test does not enable a complete Spot trading path.

[README](README.md) · [Release policy and status](docs/RELEASING.md) · [Venue decision: ADR-0056](docs/adr/ADR-0056-core-risk-vs-venue-adapters.md) · [Internal architecture plan](docs/architecture/v3-migration-plan.md)

## Direction and status

```mermaid
flowchart TD
    A["Current v2.5: Futures-oriented runtime"] --> B["Merged: ports, Spot adapter, tests and inert schema"]
    B --> C["Pending: ledger, clock/cost evidence and admission"]
    C --> D["Pending: inventory, protection and net-from-start accounting"]
    D --> E["Pending: month-boundary snapshots, revisions and open-position carry"]
    E --> F["Pending: integrated acceptance and observation"]
    F --> G["Separate operator action: activation and real capital"]
```

| Milestone | Public status | Evidence and limits |
| --- | --- | --- |
| Execution and risk foundation | Present in the repository | [Governed query engine](robsond/src/query_engine.rs), [risk policy](robson-domain/src/policy.rs) and [current venue factory](robsond/src/venue.rs). Current real venue selection remains USD-M Futures; simulation is separate. |
| Binance Spot adapter and component conformance | Merged components; activation pending | [Spot source and inertness boundary](robson-connectors/src/binance/spot/mod.rs), [Spot wire tests, PR #341](https://github.com/ldamasio/robson/pull/341), and [Futures preservation tests, PR #342](https://github.com/ldamasio/robson/pull/342). Loopback/fake-venue tests are not live Spot acceptance. |
| Durable Spot ledger foundations | Schema/index merged; remaining integration pending | [PR #343](https://github.com/ldamasio/robson/pull/343) adds inert schema and a one-attempt index. It does not introduce the production ledger writer or activate Spot. |
| Complete Spot execution and accounting | In progress | [ADR-0056](docs/adr/ADR-0056-core-risk-vs-venue-adapters.md) defines the required inventory, protection, settlement and acceptance work. The production factory still refuses `binance_spot`. |
| Net-from-start budget and month-boundary carry | Required minimum; integration pending | Account for governed net results and carry open positions through the month boundary using snapshots and revisions. This is part of the minimum before real capital, not post-activation work. See [ADR-0051](docs/adr/ADR-0051-net-from-start-monthly-budget.md) and [ADR-0056](docs/adr/ADR-0056-core-risk-vs-venue-adapters.md). |
| Real-capital Spot operation | Pending; operator-controlled | Requires the gates below and a separate activation decision. No release date or live-capital readiness is promised. |
| Distribution across additional exchanges | Future direction | Venue-neutral contracts make this possible; a second exchange, its market scope and its activation are not approved by this roadmap. Binance is first. |

## Next gates before activation

1. Integrate clock and cost evidence with admission, the durable ledger, inventory and execution paths, with review and required CI checks on the exact candidate. Preserve existing Futures protection and management of open or unsettled exposure during the transition.
2. Demonstrate typed fills, fees and reconciliation, net-from-start accounting and budget reservations, settlement completeness, and open-position carry through month-boundary snapshots and revisions. Unknown outcomes must remain visible and must not become invented execution or accounting evidence.
3. Validate Spot protection, replacement/cancellation, partial fills, residual inventory and restart recovery under the accepted venue constraints. Spot protection is not a claim that Futures `reduce-only` semantics exist on a balance account; the [ADR-0056 trade-offs](docs/adr/ADR-0056-core-risk-vs-venue-adapters.md) remain part of acceptance.
4. Complete integrated database/conformance tests and failure/recovery checks, then the documented observation, cutover and compatible rollback procedures. Component tests alone do not satisfy these gates.
5. Obtain the operator's explicit activation authorization for the selected account and release candidate. Documentation, merges and version labels do not authorize orders or capital allocation.

Initial staged acceptance (T1–T5) precedes expansion. T6 expansion comes only after T1–T5 and the first month boundary after T1; it is not a prerequisite for the first operation and is not authorized by this roadmap.

## Risk and control commitments

The transition preserves the **1% per-trade worst-case cap** and **4% monthly budget** against `capital_base`, budget-metered entry capacity, and technical stops derived from chart analysis rather than a percentage of entry price. Execution costs and venue constraints must be priced into the reviewed risk envelope. See [ADR-0021](docs/adr/ADR-0021-opportunity-detection-vs-technical-stop-analysis.md), [ADR-0039](docs/adr/ADR-0039-exchange-side-insurance-stop.md), [ADR-0043](docs/adr/ADR-0043-budget-metered-entry-admission.md) and [ADR-0051](docs/adr/ADR-0051-net-from-start-monthly-budget.md).

Protection and exits remain safety-critical. Reconciliation requires typed order/trade evidence; an unexplained residual is an open accounting problem, not an automatic adjustment. Robson does not use an LLM to choose trades, originate signals or override risk controls. These are existing constraints, not new policies introduced by the roadmap.

## Reading versions and progress

**v2.5** describes the existing product line; **v2.6** names the planned Spot-first product release. Files named `v3-*` and identifiers such as `MIG-v3#N` are historical internal architecture vocabulary and remain unchanged. They are not a second release announcement.

This roadmap records direction and repository evidence, not a shipping schedule. Package manifests, lockfiles, tags and published releases are unchanged in this documentation update. The first development source snapshot, `v2.6.0-alpha.1`, is planned and has not been published by this documentation update. Cargo and frontend package versions remain `2.5.0`; the adopted [release policy](docs/RELEASING.md) explains this distinction, subsequent alpha/beta/rc tracks and historical release metadata. Technical increments follow the existing review and CI gates; operational activation remains separate.

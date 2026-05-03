# Stop-Aware Entry v4 — Slice 007 Plan (Shadow Decision-Mirror)

**Date:** 2026-05-03
**Status:** PLANNING — design-only, no runtime authorization
**Slice:** 007 — Shadow Decision-Mirror Design
**Branch context:** `stop-aware-shadow-testnet` @ `db6b3a4a`
**Related:**
- [ADR-0035 — Stop-Aware Entry Policy (v4)](../adr/ADR-0035-stop-aware-entry-policy.md)
- [ADR-0021 — Opportunity Detection vs Technical Stop Analysis](../adr/ADR-0021-opportunity-detection-vs-technical-stop-analysis.md)
- [StopQuality Heuristics Spec](2026-04-28-stop-quality-heuristics.md)
- [Stop-Aware Entry Implementation Guide](2026-04-28-stop-aware-entry-implementation-guide.md)
- [Slice 006 Calibration Summary](2026-05-03-stop-aware-slice-006-calibration-summary.md)

---

## 0. Document Scope and Hard Boundaries

This document is a **design-only artifact**. It does not authorize, implement, deploy,
schedule, or stimulate any runtime change. Its purpose is to specify what a Shadow
Decision-Mirror would be, how it should be designed, and what must be validated before
any future implementation slice is authorized.

**Hard boundaries for Slice 007 (this slice does NOT do these things):**

- No `.rs` edits.
- No database migrations.
- No Kubernetes / GitOps changes.
- No new container image build or release.
- No new testnet stimulus.
- No frontend work.
- No production rollout.
- No boost application (shadow or real).
- No change to `RiskEngine`.
- No change to `TechnicalStopDistance`.
- No change to `DetectorSignal` or `EventBus`.
- No change to `StopQualityClassifier` thresholds.
- No use of `StopQuality` for any decision (sizing, approval, rejection).
- No enabling of `Exceptional` anywhere.
- No enabling of `MonthlyHalt` trigger.
- No executable approval path created.
- No projector correction for `entry_approval_pending` handler debt (tracked
  separately — see Section 14).

---

## 1. Summary

Slice 006 closed with six shadow telemetry events covering three `stop_quality_class`
values (None, Good, Premium), two symbols, both sides, and two confidence levels.
Evidence confirms the shadow pipeline is structurally coherent. Evidence is **not
sufficient** to act on StopQuality for any decision-affecting purpose.

The natural next step, recommended in the Slice 006 Calibration Summary, is not to
apply boost but to design a **Shadow Decision-Mirror**: a purely observational layer
that, after each real decision, computes and records the hypothetical delta — what the
decision *would have been* had StopQuality boost been considered. No real decision is
changed. No real state is written. The mirror is a read-only analytical record.

Slice 007 is design-first. It produces a design document (this file) and an acceptance
criteria set. Any future implementation is a separate, explicitly authorized slice.

---

## 2. Goal

Design a Shadow Decision-Mirror that:

1. Answers — for every real entry decision — whether StopQuality would have changed
   candidate classification, score, threshold crossing, or downstream gating.
2. Produces a stable, queryable record of the hypothetical delta between the real
   decision and the StopQuality-informed hypothetical decision.
3. Does so without altering any real runtime path: no decision change, no sizing
   change, no slot change, no RiskEngine input change, no approval path change.
4. Establishes a safe baseline for data-driven evidence about boost impact before
   any boost is ever applied.

The mirror answers questions such as:

- Would StopQuality have changed the candidate classification (None→Good, Good→Premium,
  etc.)?
- Would a hypothetical boost have changed `raw_score` only, or would it have crossed
  an internal threshold?
- Would the result still have been blocked by RiskEngine regardless of boost?
- Would the position still have required `human_confirmation`?
- Would order execution have remained unchanged?
- Would monthly risk/slots have remained unchanged?
- What is the reason the hypothetical outcome did or did not diverge from the real
  outcome?

---

## 3. Non-Goals

Slice 007 explicitly does not:

- Apply boost in any form — shadow or real.
- Change any real decision: entry acceptance, sizing, slot consumption, approval,
  RiskEngine output.
- Change `TechnicalStopDistance`.
- Change `DetectorSignal` or `EventBus` schema.
- Change `StopQualityClassifier` thresholds or scoring.
- Enable `Exceptional` (remains disabled and shadow-only).
- Enable production use of StopQuality.
- Create an executable approval payload.
- Authorize frontend visualization of StopQuality.
- Introduce new projections or eventlog event types (unless a separate ADR explicitly
  scopes this; see Section 9).
- Resolve the `entry_approval_pending` handler projector debt (tracked separately).
- Authorize new testnet stimuli.
- Authorize any capital change.
- Act as a substitute for Slice 008 or any future boost-enabling slice.

---

## 4. Inputs

The mirror requires access to the following inputs, all of which are already emitted
by the shadow pipeline and readable from existing log lines. No new runtime fields are
required for the design phase.

### 4.1 From existing shadow telemetry

| Field | Source |
|-------|--------|
| `position_id` | Shadow telemetry log line |
| `symbol` | Shadow telemetry log line |
| `side` | Shadow telemetry log line |
| `stop_anchor_present` | Shadow telemetry log line |
| `anchor_type` | Shadow telemetry log line |
| `stop_quality_class` | Shadow telemetry log line |
| `raw_score` | Shadow telemetry log line |
| `boost_pct` | Shadow telemetry log line (shadow metadata only) |
| `shadow_exceptional` | Shadow telemetry log line |
| `technical_stop_method` | Shadow telemetry log line |
| `technical_stop_confidence` | Shadow telemetry log line |
| `detected_levels_count` | Shadow telemetry log line |

### 4.2 From real decision path (already in existing log/event output)

| Field | Source |
|-------|--------|
| Real entry decision outcome | `AwaitingApproval` / `RiskChecked` / rejection event |
| Real `slots_available` state | `policy.rs` runtime output |
| Real `human_confirmation` gate | `entry_policy_mode` config |
| Real `MonthlyHalt` state | `circuit_breaker.rs` |
| Real RiskEngine outcome | `risk.rs` rejection/pass |

### 4.3 Computed by the mirror (hypothetical only)

| Field | Derivation |
|-------|-----------|
| `boosted_score_shadow` | `raw_score` × (1 + `boost_pct`) — never applied |
| `threshold_crossed_shadow` | whether `boosted_score_shadow` would cross any known scoring threshold |
| `risk_engine_unchanged` | whether RiskEngine output would have been identical with boosted score |
| `sizing_unchanged` | whether position sizing would have changed with boost |
| `decision_delta` | enum: `None` / `ScoreOnly` / `ClassificationChanged` / `ThresholdCrossed` |
| `delta_reason` | human-readable explanation of why delta is or is not present |

---

## 5. Real Decision Path to Mirror

The real decision path Robson executes for a new entry candidate is:

```
Detector emits signal
    → QueryEngine receives ProcessSignal
    → EntryPolicy evaluates candidate (signal_strategy.rs / StrategyRegistry)
    → RiskEngine evaluates risk (risk.rs / RiskGate)
        ↓ passes
    → EntryApprovalPending event written
    → QueryEngine transitions to AwaitingApproval
        ↓ if human_confirmation
    → Operator approval (POST /queries/:id/approve)
        ↓ approved
    → Order execution
        ↓
    → MonthlyHalt / slots updated
```

The mirror observes **after the real decision** and computes what would have been
different if `boost_pct` had been applied to produce `boosted_score_shadow`. The mirror
does not re-run any of the above steps and does not inject into any transition.

Key invariants about what the real path provides and what the mirror cannot change:

| Real-path component | Mirror relationship |
|--------------------|---------------------|
| `TechnicalStopDistance` | Read-only input; mirror never alters |
| `StopQuality` classification | Shadow metadata only; mirror reads, never applies |
| `slots_available()` | Read-only; mirror never consumes a slot |
| `MonthlyHalt` state | Read-only; mirror never triggers or clears |
| `RiskEngine` decision | Authoritative; mirror may note it would have been unchanged |
| `human_confirmation` gate | Unchanged; mirror never creates an approval event |
| Actual order payload | Mirror never accesses or records order payload |
| Entry lifecycle stage | Mirror observes; never advances |

---

## 6. Hypothetical Mirror Model

The mirror applies a two-step computation after observing a real decision:

### Step 1 — Compute hypothetical boosted score

```
boosted_score_shadow = raw_score × (1 + boost_pct)
```

This uses only the shadow metadata already present in the telemetry line. The
`boost_pct` field in existing telemetry is already the hypothetical boost for the
observed `stop_quality_class`. The computation is a multiplication — no re-evaluation
of heuristics is needed.

**Cap note:** `boost_pct` for `Exceptional` is 0.20, but `shadow_exceptional=false` in
all current evidence, so `Exceptional` path will not be triggered. Mirror must enforce:
if `shadow_exceptional=true`, record as `exceptional_ignored=true` and use
`boost_pct=0.15` (Premium cap) for the boosted score computation.

### Step 2 — Compare against real decision

The mirror checks each gate in sequence and records whether the outcome would have
been identical:

| Gate | Mirror check | Field recorded |
|------|-------------|----------------|
| Classification | Did stop_quality_class change from observed? | `stop_quality_class` (same field, already known) |
| Score threshold | Would `boosted_score_shadow` cross any known threshold? | `threshold_crossed_shadow` |
| RiskEngine | Would RiskEngine output have differed? | `risk_engine_unchanged` |
| Entry acceptance | Would EntryPolicy have accepted/rejected differently? | `entry_policy_unchanged` |
| Approval required | Would `human_confirmation` gate have been bypassed? | `approval_required_unchanged` |
| Sizing | Would position sizing have changed? | `sizing_unchanged` |
| Monthly slots | Would slot consumption have differed? | `monthly_slots_unchanged` |
| Execution | Would order execution have differed? | `execution_unchanged` |
| Exceptional flag | Was Exceptional considered and suppressed? | `exceptional_ignored` |

### Step 3 — Compute decision_delta

```
decision_delta =
  if all gates unchanged  → None
  if score changed only   → ScoreOnly
  if classification tier changed → ClassificationChanged
  if any threshold crossed → ThresholdCrossed
  (no real outcome changed regardless of delta value)
```

The `delta_reason` field provides a human-readable explanation for the highest-priority
delta bucket observed, along with the reason no real gate was actually crossed.

---

## 7. Proposed Output Schema

The mirror output record. This is a pure log/report record — no database writes, no
projection writes, no eventlog events unless Section 9B is later authorized.

```
shadow_decision_mirror {
    // Identity
    mirror_version:               String,      // "0.1.0" initially
    source_signal_id:             Uuid,        // DetectorSignal that triggered this decision
    position_id:                  Uuid,

    // Signal context (from shadow telemetry)
    symbol:                       String,      // e.g. "BTCUSDT"
    side:                         String,      // "Long" | "Short"
    stop_quality_class:           String,      // "None" | "Weak" | "Good" | "Premium"
    raw_score:                    i32,
    hypothetical_boost_pct:       f64,         // = boost_pct from telemetry
    hypothetical_boost_cap:       f64,         // production cap (0.15 max unless Exceptional)
    boosted_score_shadow:         f64,         // raw_score × (1 + boost_pct)

    // Real decision (authoritative)
    real_decision:                String,      // "AwaitingApproval" | "Rejected" | "Skipped"
    real_score:                   i32,         // raw_score (boost NOT applied)

    // Hypothetical comparison (read-only)
    mirrored_decision:            String,      // same real_decision enum — unchanged by definition
    decision_delta:               String,      // "None" | "ScoreOnly" | "ClassificationChanged" | "ThresholdCrossed"
    delta_reason:                 String,      // human-readable explanation

    // Gate invariant flags (all expected to be true)
    threshold_crossed_shadow:     bool,        // would boosted_score have crossed any threshold?
    risk_engine_unchanged:        bool,        // true = RiskEngine outcome identical
    entry_policy_unchanged:       bool,        // true = EntryPolicy outcome identical
    approval_required_unchanged:  bool,        // true = human_confirmation gate unchanged
    sizing_unchanged:             bool,        // true = position sizing unchanged
    monthly_slots_unchanged:      bool,        // true = slot consumption unchanged
    execution_unchanged:          bool,        // true = order execution unchanged

    // Exceptional gate
    exceptional_ignored:          bool,        // true if shadow_exceptional=true was suppressed

    // Timestamp
    mirrored_at:                  Timestamp,
}
```

**Note on `mirrored_decision`:** The field is defined but always mirrors `real_decision`
in this design. Its purpose is to make explicit that no override occurred. A future
slice could diverge these fields if a boost-with-override path were ever authorized —
but that authorization requires a separate ADR.

---

## 8. Forbidden Fields and Privacy Constraints

The following fields must **never** appear in any mirror output record, regardless of
storage strategy:

| Forbidden field | Reason |
|----------------|--------|
| API keys or secrets | Security |
| Exchange credentials (Binance API key/secret) | Security |
| `database_url` or connection strings | Security |
| Tenant secrets or tenant identifiers beyond position_id | Privacy |
| Full account balance or equity | Financial sensitivity |
| Raw order payload (price, qty, client_order_id) | Execution sensitivity |
| Human approval tokens | Security |
| Any field that can be used to replay or construct an order | Security |
| `shadow_exceptional=true` used as an active boost input | Policy |

**Acceptable financial aggregates** (already present in existing telemetry and logs):
- `stop_quality_class` and `raw_score` (classification only, not financial value)
- `boost_pct` (percentage multiplier, not financial value)
- `symbol` and `side` (non-sensitive directional metadata)
- `position_id` (UUID, not account identifier)

---

## 9. Storage Strategy Alternatives

Three design alternatives are evaluated below. The recommendation is in Section 10.

### Alternative A — Logs-Only Mirror

**Description:** Mirror output is emitted as a structured log line at `INFO` level via
`tracing`, using the existing `robsond::detector` (or a new `robsond::shadow_mirror`)
target. No database writes. No projection. No eventlog event.

**Example log line:**
```
INFO robsond::shadow_mirror: shadow decision mirror
  position_id=019dec8d-...
  symbol=ETHUSDT
  side=Long
  stop_quality_class=Premium
  raw_score=47
  hypothetical_boost_pct=0.15
  boosted_score_shadow=54.05
  real_decision=AwaitingApproval
  decision_delta=ScoreOnly
  risk_engine_unchanged=true
  sizing_unchanged=true
  monthly_slots_unchanged=true
  execution_unchanged=true
  exceptional_ignored=false
  mirror_version=0.1.0
```

**Pros:**
- Lowest implementation risk
- Zero schema commitment
- Zero projection impact
- Queryable via log search (Loki, grep, etc.)
- Reversible: removing the log line removes the mirror

**Cons:**
- Not queryable by application logic
- Log rotation can lose data
- No structured aggregation without external tooling
- Cannot be trivially surfaced in a future UI

**Risk level:** Minimal. Additive log line in existing log stream.

---

### Alternative B — Eventlog Shadow Event

**Description:** A new event type (`ShadowDecisionMirrored`) is appended to the
eventlog for each decision observation. The mirror record becomes part of the
append-only event stream alongside existing domain events.

**Pros:**
- Full audit trail in the event stream
- Queryable by projections
- Survives log rotation
- Natural fit for event-sourcing architecture

**Cons:**
- **Schema commitment:** Once committed to the append-only log, the event type and
  field set is difficult to evolve or remove.
- Requires a new event variant in `events.rs` (domain boundary crossing).
- Requires handler/projection consideration (and risks mixing with known
  `entry_approval_pending` projector debt).
- Highest coordination cost across robson-domain, robsond, robson-eventlog.
- A design error in the event schema becomes permanent in the log.

**Risk level:** Moderate. Acceptable only if a separate ADR explicitly scopes and
approves the `ShadowDecisionMirrored` event type, its fields, its projection
requirements, and its relationship to existing events.

---

### Alternative C — Projection/Report-Only

**Description:** Mirror output is written to a dedicated shadow-mirror report file or
database table (separate from the main eventlog), aggregated per observation window,
and queryable offline. No runtime writes to the main event stream.

**Pros:**
- Better queryability than logs
- No schema commitment in the main eventlog
- Can be structured as a markdown report (like existing analysis documents) or a
  lightweight append-only flat file

**Cons:**
- Requires a write path (file or DB) that may not be available in the same pod
- Lifecycle/rotation policy needs definition
- More implementation cost than logs-only
- Markdown-based report is the safer subvariant but requires manual generation
  (i.e. it is equivalent to Slice 006 documents, not runtime-generated)

**Risk level:** Low-to-moderate depending on variant. Report-only (markdown, manually
generated from log analysis) is equivalent risk to logs-only. Database-write variant
requires more justification.

---

## 10. Recommended First Implementation Approach

**Recommendation: Logs-Only Shadow Mirror (Alternative A), if authorized.**

Rationale:

1. **Lowest risk path forward.** The shadow telemetry pipeline already emits a
   structured log line. Emitting a second structured log line from the same context
   (after the real decision) requires no schema changes, no event type additions, no
   projection work.

2. **Zero commitment.** If the mirror log line is later deemed insufficient, it can
   be replaced by Alternative B or C without any migration burden. The reverse is not
   true: a premature `ShadowDecisionMirrored` event type in the append-only log is
   difficult to retire.

3. **Sufficient for the current evidence stage.** The goal of Slice 007 is to answer
   "what would have happened" questions. Structured log lines, queryable via Loki or
   grep, are sufficient to answer these questions across the small volume of testnet
   observations currently anticipated.

4. **Consistent with the existing shadow telemetry pattern.** Slice 001–006 telemetry
   is already log-line-based. A mirror log line follows the same pattern.

**If Eventlog is later considered** (Alternative B), it must be preceded by:
- A separate ADR (ADR-0025 or successor) explicitly scoping the
  `ShadowDecisionMirrored` event.
- An explicit acceptance criteria set for the event schema.
- Resolution of the `entry_approval_pending` projector debt (Section 14) to avoid
  compounding unhandled event types.
- Operator sign-off on the schema before the first event is written.

**Report-only variant** (Alternative C, markdown) is an acceptable alternative for
Slice 007 itself: the mirror output is generated manually from log analysis rather
than runtime, following the same pattern as the Slice 006 calibration documents. This
would require no runtime code at all and is the safest option for the design phase.

---

## 11. Invariants

The following invariants must hold in any implementation of the Shadow Decision-Mirror,
regardless of storage strategy:

1. **Real decision is never altered.** The mirror computation occurs after the real
   decision has been recorded. No field in the real decision path is modified.

2. **RiskEngine authority is preserved.** The mirror does not re-evaluate RiskEngine
   logic. `risk_engine_unchanged=true` is the expected value for all current
   observations. A `false` value would indicate a previously unknown threshold
   sensitivity and must trigger an investigation before any boost is authorized.

3. **`boosted_score_shadow` is never applied.** The computed hypothetical score exists
   only in the mirror record. It is not passed to `EntryPolicy`, `RiskEngine`, sizing,
   or any approval path.

4. **`exceptional_ignored=true` when `shadow_exceptional=true`.** If the underlying
   telemetry ever records `shadow_exceptional=true`, the mirror must suppress the
   Exceptional boost tier and cap at `boost_pct=0.15`. The Exceptional flag remains
   disabled.

5. **`monthly_slots_unchanged=true` always.** The mirror does not consume slots. A
   `false` value is a bug.

6. **`execution_unchanged=true` always.** The mirror does not execute orders. A
   `false` value is a bug.

7. **No secrets or credentials in mirror output.** Any implementation must verify that
   the forbidden field list in Section 8 is enforced at the output boundary.

8. **`StopQuality` cannot rescue an invalid signal.** If `EntryPolicy` would have
   rejected the signal regardless of boost, `decision_delta` must not be
   `ThresholdCrossed` for the acceptance gate. It may be `ScoreOnly` if only the
   numeric score would change.

9. **Boost does not authorize.** `boosted_score_shadow` is an analytical quantity.
   No downstream system may treat it as an approval token or authorization signal.

10. **Mirror version must be recorded.** Every mirror record carries `mirror_version`
    to enable future schema evolution without ambiguity.

---

## 12. Acceptance Criteria

The following criteria must be satisfied before Slice 007 can be considered complete
(design phase) and before any future implementation slice can be opened.

### Design acceptance (Slice 007 itself)

- [ ] This document is reviewed and acknowledged by the operator.
- [ ] The proposed output schema (Section 7) is reviewed for forbidden field
      violations (Section 8).
- [ ] At least one worked example of mirror output is produced manually from
      existing Slice 006 telemetry (using the ETHUSDT LONG Premium observation as
      the reference case, since it has the highest `raw_score` and thus the most
      interesting hypothetical delta).
- [ ] The decision_delta taxonomy (None / ScoreOnly / ClassificationChanged /
      ThresholdCrossed) is validated against all six existing Slice 006 telemetry
      events.
- [ ] The storage strategy recommendation (logs-only, Section 10) is accepted or
      explicitly rejected with an alternative selected.
- [ ] If Alternative B (eventlog) is selected, a separate ADR is committed and
      reviewed before any implementation begins.

### Implementation acceptance (future slice, not Slice 007)

- [ ] Implementation emits mirror log line for every shadow telemetry observation.
- [ ] `risk_engine_unchanged=true` holds for all observed events.
- [ ] `execution_unchanged=true` holds for all observed events.
- [ ] `monthly_slots_unchanged=true` holds for all observed events.
- [ ] `exceptional_ignored=true` whenever `shadow_exceptional=true`.
- [ ] No forbidden field appears in any mirror log line.
- [ ] Mirror log lines are queryable from testnet pod logs.
- [ ] A post-implementation analysis document is written covering at least five
      mirror observations across at least two symbols and both sides.
- [ ] All existing tests pass (`cargo test`).
- [ ] No new production approval is issued during the implementation window.

---

## 13. Failure Modes

| Failure mode | Description | Mitigation |
|-------------|-------------|------------|
| Mirror alters real decision | Mirror computation is incorrectly placed before real decision is recorded | Place mirror strictly after real decision path; enforce in code review |
| `boosted_score_shadow` leaks into RiskEngine | Hypothetical score is accidentally passed to a real evaluation path | Mirror struct must be distinct from real evaluation struct; compiler-enforced types |
| Forbidden field in log output | API key or credential appears in mirror log | Pre-commit log output review; log field allowlist in implementation |
| `shadow_exceptional=true` treated as active | Exceptional tier activates when it should be suppressed | Explicit `exceptional_ignored` guard in mirror computation |
| Mirror produces `decision_delta=ThresholdCrossed` on all events | Mirror computation finds that boost would always cross a threshold | If observed, this is evidence about scoring thresholds, not a mirror bug, but must be investigated before any boost is authorized |
| Eventlog schema committed prematurely | Alternative B is chosen before ADR is written | Section 10 requirement: separate ADR before any eventlog write |
| Mirror mixed with projector debt | `entry_approval_pending` handler debt contaminates mirror implementation | Section 14: keep separate; projector debt is not in scope for Slice 007 |

---

## 14. Relationship to Known Projector Debt

During Slice 006 observation, a non-blocking log warning appeared:

```
WARN robsond: no handler for event type entry_approval_pending
```

This indicates that the `entry_approval_pending` event type is written to the eventlog
but has no corresponding read-side projector handler. This is known debt:

- It did not block telemetry collection.
- It did not block `AwaitingApproval` lifecycle transitions.
- It did not execute orders.
- It is not related to the Shadow Decision-Mirror design.

**Slice 007 must not attempt to resolve this debt.** Mixing projector correction with
mirror design would conflate two unrelated concerns and increase the risk of both.

The `entry_approval_pending` projector debt should be tracked as a separate backlog
item (candidate: `QE-P5` projector backlog or a new `PROJ-001` ticket) and addressed
in a dedicated scope where:
- The full projector gap is understood
- The impact on existing read models is assessed
- A migration path for any existing unhandled events is defined

If Alternative B (eventlog shadow event) is ever pursued for the mirror, resolving
this projector debt first is a prerequisite, to avoid compounding the number of
unhandled event types in the stream.

---

## 15. Gates Before Any Implementation

The following gates must be explicitly passed before any implementation of the Shadow
Decision-Mirror runtime is authorized. These are design gates, not calendar gates.

| Gate | Condition |
|------|-----------|
| G1 | This document (Slice 007 plan) is reviewed and accepted by the operator |
| G2 | Worked example: mirror output computed manually from all six Slice 006 events |
| G3 | `decision_delta` taxonomy validated against worked examples |
| G4 | Forbidden field list reviewed and accepted |
| G5 | Storage strategy selected (logs-only recommended; eventlog requires separate ADR) |
| G6 | If eventlog selected: ADR written, reviewed, and accepted before any `.rs` edit |
| G7 | `entry_approval_pending` projector debt is tracked separately and explicitly out of scope |
| G8 | Operator confirms StopQuality boost remains shadow-only after mirror implementation |
| G9 | Operator confirms no production rollout follows directly from mirror implementation |

Gates G1–G5 are within the scope of Slice 007. Gates G6–G9 gate any future
implementation slice.

---

## 16. Recommendation

**Slice 007 is a design-first slice.** Its deliverable is this document and a set of
worked examples demonstrating the mirror computation against existing Slice 006
telemetry.

No runtime implementation is authorized by this document.

If the operator chooses to proceed to implementation after Slice 007 is accepted, the
recommended path is:

1. **Worked examples first** (Section 12 design acceptance) — compute mirror output
   manually for all six Slice 006 events. This validates the schema and delta taxonomy
   without touching code.

2. **Logs-only implementation** — emit a structured mirror log line from the detector
   context, after the real shadow telemetry line, using the output schema in Section 7.
   This requires minimal code change, carries no schema commitment, and is fully
   reversible.

3. **Observation period** — collect mirror log lines across at least five new testnet
   observations covering both symbols and both sides. Produce an analysis document
   confirming all invariants held.

4. **Gates review** — if all invariants held and evidence supports consideration of
   boost, a separate, explicitly authorized slice opens. That slice requires its own
   plan, ADR amendment, and operator sign-off.

**StopQuality remains shadow-only.** RiskEngine remains the final authority. No boost
is authorized. No production rollout is authorized. Exceptional remains disabled.

---

*This document was produced as the Slice 007 planning artifact for the Stop-Aware
Entry v4 research track on branch `stop-aware-shadow-testnet`. It does not authorize
any runtime change.*

# ADR-0044 — REST Market-Data Fallback for the Trailing Engine

**Date**: 2026-07-05
**Status**: Decided (implementation pending)
**Deciders**: RBX Systems (operator + architecture)

---

## Context

On 2026-07-05, after a pod restart at 00:33 UTC, the market-data WebSocket
entered a permanent silent-connection loop: the connection opened successfully
but delivered zero ticks. The read-idle watchdog (90 s, added in the ADR-0039
remediation cycle) correctly detected the silence and reconnected — 45 times
over ~1 h 50 min — but every new connection was equally mute. The connection
opening and staying silent points at a broken subscription or egress problem,
not a network outage.

The consequences split cleanly along the two stop layers of ADR-0039:

- **Bounded loss held.** The exchange-side insurance stop kept resting at the
  entry-time level. Maximum loss stayed capped without the daemon's help.
- **Profit protection froze.** The trailing engine is tick-driven. With no
  ticks, the trailing stop of an active short (BTCUSDT, entry 63,145.20, span
  274.80) stayed at its entry-time level for over an hour after price crossed
  the first advance target (62,870.40 at 01:15 UTC). A reversal during that
  window would have turned +0.7% unrealized into the full ~1% stop-out — the
  June incident class in a new variant: **daemon alive, feed mute**.

An operator-authorized `rollout restart` resolved the instance: startup
recovery replayed the gap candles, advanced the trailing stop two steps into
the locked-profit zone, and cancel-replaced the insurance stop accordingly.
The WS delivered ticks normally on the new pod.

The structural gap remains: **the watchdog can only reconnect**. There is no
alternate data path into the trailing engine. Meanwhile, the Safety Net
already polls the exchange REST API every 20 s — but only for position
reconciliation; it does not drive trailing. The failure was invisible from
the outside: `/status` served a fresh `current_price` (REST) while the
tick pipeline starved.

## Decision

### 1. Degraded market-data mode (REST polling)

When the watchdog declares the feed silent, the market-data layer enters
**REST fallback mode** for every symbol with a risk-open position: it polls
the exchange REST price endpoint at a fixed interval (default 5 s,
operator-configurable via `ROBSON_REST_FALLBACK_POLL_SECS`) and emits the
result into the **same** `MarketData` pipeline the WS feeds, tagged
`source: rest_fallback`.

The trailing engine is unchanged. It consumes prices; it does not know or
care where they came from. Discrete-step trailing, span multiples, buffer,
and guard semantics (ADR-0041/0042) apply identically.

### 2. Mode transitions with hysteresis

- **Enter fallback**: watchdog fires (90 s read-idle). REST polling starts
  immediately; WS reconnection attempts continue in the background unchanged.
- **Exit fallback**: the WS connection delivers ticks again and stays healthy
  for a hold-down window (default 60 s). Only then does REST polling stop.
  The hold-down prevents flapping between modes on a half-healthy socket.
- Both sources may deliver concurrently during transitions. Price events
  carry the exchange event time; the pipeline deduplicates by monotonic
  timestamp per symbol so a step can never be applied twice for the same
  market movement.

### 3. Request budget

REST fallback must fit the exchange rate-limit budget by construction:

- Poll only symbols with an Entering or Active position (the same set the
  Safety Net tracks), never the full watchlist.
- One price request per symbol per interval; no burst retries — a failed poll
  waits for the next interval.
- Request-count telemetry with an alert threshold, so an accidental
  N+1-style regression (per-tick REST calls) is caught by monitoring, not by
  a Binance ban. This applies the workspace engineering guardrails
  (`rbx-agent-layer/rbx-engineering-guardrails.md`) to this component.

### 4. Observability

The June and July incidents were both silent from the outside. This one must
not be:

- `market_data_mode` gauge per symbol (`ws` = 0, `rest_fallback` = 1).
- `market_data_silent_seconds` counter since last tick, per symbol.
- WARN log on every mode transition, with cause.
- Alert when fallback mode persists beyond an operator threshold (default
  15 min) — fallback is a degraded state to leave, not a home.

### 5. Equivalence and concurrency verification

Two properties must hold and be tested as properties, not examples:

- **Source equivalence**: given the same price series, the trailing engine
  reaches the same stop regardless of which source delivered it. Verified
  with property-based tests (`proptest`): random price walks fed as
  WS-tagged, REST-tagged, and interleaved streams must converge to identical
  final stops.
- **No double application**: concurrent delivery from both sources during
  transitions must never advance the stop twice for one movement. Verified
  with randomized interleaving tests hammering the mode switch.

Buffered channels between the poller and the engine are bounded; a stalled
consumer drops the oldest price, never grows the queue (a price feed only
needs the latest value — and an unbounded queue on a stalled consumer is the
canonical slow leak).

### 6. Failure containment

| Failure | Behavior |
| --- | --- |
| WS silent, REST healthy | Fallback drives trailing; WARN + gauge flips |
| WS healthy, REST failing | Normal operation; fallback errors logged, no panic |
| Both paths dead | Loud alert (both-feeds-dead); insurance stop (ADR-0039) remains the bounded-loss floor; startup recovery heals on next boot |
| Daemon dies entirely | Unchanged from ADR-0039: exchange-side stop executes alone |

No code path in the fallback may panic the daemon or block the runtime. A
dead REST poll is a logged, counted event, and nothing else.

### 7. Rollout

1. **robsond first** (this ADR): implement, verify with the property suite,
   deploy behind the config default (fallback enabled).
2. **Strategos next**: this design — silent-feed watchdog, REST degraded
   mode, source-equivalence property tests — is the reference pattern for
   Strategos' market-data layer. A Strategos ADR will adapt it to that
   codebase; the invariants transfer unchanged.

## Consequences

### Positive

- Profit protection no longer depends on a single data path. The "daemon
  alive, feed mute" failure mode degrades to 5 s price granularity instead
  of freezing.
- The degraded state is visible and alertable instead of silent.
- Startup recovery stops being the only self-healing mechanism for feed
  outages — it becomes the second line instead of the first.

### Negative / Trade-offs

- Trailing advances up to one poll interval later in fallback mode than a
  tick would deliver. Accepted: 5 s of latency against an indefinite freeze
  is not a close call, and the discrete-step policy already tolerates
  sub-span movement by design.
- Permanent REST budget consumption while degraded, and more states to test
  (two sources, transitions, interleavings). The property suite is the cost
  of admission.
- This ADR does not diagnose the root cause of the silent WS (suspect
  subscription or egress). That investigation continues independently;
  fixing it does not remove the value of a second path.

## Alternatives considered

### Fix the WebSocket and do nothing else (rejected)

The root cause of the silence is not yet identified, and even a fixed WS is
still a single path. June taught that daemon availability cannot be a
precondition for bounded loss; July taught that feed availability cannot be
a precondition for profit protection.

### Let the Safety Net drive trailing directly (rejected)

The Safety Net is a reconciliation auditor. Feeding its 20 s position poll
into the trailing engine would mix responsibilities and create a second,
subtly different trailing path — exactly the class of divergence the
equivalence property exists to prevent. The fallback reuses the *pipeline*,
not the auditor.

### Candle replay on each reconnect (rejected)

Replaying missed candles on every watchdog reconnect closes each gap but
leaves the system blind between reconnects — under a permanently silent WS
that is a 90 s blind window forever. It also makes the reconnect path do
risk-relevant work under a failure condition, which is where surprises live.

## Related

- [ADR-0039](ADR-0039-exchange-side-insurance-stop.md) — two-layer stop
  enforcement; the watchdog this ADR extends
- [ADR-0041](ADR-0041-executable-stop-buffer.md) — executable stop buffer
- [ADR-0042](ADR-0042-invalidation-guard.md) — invalidation guard semantics
- `rbx-agent-layer/rbx-engineering-guardrails.md` — workspace engineering
  guardrails applied in §3 and §5 (request budget, property-based
  concurrency tests, bounded queues)
- Incident record: 2026-07-05 silent-WebSocket trailing freeze (operator
  runbook)

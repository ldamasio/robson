# ADR-0047 — FE ↔ robsond Communication Topology (Option A, No Change)

**Date**: 2026-07-06
**Status**: Decided
**Deciders**: RBX Systems (operator + architecture)

---

## Context

After the SSE hardening cycle (heartbeat observability, client read-idle
watchdog, LIVE/STALE badge, snapshot resync — PRs #118/#120/#121), the
operator asked whether the downstream link could be simplified or optimized:
"do we need both SSE and WS; can we use RPC or the k3s internal network;
stable and efficient, without weighing on RBX infra or losing security/UX?"

First, the two channels are different links and not redundant:

| Link | Direction | Payload |
| --- | --- | --- |
| WS (+ REST fallback, ADR-0044) | Binance → robsond | market ticks (trailing engine) |
| SSE | robsond → browser | domain events only |

This ADR records the decision for the downstream link. A full decision study
(five options, failure-mode tables, cost at 1 vs 100 users) was produced by a
headless executor and reviewed; its analysis is summarized here.

Measured facts (2026-07-06, production): browser → traefik
(`api.robson.rbx.ia.br`) → robsond; SSE keepalives verified end to end
through traefik (no buffering); volume is 1 operator, 1 connection,
~4 heartbeats/min, a handful of domain events per day. The Bearer token is
sent via the `Authorization` header by the FE's fetch-based SSE client — it
does NOT ride the query string. (ADR-0030 A1.3 describes the older native
`EventSource` limitation; that section is superseded on this point.)

## Decision

**Keep the current topology (Option A): browser → traefik → robsond, REST
snapshots + SSE domain events, Bearer in headers. No RPC, no FE-side proxy,
no transport change.**

Options considered and rejected for now:

- **B — Consolidate behind the FE host** (adapter-node proxy over
  `robsond.robson.svc`, HttpOnly cookie): genuine security win (token out of
  JS), but it forces the adapter-node migration ADR-0030 deferred,
  *introduces* a new silent-failure class (a buffering proxy stacks
  keepalives → watchdog trips → reconnect storm — the §5 worst class,
  created by the change), and gives every FE rollout a blast radius over all
  live streams (today FE restarts do not touch browser→robsond SSE).
- **C — SSR-internal reads only**: pays the full adapter-node cost for half
  the benefit and a dual-auth model.
- **D — RPC (gRPC-web / WS API / tRPC)**: grpc-web requires a proxy — it adds
  infra, the opposite of the ask; gRPC's strengths (typing, bidirectional
  streams, multiplexing) are irrelevant at this event volume; a WS API
  discards SSE's simplicity without being cheaper through traefik.
- **E — Polling only**: defensible at 1 user but degrades the staleness
  signal to an ambiguous timer ("idle or stuck?") — against §5 — and scales
  worse (1,200 req/min at 100 users vs ~100 idle SSE connections). Polling
  remains the right shape only as a degraded *fallback*, mirroring ADR-0044.

We also deliberately do NOT add `Last-Event-ID`/event replay: at a handful of
events per day, the `/status` snapshot on reconnect is the consistency anchor;
replay would add buffer state and a dedup test obligation with no payoff at
this volume.

## Consequences

- Zero new infrastructure; the verified stable path stays untouched.
- The event-log-first UX (ADR-0032) keeps true push latency and the crisp
  heartbeat-driven staleness signal.
- Two public hosts and CORS remain (accepted; configuration, not
  architecture).
- The cookie-based auth model is consciously deferred, not rejected.

## When to revisit (triggers for Option B)

1. **Multi-tenant launch**: per-tenant auth and the obligation not to hand
   every browser a robsond-issued token make FE-brokered auth + HttpOnly
   cookie the correct model; adapter-node then pays for itself.
2. **More than ~20 concurrent operators** (connection scaling, centralized
   rate limiting).
3. **A genuine client→server streaming need** (only then a WS API).

## Process note

The decision study, produced from ADRs alone, recommended "move the token
out of the SSE query string" — a change that was already implemented in the
code. Documentation had drifted from source. The correction was caught in
review; the agnostic lesson (verify recommendations against current source,
not documentation alone) is recorded in the engineering guardrails.

## Related

- ADR-0030 (FE stack; A1.3 superseded on the SSE-auth point)
- ADR-0032 (event-log-first UX)
- ADR-0044 (upstream market-data fallback — the pattern polling follows here)
- `rbx-agent-layer/rbx-engineering-guardrails.md` §5
- Decision study: `~/rbx/scratch/robsond-fe-comms-decision.md` (working copy)

# Robson product sessions: Fable 5.1 review and disposition

Date: 2026-10-20. Scope: architecture and local implementation, no deployment.
Related decision: [ADR-0055](../adr/ADR-0055-product-sessions-web-native-cli.md).

The operator selected remembered personal-device sessions with recent identity
confirmation for sensitive actions, and authorized local implementation only.
Two frozen evidence packs were reviewed through Claude Code 2.1.292 with
`--model claude-fable-5-1`, no tools, MCP, hooks, browser or session persistence.
Both result records identify canonical model `claude-fable-5-1`, provider
`firstParty`. Both verdicts were **approve-with-changes**, not clean approval.
The corrections below were implemented and tested after the second review;
they are the implementer's disposition, not a third model approval.

On 2026-10-20, after the implementation summary and outstanding gates were
presented, the operator explicitly accepted the architectural guidelines. This
records human acceptance of the direction for Robson and the local-only scope;
it does not change either Fable verdict or authorize deployment. Persistent
storage, real-IdP validation, native-client acceptance and production readiness
remain pending as documented in ADR-0055.

| Evidence | Input SHA-256 | Review output SHA-256 |
|---|---|---|
| Architecture pack v1 | `aa1fd9f8d9c85da981e074f80f4d9a255f1f5e16c13cf67729a4ca30865e1461` | `490d8cc086a53d39450a13671d8f559abb2af2f01a1e837e65909641cac3dea2` |
| Architecture and code pack v2 | `9ad5d7ded2daef0f77896150e4258b4910ec69cf217179f556584e6d301f3d47` | `220ce1eafea34eb474ade8fec04c03d0f089321e4876ea4f0e5e2678d8acd264` |

Full review results/manifests are retained as local task artifacts. Pattern
scans of both transmitted packs found no private key, provider key or JWT;
personal email occurrences were removed from the second source pack. This is
not represented as a comprehensive secret scan or the proposed institutional
two-reviewer gate.

## Architectural corrections from the first review

- **Availability:** explicitly choose fail-closed introspection for the sandbox,
  including protective manual operations. Do not conflate a freshness lock with
  IdP availability. Engine and exchange-side stops remain independent. The
  production availability choice remains an operational gate.
- **Recent authentication:** bound Code+PKCE to the current session, request
  fresh login, verify nonce, canonical subject and `auth_time`, rotate the
  session ID and preserve its original absolute deadline.
- **Upstream limits:** product 7/30-day values are upper-bound targets, not a
  claim about configured RBX provider lifetimes. Record actual sandbox values
  before real activation. Native grant capabilities remain unverified.
- **Public repository:** use a public certified OIDC library and shared identity
  contracts; do not copy proprietary Identity crates into public Robson.

## Second review: evidence-based dispositions

| Finding | Resolution |
|---|---|
| L1: keep SSE alive when introspection is unavailable | **Recommendation not adopted as stated.** Keeping a stream until access-token expiry could exceed the proposed 60-second revocation bound. The local slice has zero positive read caching, closes on unavailable authority, records that cause and uses existing bounded reconnect backoff. This stricter choice is now explicit in the ADR and tested for both revocation and outage. A measured production cache/grace policy remains pending. |
| L2: discovery timeout might not bound other calls | Source check of pinned `openid-client` 6.8.8 shows discovery copies the configured timeout into Configuration; protocol calls use that value. Added an explicit `client.timeout = 2` for clarity and a hanging-provider test: protective admission returns 503 at about two seconds and never dispatches. |
| L3: maxAge=0 might reject every delayed exchange | Source check of pinned `oauth4webapi` shows default clock tolerance is 30 seconds, so the claimed same-second requirement was inaccurate. Nonetheless made tolerance explicit (5 s) and separated request `max_age=0` from response validation's bounded 300 s transaction window. The stricter transaction-start/auth_time check remains authoritative. A delayed callback passes; old silent SSO still fails. |
| L4: late refresh/superseded login leaves grants | Losing refresh passes newly returned credentials to a best-effort revoker; superseded login/step-up revokes the prior pair. Logout/expiry still wins locally. Production must prove grant isolation so cleanup cannot revoke a new authorization; failures still need a durable queue before rollout. |
| L5: stale activity metadata/browser clock skew | Apply activity response metadata, reject reordered older metadata and calculate UI deadlines from server time plus monotonic elapsed time. Wall-clock-skew regression test added. |
| L6: adapter rough edges | Remove redundant connect-time stream introspection; add safe failure-class logs; assert mode exclusivity at router construction as well as daemon startup. Token/config structs intentionally have no Debug implementation. Add exact stream-expiry deadline independent of event traffic. |
| L7: CSRF comparison | Equal-length `timingSafeEqual`, with type/length guards. |
| L8: one revocation failure skips the other | `Promise.allSettled` attempts both access and refresh revocation; failure metric and response remain explicit. |
| L9: reconciliation classification | Keep `/reconcile-close` sensitive: it changes historical book/accounting evidence and future budget, not emergency exchange-order execution. Document the decision; protective endpoints remain DELETE position, panic and halt. |
| L10: consumer inventory | Inventory frontend/API paths, shipped CLI operations, health probes and local smoke assumptions. Mark affected operational runbooks. External monitors require inventory before deployment; none was changed or deployed. |

## Local verification and limits

Verification covers Rust workspace tests with and without PostgreSQL features
(live database tests remain ignored), frontend type checking and unit tests,
BFF HTTP/OIDC tests with ephemeral RSA keys, property-based refresh/logout
interleavings, and Chromium login/reload/offline/logout/periodic-expiry tests.
Browser scenarios also run against the built static frontend, because earlier
frontend incidents involved production-build lifecycle behavior.

Final results: **839** Rust tests in the default workspace suite (27 ignored),
**845** with PostgreSQL features (73 ignored; no live database), **149** frontend
unit tests, **18** BFF tests, and **2** browser scenarios passing both in dev and
against the built static bundle. Frontend check has zero errors and six
pre-existing warnings; both Google and session builds succeed.

No database, IdP client, DNS, cluster resource or real account was provisioned.
The BFF dependency audit reports zero known vulnerabilities for its six locked
dependencies; this is not a vulnerability-free claim about the full workspace.
Clippy completes with existing repository warnings. Gitleaks/cargo-audit were
not installed on this host; the repository's CI security gates still apply.
The Rust lockfile required pruning stale unused vendored-OpenSSL entries before
`--locked` could pass; no Rust dependency version was upgraded. A pre-existing
race between the CLI's two XDG-config environment tests surfaced during the
full suite and was fixed by serializing those tests, without changing CLI login.

Production blockers remain explicit: durable shared sessions and transactions,
distributed rotation/crash recovery, durable revocation cleanup, production
cookie/TLS/key handling, real IdP claim/grant/lifetime acceptance, multi-replica
and long-duration stream/load tests, and operational protective-action policy.
No Tauri application exists here. Neither Tauri readiness nor an RBX CLI
migration is claimed.

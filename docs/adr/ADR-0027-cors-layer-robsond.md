# ADR-0027: Robsond CORS Layer Driven by Env Var

**Date:** 2026-04-25
**Status:** Accepted

## Context

Production exposes the frontend at two origins:

- `https://robson.rbx.ia.br` (pt-BR default)
- `https://robson.rbxsystems.ch` (en default)

Both origins call the robsond API at
`https://api.robson.rbx.ia.br`. Every cross-origin request must
pass a CORS preflight (`OPTIONS`) that the backend acknowledges
with `Access-Control-Allow-Origin`. Without that header the
browser refuses to send the actual request and the operator sees
silent failures (login appears to hang, dashboard shows
"connection error").

Robsond historically had no CORS handling because v2 ran behind
the same origin as the legacy React frontend (`app.robson.rbx.ia.br`).
The v3 cutover splits frontend and backend onto different
subdomains, making CORS mandatory.

The allow-list must be configurable per environment:

- Production: only the two real frontend origins.
- Testnet: same hosts under a different scheme or a `testnet-*`
  prefix.
- Development: typically `http://localhost:5173` for `pnpm dev`.

Hardcoding the list in code would force a rebuild for any origin
change.

## Decision

Add a `tower_http::cors::CorsLayer` to the merged Axum router in
`v3/robsond/src/api.rs`. The allow-list is read from the
`ROBSON_CORS_ALLOWED_ORIGINS` environment variable as a
comma-separated list.

```rust
fn build_cors_layer() -> CorsLayer {
    let raw = std::env::var("ROBSON_CORS_ALLOWED_ORIGINS")
        .unwrap_or_default();
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| HeaderValue::from_str(s).ok())
        .collect();
    if origins.is_empty() {
        return CorsLayer::new();
    }
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST,
                        Method::DELETE, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION,
                        header::CONTENT_TYPE])
}
```

When `ROBSON_CORS_ALLOWED_ORIGINS` is unset or empty, the layer
adds no CORS headers. This preserves the pre-existing behavior
that test harnesses (which call the API from the same origin)
relied on, and avoids leaking origins when the variable is not
intentionally configured.

In production the variable is set in
`rbx-infra/apps/prod/robson/robsond-config.yml`:

```yaml
ROBSON_CORS_ALLOWED_ORIGINS: "https://robson.rbx.ia.br,https://robson.rbxsystems.ch"
```

## Consequences

**Positive**

- Browser preflight succeeds; frontend works against the prod
  backend without further changes.
- Allow-list is operator-managed via ConfigMap, no rebuild needed
  to add/remove origins (e.g., adding a staging frontend).
- Default empty behavior preserves test compatibility.
- Pattern matches the existing env-direct reads in robsond
  (`ROBSON_API_HOST`, `ROBSON_API_PORT`, `ROBSON_API_TOKEN`) — no
  new architectural concept introduced.

**Negative / trade-offs**

- Wildcard origins (`*`) are not supported by intent; the parser
  treats `*` as a literal value that fails `HeaderValue::from_str`
  for `AllowOrigin::list`. If a future use case requires wildcard
  + credentials, the function will need an explicit branch.
- Misconfiguration (typo in env var) silently disables CORS for
  that origin. The recommended check is `curl -I -X OPTIONS -H
  "Origin: https://..." https://api.robson.rbx.ia.br/health` — see
  the runbook in `rbx-infra/docs/runbooks/`.

## Alternatives

- **Hardcoded allow-list in source** — rejected; requires rebuild
  for every origin change and conflates code with environment.
- **`tower_http::cors::CorsLayer::permissive()`** — rejected;
  exposes the API to any origin, including hostile ones, defeating
  the trust boundary even though Bearer auth still gates mutation.
- **CORS via Traefik middleware** — viable but splits the policy
  between code and ingress config. Keeping the allow-list with the
  service makes it visible to anyone reading the daemon source.

## Implementation Notes

- Code: `v3/robsond/src/api.rs` (`build_cors_layer`, applied via
  `.layer()` on the merged router).
- Doc reference in `v3/robsond/src/config.rs` (`ApiConfig`
  comment) so readers find the env var without grepping.
- Unit test: `build_cors_layer_parses_origins_from_env` in the
  `tests` module of `api.rs`.
- ConfigMap: `rbx-infra/apps/prod/robson/robsond-config.yml`.

**Related:** ADR-0025 (Bearer token auth), ADR-0028 (k3s hosting).

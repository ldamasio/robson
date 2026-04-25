# Developer Quickstart — Robson v3

**Audience:** engineers contributing to the Robson v3 codebase
(Rust runtime + SvelteKit frontend).
**Goal:** clone, build, and submit a first PR within an hour.

For infrastructure access (cluster, DNS, secrets) see
`rbx-infra/docs/onboarding/ENGINEER-DAY-ONE.md`.

---

## What Robson is

Robson v3 is the canonical version. Earlier iterations (v1 Django,
v2 hybrid) live only in git history.

- **Runtime:** `robsond`, a Rust daemon that executes governed
  trading actions on Binance Futures. Production live with real
  capital.
- **Frontend:** static SvelteKit single-page app served from k3s
  in-cluster behind Traefik.
- **Hosting:** rbx-infra k3s cluster, ArgoCD GitOps,
  cert-manager + Let's Encrypt TLS.
- **Operator UI:** `robson.rbx.ia.br` (pt-BR) and
  `robson.rbxsystems.ch` (en).

Read `AGENTS.md` and `v3/CLAUDE.md` for the full project rules
before writing code.

---

## Prerequisites

Software:

- Rust stable toolchain (`rustup default stable`)
- Node.js 20.x and pnpm 9
- Docker (only if you build container images locally)
- `gh` CLI authenticated to your GitHub user

Optional but recommended:

- `kubectl` and `kustomize` (for inspecting cluster state)
- `just` (workspace task runner; see `v3/justfile` if present)

You do **not** need cluster access to contribute code. Reviewers
and CI handle deployments.

---

## Clone and build

```bash
git clone git@github.com:ldamasio/robson.git
cd robson
```

### Backend (Rust)

```bash
cd v3
cargo build
cargo test --all                  # unit + in-memory tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

The workspace has 11 crates. Pure-domain logic lives in
`robson-domain`; business logic in `robson-engine`; daemon entry
in `robsond`. See `v3/CLAUDE.md` for crate dependency rules.

Postgres-backed integration tests are gated behind
`#[ignore = "requires DATABASE_URL"]`. Skip them locally; CI runs
them with a provisioned database.

### Frontend (SvelteKit)

```bash
cd apps/frontend
pnpm install
cp .env.example .env.local
# .env.local: PUBLIC_ROBSON_API_BASE=http://localhost:8080
pnpm run dev                      # opens http://localhost:5173
```

Verification scripts:

```bash
pnpm run check                    # svelte-check, 0 errors expected
pnpm run test                     # vitest unit
pnpm run test:e2e                 # playwright (mocked API)
pnpm run build                    # static output in build/
```

---

## Running the stack locally

**Frontend only (no backend):** the login form calls
`GET /health` against `PUBLIC_ROBSON_API_BASE`. With no backend
running, login will fail. UI rendering, i18n, and most components
work without a backend.

**Backend only:**

```bash
cd v3
ROBSON_ENV=development \
ROBSON_API_HOST=127.0.0.1 \
ROBSON_API_PORT=8080 \
cargo run -p robsond
```

The daemon refuses to start in production mode without a real
exchange configuration. Development mode runs against the
in-memory store.

**Frontend + backend together:**

1. Run robsond as above.
2. In another terminal, run `pnpm run dev` in `apps/frontend/`.
3. Generate a dev token (any non-empty string when
   `ROBSON_ENV=development` and `ROBSON_API_TOKEN` is unset; the
   auth middleware is no-op when token is unset).
4. Open `http://localhost:5173`, paste any non-empty string at
   `/login`, and the dashboard loads.

---

## Project conventions

- **English only** — code, comments, commits, docs (ADR-0006).
- **Conventional Commits** — `<type>(<scope>): <subject>`. Scopes
  follow the crate or area touched (`domain`, `engine`, `frontend`,
  `infra`, etc.).
- **One PR per logical slice.** Avoid bundling unrelated changes.
- **Hexagonal architecture** in Rust — domain has zero external
  deps, ports in `robson-exec`, adapters in `robson-connectors` and
  `robson-store` (ADR-0002).
- **No `unwrap()`/`expect()` in production code.** Use `?` and
  proper error types. Tests may unwrap.
- **`rust_decimal::Decimal` for all financial amounts.** Never
  `f64`.
- **Robson-authored position invariant** — every open exchange
  position must trace to a `robsond` `GovernedAction`. See
  ADR-0022 and `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`.
- **Symbol-agnostic policies** — never hardcode a symbol in policy
  text. See ADR-0023 and
  `docs/policies/SYMBOL-AGNOSTIC-POLICIES.md`.
- **Technical Stop from chart analysis only.** `entry × (1 − pct)`
  is forbidden. See ADR-0021.

---

## Submitting your first PR

```bash
git checkout -b feat/<scope>/<short-description>
# ...edit, build, test...
git commit -m "feat(<scope>): <subject>"
git push -u origin feat/<scope>/<short-description>
gh pr create --base main
```

CI runs:

- `Robsond CI/CD` — when `v3/**` changes (`cargo fmt`, `cargo
  clippy`, `cargo test`, build + push image to GHCR if mergeing
  to main).
- `Frontend Tests` — when `apps/frontend/**` changes (`pnpm
  check`, `pnpm test`, `pnpm build`).
- `Frontend Build & Publish` — when frontend changes are merged
  to main; builds and pushes the image.

All three must pass before merge.

After merge, ArgoCD picks up new image SHAs from `rbx-infra`
deployment manifests (auto-bumped via separate GitHub Actions on
the rbx-infra side). Cluster sync typically completes within
2–3 minutes.

---

## Where to look when something breaks

| Symptom | First place to check |
|---------|---------------------|
| `cargo clippy` warning that wasn't there yesterday | `v2/clippy.toml` (workspace-level allowlist) |
| `pnpm run check` errors after a merge | run `pnpm install --frozen-lockfile` to sync deps |
| Frontend dev server can't reach backend | `PUBLIC_ROBSON_API_BASE` in `.env.local` |
| Tests pass locally, fail in CI | check `cargo test --features postgres` and the database test tier |
| Production dashboard shows "connection error" | likely CORS — see ADR-0027 and `rbx-infra/docs/runbooks/CERT-MANAGER-DEBUG.md` for related diagnostics |

---

## Further reading

- `AGENTS.md` — repository-wide rules
- `v3/CLAUDE.md` — Rust workspace details
- `docs/architecture/v3-runtime-spec.md` — runtime architecture
- `docs/architecture/v3-control-loop.md` — execution stages
- `docs/adr/` — accepted architectural decisions
- `docs/runbooks/frontend-deploy.md` — deploy procedure
- `rbx-infra/docs/onboarding/ENGINEER-DAY-ONE.md` — infra access
- `rbx-infra/docs/infra/ARCHITECTURE.md` — cluster topology

When in doubt, read the ADRs. They explain why the codebase looks
the way it does.

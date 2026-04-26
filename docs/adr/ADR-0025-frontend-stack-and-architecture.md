ADR-0025: Frontend Stack and Architecture

Status: Accepted (amended 2026-04-23 after EP-003 reality check)
Date: 2026-04-23

Context
- Robson v3 is live in production operating real capital (see `docs/adr/ADR-0007` and repo state as of 2026-04).
- The legacy frontend at `apps/frontend/` (React + Vite + `react-bootstrap`, slate-900 + blue→violet gradient) is being retired. It was a consumer-fintech demo and does not reflect the operational, audit-first nature of Robson.
- Deployment strategy changes: new frontend is a static bundle hosted on Contabo Object Storage (S3-compatible), fronted by the existing PowerDNS sovereign setup.
- Two domains must be supported: `robson.rbx.ia.br` (pt-BR default) and `robson.rbxsystems.ch` (en default). Bilingual, same product.
- MVP scope is single operator (the repo author). Multi-tenant isolation is a later phase.

Decision
- **Framework**: SvelteKit + `@sveltejs/adapter-static`.
  - Small bundle (10× smaller than Angular for the target screen count).
  - SSG-native for S3 hosting.
  - Native reactive stores for state, no Redux/Zustand/TanStack.
  - Ships well with custom tokens and layout primitives.
- **Hosting**: Contabo Object Storage, region `eu2.contabostorage.com`. Same provider already serving `rbx-content` (blog + team assets).
- **CDN**: None for MVP. Direct HTTPS serve from Contabo bucket. Cloudflare free CNAME introduced in FE-P2 for custom-domain TLS + DDoS mitigation.
- **DNS**: PowerDNS sovereign 2-VPS setup (`project_dns_secrets_pattern`). CNAMEs:
  - `robson.rbx.ia.br → eu2.contabostorage.com`
  - `robson.rbxsystems.ch → eu2.contabostorage.com`
- **Auth**: GitHub OAuth for MVP via Auth.js / SvelteKitAuth. Architecture supports multi-provider (Google, magic link) without refactor when multi-tenant phase arrives.
- **State**: Svelte native stores per domain (`auth`, `slots`, `operations`, `events`). No external state library.
- **Styling**: RBX Voltage design tokens (see ADR-0026) + native CSS Grid + container queries + layout primitives (`<Stack>`, `<Row>`, `<Grid>`, `<Bleed>`, `<Prose>`). **No Tailwind.** Tailwind's utility explosion contradicts the Swiss restraint philosophy and inflates bundle for no benefit at this scale.
- **Icons**: Lucide subset (tree-shaken) + 20 RBX-custom domain glyphs (position states, kill-switch, slot indicators). Build-time SVG-to-Svelte-component generation.
- **i18n**: Single codebase, domain-aware default locale via hostname detection. pt-BR on `rbx.ia.br`, en on `rbxsystems.ch`. Cookie toggle overrides.
- **Backend contract**: Robson REST API for operation data. SSE for live event stream. WebSocket deferred.
- **Repo layout**: `apps/frontend-v2/` inside the existing `robson/` monorepo. Legacy `apps/frontend/` removed post-cutover.

Consequences
- Positive
  - Fastest path to MVP (SvelteKit velocity vs Angular boilerplate).
  - Bundle <100KB gzip projected — critical for a tool that may run on operator laptops in poor-connectivity regions.
  - Single codebase eliminates translation drift and CI duplication between `rbx.ia.br` and `rbxsystems.ch`.
  - Contabo keeps data sovereignty consistent with existing infrastructure.
  - Custom tokens enforce design discipline; no utility-class sprawl.
- Negative / Trade-offs
  - SvelteKit ecosystem smaller than React; fewer ready-made components. Mitigation: build from primitives per the design philosophy; ready-made components would violate Voltage anyway.
  - No CDN in MVP means latency outside EU is higher. Acceptable for single-operator MVP; Cloudflare free tier addresses this in FE-P2.
  - Auth.js adds a non-trivial dependency surface. For a single provider, custom session middleware would be simpler. Revisited in FE-P4 when second provider is added.
  - Contabo TLS covers `*.contabostorage.com`, not custom domains out of the box. MVP either redirects or relies on Cloudflare from day 1 (pending decision).

Alternatives
- **Angular** — rejected. Overkill for 6-screen MVP. DI + RxJS + NgModules carry weight that does not pay back at this scale. Reconsiderable if team scales and forms complexity grows.
- **Next.js** — rejected. SSR and serverless features are unnecessary for a static S3 bundle. `rbx-robotica-frontend` uses Next.js for the blog, which is a separate product.
- **Vue/Nuxt** — rejected. No net advantage over SvelteKit in this context; ecosystem size similar.
- **Tailwind CSS** — rejected. Utility classes proliferate; tokens from Voltage system need to be the enforcement mechanism. Tailwind would make it easy to bypass the design system.
- **Separate repo for frontend** — rejected for now. Monorepo simplifies CI and keeps backend/frontend changes atomic. Extractable later without penalty.
- **Two separate builds per domain** — rejected. Single codebase with locale detection reduces maintenance and prevents feature drift between markets.
- **Vercel / Netlify / Cloudflare Pages** — rejected. Data sovereignty: Contabo (EU-based, already used) is preferred over US-headquartered PaaS.

Implementation Notes
- Scaffold location: `apps/frontend-v2/`
- Build: `pnpm build` produces `build/` suitable for `aws s3 sync`.
- CI: GitHub Actions job triggered by changes in `apps/frontend-v2/**`. Runs `pnpm install`, `pnpm build`, `pnpm test`, `aws s3 sync build/ s3://robson-app/ --delete`.
- Rollout phases (canonical `FE-PN` nomenclature, consistent with `MIG-v3#N`, `QE-PN`):
  - **FE-P1** (MVP): Auth, dashboard current month, operation detail, kill-switch, i18n, Voltage applied.
  - **FE-P2**: History year view, month detail, sparklines, export JSON/CSV, Cloudflare CDN.
  - **FE-P3**: Hash chain UI, slot timeline, command palette ⌘K, grid overlay ⌘G, WCAG AA certification.
  - **FE-P4**: Multi-tenant, additional OAuth providers, admin console, billing, per-tenant branding.
- Related agnostic docs (source of truth):
  - `~/docs/rbx-frontend-architecture.md`
  - `~/docs/rbx-brand-voltage-system.md` (see ADR-0026)
  - `~/docs/rbx-frontend-ux-slots-events.md` (see ADR-0027)
  - `~/docs/rbx-frontend-kill-switch.md` (see ADR-0027)
- Implementation guide: `docs/implementation/FE-P1-FRONTEND-MVP.md`
- Pending decisions (tracked in implementation guide):
  - TLS strategy for MVP (Contabo mismatch vs Cloudflare from day 1)
  - Legacy `apps/frontend/` removal timing (post-cutover vs parallel N weeks)

---

## Amendment 1 — 2026-04-23 (post EP-003 reality check)

EP-003 implementation by GLM-5.1 exposed two gaps between this ADR and reality. Both are accepted as amendments, not rollbacks.

### A1.1 Svelte version: 5, not 4

Scaffold initially targeted Svelte 4. During EP-003 the component model was migrated to Svelte 5 (runes: `$props`, `$state`, `$derived`, `Snippet` children). Svelte 5 is the project baseline from this point on. All new components MUST use runes; class-component Svelte 4 syntax is deprecated in this codebase.

### A1.2 Auth: Bearer token MVP, OAuth deferred to FE-P4

`adapter-static` has no server runtime. Auth.js / SvelteKitAuth requires a server (or edge function) for the OAuth callback, so full GitHub OAuth is **incompatible** with the chosen hosting strategy for MVP.

Options evaluated:
- Accept Bearer token auth for MVP (operator pastes a Robson-issued token into a login input; stored in sessionStorage; validated via `/health` on login)
- Add a Cloudflare Worker / edge function for OAuth callback (defer to FE-P2 if Cloudflare CDN is introduced)
- Switch to `adapter-node` + run a small Node process (loses S3 hosting simplicity, adds ops surface)

**Decision**: **Bearer token MVP**. Rationale:
- Single operator (repo author) for FE-P1. OAuth does not deliver value at this scale.
- Preserves `adapter-static` simplicity.
- Aligns with existing Robson CLI token semantics.
- Future-compatible: when FE-P4 multi-tenant arrives, add Cloudflare Worker edge function for OAuth callback, or migrate to `adapter-node`. The `authToken` store abstraction already isolates the change surface.

**GitHub OAuth app is not to be created for FE-P1.** The item is removed from the Infrastructure Gaps P2 row.

### A1.3 Token handling rules

- Token stored in `sessionStorage` under key `robson_api_token`. Scope per tab. Cleared on close.
- Token transmitted as `Authorization: Bearer <token>` on REST calls; for SSE `/events`, passed as query param (EventSource cannot set headers).
- Token MUST NOT be logged, echoed, printed, or committed.
- Token MUST NOT be stored in `localStorage` (persists across tabs and windows; larger attack surface).
- Login page validates the token by calling `GET /health` with the `Authorization` header; rejects on non-2xx.

### A1.4 Amendment to "Pending decisions"

Removed: "Auth.js vs custom session middleware" — resolved (neither; Bearer token).

Added:
- Cooldown for kill-switch must be verified server-side (backend `/monthly-halt` enforcement); if absent, file a backend ticket and document as FE-P1 limitation.
- Future: Cloudflare Worker for OAuth callback in FE-P4, or `adapter-node` migration.


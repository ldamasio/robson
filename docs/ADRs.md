Robson Bot – Architecture Decision Records (ADRs)

Purpose
- Capture significant architectural decisions and their context, especially when introducing or changing a design pattern, a core dependency, or a cross‑cutting concern.

Project Policy
- When introducing a design pattern (e.g., Singleton, Factory, Strategy), add or update an ADR entry in this document.
- For substantial or long‑form ADRs, you may also create per‑ADR files (e.g., `docs/adr/ADR-000X-<slug>.md`) and list them here.
- Keep ADRs concise, actionable, and dated. Reference code paths and links to PRs when relevant.

ADR Template (inline)
- Title: <short, imperative>
- Status: Proposed | Accepted | Deprecated | Superseded
- Date: YYYY-MM-DD
- Context: What problem or forces led to this decision?
- Decision: What is decided?
- Consequences: Positive, negative, trade‑offs
- Alternatives: Considered options and why not chosen
- Notes: References to code, tests, docs, or PRs

---

ADR-0001: BinanceService Singleton
- Status: Accepted
- Date: 2025-09-13

Context
- Multiple parts of the system require a Binance SDK client.
- Instantiating the SDK may perform network calls (e.g., ping) and should not be repeated during a process lifecycle nor during unit tests.
- Tests must be able to patch the client cleanly without hitting the network.

Decision
- Implement a process‑wide Singleton for `BinanceService`, sharing the underlying SDK client.
- Resolve the `Client` symbol dynamically from `api.services.Client` so `@patch('api.services.Client')` in tests replaces the dependency reliably.

Consequences
- Positive:
  - Avoids repeated SDK initialization/network pings
  - Centralizes configuration (testnet vs prod)
  - Improves testability through a single patch point
- Negative/Trade‑offs:
  - Global state (singleton) must be reset in tests when needed
  - Requires care if we ever need multiple independent clients in the same process

Alternatives
- No Singleton: create a new client per use — simpler but can be noisy/slow and harder to patch consistently.
- Dependency Injection container/provider: more flexible but heavier than needed today.
- Module‑level client instance: similar trade‑offs to singleton, harder to reset predictably in tests.

Notes
- Implementation: `backends/monolith/api/services/binance_service.py`
  - Class‑level `_instance` and `_client` for singleton behavior
  - `import_module('api.services')` to resolve `Client` dynamically
- Tests: `backends/monolith/api/tests_services.py`
  - Reset singleton state in `setUp()` and patch `api.services.Client`
- Related docs: `docs/STYLE_GUIDE.md` (English‑only, Conventional Commits); `docs/AI_WORKFLOW.md` (AI collaboration rules)


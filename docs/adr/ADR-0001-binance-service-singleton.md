ADR-0001: BinanceService Singleton

Status: Accepted
Date: 2025-09-13

Context
- Multiple parts of the system require a Binance SDK client.
- The SDK may perform a network call on initialization (e.g., `ping`), which is undesired to repeat per use and breaks tests in CI.
- Tests must patch the client cleanly and avoid external network calls.

Decision
- Implement a process‑wide singleton for `BinanceService`, sharing the underlying SDK client.
- Resolve the `Client` symbol dynamically from `api.services.Client` so `@patch('api.services.Client')` in tests replaces the dependency reliably.

Consequences
- Positive
  - Avoids repeated SDK initialization/network pings.
  - Centralizes configuration (testnet vs prod) through a single client.
  - Improves testability via a single patch point.
- Negative/Trade‑offs
  - Global state must be reset in tests (`_instance`, `_client`).
  - Limits concurrent multiple client configurations in a single process unless extended.

Alternatives
- No singleton — new client per use: simpler but noisy/slow and harder to patch consistently.
- DI container/provider: flexible but heavier than current needs.
- Module‑level client instance: similar trade‑offs to singleton, harder to reset in tests.

Implementation Notes
- Code: `apps/backend/monolith/api/services/binance_service.py`
  - Class‑level `_instance`, `_client` enforce singleton.
  - `import_module('api.services')` dynamically loads `Client` to honor test patches.
- Tests: `apps/backend/monolith/api/tests_services.py`
  - Reset singleton state in `setUp()` and patch `api.services.Client`.
- Related docs: `docs/STYLE_GUIDE.md`, `docs/AI_WORKFLOW.md`.

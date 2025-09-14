# Contributing: Ports & Adapters

This project uses Hexagonal Architecture. Follow these rules when adding or changing adapters.

- Ports (interfaces): live under the application layer and have no framework imports.
  - Backend: `apps/backend/core/application/ports.py`
  - Frontend: `apps/frontend/src/ports/*`
- Adapters (implementations): live under `adapters/` and may import frameworks/SDKs.
  - Backend: `apps/backend/core/adapters/driven|driving/*`
  - Frontend: `apps/frontend/src/adapters/*`
- Wiring: construct use cases with adapters in a composition root only.
  - Backend: `apps/backend/core/wiring/*`

Checklist
- Define/extend a port interface first (narrow surface; return domain/DTO types, not framework types).
- Implement the adapter:
  - Map between port DTOs and framework objects.
  - Handle errors, timeouts, and retries within the adapter; never leak framework exceptions.
  - Keep domain/application free of framework imports.
- Add tests:
  - Contract tests for the adapter vs. port expectations.
  - Unit tests for the application layer using fakes of ports.
- Configuration:
  - Backend: load config via env in settings/wiring (not in domain/application).
  - Frontend: use `VITE_*` env vars; never inline URLs.
- Observability:
  - Log adapter boundary events (latency, failures) in adapters only.

Naming
- Ports: `*Repository`, `*Service`, `*Gateway`, `*Port`.
- Adapters: concrete and specific, e.g., `DjangoOrderRepository`, `BinanceMarketData`, `RabbitBus`.

Examples
- Backend use case: `apps/backend/core/application/place_order.py`
- Backend adapters: `apps/backend/core/adapters/driven/*`
- Frontend adapter: `apps/frontend/src/adapters/http/TradeHttp.js`

Review tips
- Ensure imports from domain/application never reference adapters or frameworks.
- Validate env/config is injected at the edge (wiring), not pulled from domain/application.


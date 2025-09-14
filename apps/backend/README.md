# robson-backend

Backend monolith (Django) adopting Hexagonal Architecture under `core/`.

- `core/domain`: domain entities and rules (no Django).
- `core/application`: use cases and ports (interfaces).
- `core/adapters`: concrete integrations (DB, cache, messaging, external APIs) and inbound interfaces (REST/WS).
- `core/wiring`: dependency composition.

The legacy monolith is kept temporarily under `monolith/` during migration.


ADR-0002: Adopt Hexagonal Architecture (Ports & Adapters)

Status: Accepted
Date: 2025-09-14

Context
- We are consolidating services in a monorepo and need a structure that keeps domain logic independent from frameworks, databases, and delivery mechanisms (REST/WS/CLI).
- The project integrates multiple external systems (Binance, Redis, RabbitMQ, Object Storage) and must remain testable and evolvable as technologies change.
- Existing Django code mixes domain rules with framework concerns, making tests and substitutions harder.

Decision
- Adopt Hexagonal Architecture (Ports & Adapters) as the central architectural pattern.
- Organize backend code into:
  - `domain/`: entities, value objects, domain services (no framework imports)
  - `application/`: use cases and ports (interfaces)
  - `adapters/`: implementations of ports (driven: DB/cache/broker/external APIs; driving: REST/WS/CLI)
  - `wiring/`: composition root for dependency injection and transactions
- Keep infrastructure (Terraform, Ansible, K8s manifests) outside application packages under `infra/`.

Consequences
- Positive
  - Domain logic remains independent from frameworks and I/O technologies.
  - High testability via port fakes/mocks; contract tests for adapters.
  - Easier technology swaps (e.g., replace RabbitMQ or HTTP client) without touching domain/application.
- Trade-offs
  - Requires discipline on import boundaries and an initial migration cost.
  - Some duplication in mapping DTOs <-> persistence/API until generators or shared contracts are in place.

Implementation Notes
- Repository layout: see `docs/ARCHITECTURE.md` and `apps/backend/core/*`.
- Initial use case: `PlaceOrderUseCase` in `apps/backend/core/application/place_order.py`.
- Example ports: `apps/backend/core/application/ports.py`.
- Example adapters: 
  - Persistence: `apps/backend/core/adapters/driven/persistence/django_order_repo.py`
  - External (Binance MD): `apps/backend/core/adapters/driven/external/binance_client.py`
  - Messaging (noop): `apps/backend/core/adapters/driven/messaging/noop_bus.py`
  - Time: `apps/backend/core/adapters/driven/time/clock.py`
- Wiring: `apps/backend/core/wiring/container.py`.
- Frontend follows the same idea client-side with `src/{domain,application,ports,adapters}`.

Related
- ADR-0001: BinanceService Singleton (legacy service compatibility and testing strategy)
- docs/ARCHITECTURE.md
- docs/MIGRATION_PLAN.md


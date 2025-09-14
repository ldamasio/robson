# Robson — Hexagonal Architecture (Ports & Adapters)

This repository adopts Hexagonal Architecture (aka Ports & Adapters) as an architectural pattern.

Key principles:

- Ports are abstractions (interfaces) owned by the application/domain.
- Adapters are concrete implementations of ports (frameworks, DBs, brokers, HTTP, etc.).
- The domain never depends on adapters or frameworks. Dependencies flow outward.
- Infrastructure artifacts (migrations, Terraform, Ansible, K8s manifests) live outside the application code.

Conceptual positioning:

- Design Pattern: localized solutions for small code problems (e.g., Factory, Observer).
- Architectural Style: broad structuring (layers, client-server, microservices).
- Architectural Pattern (Hexagonal): in-between; defines roles and dependency rules without dictating tech.

Why Hexagonal here:

- Keeps core domain independent of infra, enabling tech swaps and testability.
- Uses ports (interfaces) and adapters (implementations) for all I/O.
- Enforces unidirectional dependencies from domain → outwards.

Repository structure (high level):

```
apps/
  backend/
    core/                # hexagonal center (domain, application, ports, adapters, wiring)
  frontend/              # React (ports & adapters on the client side)
infra/                   # Terraform, Ansible, K8s, GitOps, Observability, DB
docs/                    # ADRs, architecture, developer guides
```

Backend (Django monolith) — Hexagonal layout under `apps/backend/core`:

```
domain/                  # entities, value objects, domain services (no Django imports)
application/             # use cases and ports (interfaces)
adapters/
  driven/                # outbound: persistence, cache, messaging, external APIs
  driving/               # inbound: REST/WS/CLI/jobs calling use cases
wiring/                  # dependency composition (factories/DI, transactions)
```

Frontend (React + Vite):

```
src/domain               # pure types and logic
src/ports                # interfaces (HTTP/WS/storage)
src/adapters             # implementations (fetch/WebSocket/storage)
src/application          # client-side use cases/state orchestration
```

Testing strategy:

- Unit: `domain` and `application` with fakes for ports.
- Contract: each adapter vs its port (e.g., REST client/server, broker bindings).
- Integration/E2E: end-to-end flows with real adapters where sensible.

Migration guidelines:

1) Extract domain models and rules from Django models/views to `core/domain`.
2) Define ports in `core/application` for persistence, market data, messaging, etc.
3) Implement adapters in `core/adapters/*` and wire them in `core/wiring`.
4) Make REST/WS call use cases only (no domain logic in views).
5) Keep migrations and infra in `infra/` or adapter-specific folders.


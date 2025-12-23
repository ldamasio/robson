"""
Domain Layer - Pure Business Logic

This layer contains:
- Entities (objects with identity)
- Value Objects (immutable objects without identity)
- Domain services (pure business logic)

CRITICAL RULES:
- ZERO framework dependencies (no Django, no database, no HTTP)
- Pure Python only (stdlib + typing)
- All objects are immutable where possible (dataclasses with frozen=True)
- Business logic is explicit and testable

If you need Django models, put them in apps/backend/monolith/api/models/
This layer is for the PURE business domain.
"""

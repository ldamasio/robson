"""
Robson Bot - Hexagonal Architecture Core

This package contains the framework-independent core of the trading system,
following the Ports & Adapters (Hexagonal) architecture pattern.

Structure:
- domain/: Pure business entities and value objects (NO framework dependencies)
- application/: Use cases and port definitions (business logic orchestration)
- adapters/: Concrete implementations of ports
  - driven/: Outbound adapters (database, exchange API, messaging, etc.)
  - driving/: Inbound adapters (REST, CLI, scheduled jobs, etc.)
- wiring/: Dependency injection container

Key Principle: Dependencies point INWARD.
- Domain has ZERO external dependencies
- Application depends only on domain
- Adapters depend on application ports (but not vice versa)

See: docs/adr/ADR-0002-hexagonal-architecture.md
"""

__version__ = "1.0.0"

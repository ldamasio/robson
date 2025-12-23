"""
Adapters Layer - Concrete Implementations of Ports

This layer contains implementations of the ports defined in application/ports.py.

Structure:
- driven/: Outbound adapters (things the application USES)
  - persistence/: Database repositories
  - messaging/: Message bus implementations
  - external/: Exchange API clients, etc.
- driving/: Inbound adapters (things that USE the application)
  - rest/: REST API endpoints
  - cli/: CLI commands
  - workers/: Background workers

Key Principle: Adapters depend on ports, ports don't depend on adapters.
"""

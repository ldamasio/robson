"""
Entry Gate Module - Opportunity Execution Control Layer

This module implements the entry gating system that decides WHEN an entry is permitted.
Entry happens only if Robson:
- has available risk budget (dynamic concurrent position limit)
- is not in cooldown after a stop-out
- has not exceeded monthly loss quota
- market context permits (optional constraints)

Architecture: Hexagonal (Ports & Adapters) inside Django monolith
- domain.py: Pure entities (NO Django dependencies)
- ports.py: Port definitions (Protocol interfaces)
- use_cases.py: Business logic (gate checks)
- adapters.py: Concrete implementations (Django ORM)
- wiring.py: Dependency injection
"""

from .domain import GateCheckResult, EntryGateDecision, EntryGateConfig

__all__ = [
    "GateCheckResult",
    "EntryGateDecision",
    "EntryGateConfig",
]

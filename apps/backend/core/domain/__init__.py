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

# Trading domain
from apps.backend.core.domain.trading import (
    TradingIntent,
    IntentStatus,
    PolicyState,
    PolicyStatus,
    ExecutionEvent,
)

# Margin trading domain
from apps.backend.core.domain.margin import (
    MarginPosition,
    MarginPositionStatus,
    MarginSide,
    MarginLevel,
    MarginAccountInfo,
    TransferResult,
    MarginOrderResult,
    MarginPositionSizingResult,
    calculate_margin_position_size,
)

# Emotional Trading Guard domain
from apps.backend.core.domain.emotional_guard import (
    SignalType,
    RiskLevel,
    EmotionalSignal,
    ExtractedParameters,
    IntentAnalysis,
    calculate_risk_level,
)

__all__ = [
    # Trading
    "TradingIntent",
    "IntentStatus",
    "PolicyState",
    "PolicyStatus",
    "ExecutionEvent",
    # Margin
    "MarginPosition",
    "MarginPositionStatus",
    "MarginSide",
    "MarginLevel",
    "MarginAccountInfo",
    "TransferResult",
    "MarginOrderResult",
    "MarginPositionSizingResult",
    "calculate_margin_position_size",
    # Emotional Guard
    "SignalType",
    "RiskLevel",
    "EmotionalSignal",
    "ExtractedParameters",
    "IntentAnalysis",
    "calculate_risk_level",
]
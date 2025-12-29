"""
Domain entities for Entry Gate system.

NO Django dependencies. Pure Python domain objects.
"""

from dataclasses import dataclass, field
from datetime import datetime
from decimal import Decimal
from typing import Dict, List, Optional


@dataclass
class GateCheckResult:
    """
    Result of a single gate check.

    Attributes:
        gate_name: Name of the gate (e.g., "DynamicPositionLimit")
        passed: True if gate allows entry, False if gate blocks
        message: Human-readable message explaining result
        details: Additional context (limits, current values, etc.)
    """
    gate_name: str
    passed: bool
    message: str
    details: Dict[str, any] = field(default_factory=dict)

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "gate_name": self.gate_name,
            "passed": self.passed,
            "message": self.message,
            "details": self.details,
        }


@dataclass
class EntryGateDecision:
    """
    Final entry gate decision aggregating all gate check results.

    Attributes:
        allowed: True if ALL gates passed, False if ANY gate failed
        reasons: List of human-readable reasons (failures + warnings)
        gate_checks: Map of gate_name -> GateCheckResult
        timestamp: When this decision was made
        symbol: Trading pair (e.g., BTCUSDT)
        client_id: Tenant identifier
        context: Full context for debugging/audit
    """
    allowed: bool
    reasons: List[str]
    gate_checks: Dict[str, GateCheckResult]
    timestamp: datetime
    symbol: str
    client_id: int
    context: Dict[str, any] = field(default_factory=dict)

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "allowed": self.allowed,
            "reasons": self.reasons,
            "gate_checks": {name: result.to_dict() for name, result in self.gate_checks.items()},
            "timestamp": self.timestamp.isoformat(),
            "symbol": self.symbol,
            "client_id": self.client_id,
            "context": self.context,
        }


@dataclass
class EntryGateConfig:
    """
    Configuration for entry gate checks (value object).

    Note: 4% monthly / 1% per operation are CONSTANTS (not configurable).
    These are core business rules defining Robson's risk management philosophy.
    """
    # Risk budget constants (NOT configurable)
    BASE_MONTHLY_RISK_PERCENT: Decimal = Decimal("4.0")
    RISK_PER_POSITION_PERCENT: Decimal = Decimal("1.0")

    # Cooldown settings
    enable_cooldown: bool = True
    cooldown_after_stop_seconds: int = 900  # 15 minutes default

    # Market context gates
    enable_funding_rate_gate: bool = True
    funding_rate_threshold: Decimal = Decimal("0.0001")  # 0.01%
    enable_stale_data_gate: bool = True
    max_data_age_seconds: int = 300  # 5 minutes

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "BASE_MONTHLY_RISK_PERCENT": str(self.BASE_MONTHLY_RISK_PERCENT),
            "RISK_PER_POSITION_PERCENT": str(self.RISK_PER_POSITION_PERCENT),
            "enable_cooldown": self.enable_cooldown,
            "cooldown_after_stop_seconds": self.cooldown_after_stop_seconds,
            "enable_funding_rate_gate": self.enable_funding_rate_gate,
            "funding_rate_threshold": str(self.funding_rate_threshold),
            "enable_stale_data_gate": self.enable_stale_data_gate,
            "max_data_age_seconds": self.max_data_age_seconds,
        }

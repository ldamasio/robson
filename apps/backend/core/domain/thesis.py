"""
Trading Thesis domain entities for AI Chat.

NO DJANGO DEPENDENCIES - Pure Python business logic.

A Trading Thesis is a structured market hypothesis that captures the user's
reasoning about a potential trading opportunity, WITHOUT executing any orders.

This is the "thinking layer" - a space for learning, observation, and
selective conversion to executable strategies.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any


class ThesisStatus(str, Enum):
    """Lifecycle status of a trading thesis."""

    DRAFT = "draft"           # Being created/refined
    ACTIVE = "active"         # Monitoring for trigger/invalidation
    VALIDATED = "validated"   # Trigger occurred, thesis was correct
    REJECTED = "rejected"     # Invalidation occurred, thesis was wrong
    EXPIRED = "expired"       # Time-based expiry (e.g., 30 days)
    CONVERTED = "converted"   # Converted to executable strategy


@dataclass(frozen=True)
class TradingThesis:
    """
    A trading hypothesis expressed in natural language.

    Unlike a Strategy, a Thesis does NOT execute trades.
    It is a structured observation/diary entry that can be
    monitored and optionally converted to a Strategy later.

    Core Philosophy:
    - THESIS = "I think X might happen" (observation)
    - STRATEGY = "Execute order when Y occurs" (automation)
    - Not every thesis should become a strategy

    The 4 Required Elements:
    1. Market Context: What's happening now?
    2. Rationale: Why might this opportunity exist?
    3. Expected Trigger: What confirms the thesis?
    4. Invalidation: What proves it wrong?
    """

    # Identity
    id: str
    tenant_id: str

    # Basic info
    title: str
    symbol: str
    timeframe: str

    # The 4 required elements (in natural language)
    market_context: str      # What is happening in the market
    rationale: str           # Why this opportunity might exist
    expected_trigger: str    # What needs to happen to confirm
    invalidation: str        # What proves the thesis wrong

    # Optional metadata
    hypothesis_type: str | None = None  # trend_following, mean_reversion, breakout, etc.
    confidence_level: str | None = None  # low, medium, high
    tags: list[str] = field(default_factory=list)
    notes: str | None = None

    # System fields
    status: ThesisStatus = ThesisStatus.DRAFT
    created_at: datetime = field(default_factory=datetime.now)
    updated_at: datetime = field(default_factory=datetime.now)
    validated_at: datetime | None = None
    converted_to_strategy_id: str | None = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for serialization."""
        return {
            "id": self.id,
            "tenant_id": self.tenant_id,
            "title": self.title,
            "symbol": self.symbol,
            "timeframe": self.timeframe,
            "market_context": self.market_context,
            "rationale": self.rationale,
            "expected_trigger": self.expected_trigger,
            "invalidation": self.invalidation,
            "hypothesis_type": self.hypothesis_type,
            "confidence_level": self.confidence_level,
            "tags": self.tags,
            "notes": self.notes,
            "status": self.status.value,
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
            "validated_at": self.validated_at.isoformat() if self.validated_at else None,
            "converted_to_strategy_id": self.converted_to_strategy_id,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> TradingThesis:
        """Create from dictionary."""
        return cls(
            id=data["id"],
            tenant_id=data["tenant_id"],
            title=data["title"],
            symbol=data["symbol"],
            timeframe=data["timeframe"],
            market_context=data["market_context"],
            rationale=data["rationale"],
            expected_trigger=data["expected_trigger"],
            invalidation=data["invalidation"],
            hypothesis_type=data.get("hypothesis_type"),
            confidence_level=data.get("confidence_level"),
            tags=data.get("tags", []),
            notes=data.get("notes"),
            status=ThesisStatus(data.get("status", ThesisStatus.DRAFT)),
            created_at=datetime.fromisoformat(data["created_at"]) if data.get("created_at") else datetime.now(),
            updated_at=datetime.fromisoformat(data["updated_at"]) if data.get("updated_at") else datetime.now(),
            validated_at=datetime.fromisoformat(data["validated_at"]) if data.get("validated_at") else None,
            converted_to_strategy_id=data.get("converted_to_strategy_id"),
        )

    def is_monitoring(self) -> bool:
        """Check if thesis is in monitoring state."""
        return self.status == ThesisStatus.ACTIVE

    def is_terminal(self) -> bool:
        """Check if thesis is in terminal state (no further updates)."""
        return self.status in {
            ThesisStatus.VALIDATED,
            ThesisStatus.REJECTED,
            ThesisStatus.EXPIRED,
            ThesisStatus.CONVERTED,
        }

    def is_complete(self) -> bool:
        """Validate that all 4 required elements are present."""
        return bool(
            self.market_context and
            self.rationale and
            self.expected_trigger and
            self.invalidation
        )


@dataclass(frozen=True)
class ThesisTemplate:
    """
    A template for creating common trading thesis patterns.

    Helps users structure their theses with suggested wording.
    """

    name: str
    hypothesis_type: str
    description: str
    context_template: str
    rationale_template: str
    trigger_template: str
    invalidation_template: str

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary."""
        return {
            "name": self.name,
            "hypothesis_type": self.hypothesis_type,
            "description": self.description,
            "context_template": self.context_template,
            "rationale_template": self.rationale_template,
            "trigger_template": self.trigger_template,
            "invalidation_template": self.invalidation_template,
        }


# Predefined templates for common thesis patterns
THESIS_TEMPLATES: dict[str, ThesisTemplate] = {
    "breakout": ThesisTemplate(
        name="Breakout Setup",
        hypothesis_type="breakout",
        description="Price consolidates then breaks out with volume",
        context_template="{symbol} has been consolidating between {support} and {resistance} for {duration}",
        rationale_template="Volume declining, Bollinger Bands squeezing - coiled spring pattern",
        trigger_template="Close above {resistance} with volume > {volume_multiplier}x average",
        invalidation_template="Break below {support} or volume spike without price movement",
    ),
    "mean_reversion": ThesisTemplate(
        name="Mean Reversion",
        hypothesis_type="mean_reversion",
        description="Price extends too far from mean and reverts",
        context_template="{symbol} is {oversold_overbought} on {timeframe} (RSI: {rsi_value})",
        rationale_template="Price extended {standard_deviations} standard deviations from mean",
        trigger_template="RSI crosses back through {threshold} on {timeframe}",
        invalidation_template="Price extends further by {additional_percent} or breaks recent {high_low}",
    ),
    "trend_following": ThesisTemplate(
        name="Trend Continuation",
        hypothesis_type="trend_following",
        description="Existing trend continues after pullback",
        context_template="{symbol} in {uptrend_downtrend}, pulling back to {support_level}",
        rationale_template="Higher highs/lows intact, pullback is low-risk entry zone",
        trigger_template="Price breaks back above {breakout_level} with volume",
        invalidation_template="Break below {support_level} or price closes below {moving_average}",
    ),
}


def get_template(hypothesis_type: str) -> ThesisTemplate | None:
    """Get thesis template by type."""
    return THESIS_TEMPLATES.get(hypothesis_type)


def list_templates() -> list[ThesisTemplate]:
    """List all available thesis templates."""
    return list(THESIS_TEMPLATES.values())

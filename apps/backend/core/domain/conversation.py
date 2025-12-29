"""
Conversation domain entities for AI Chat.

NO DJANGO DEPENDENCIES - Pure Python business logic.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from decimal import Decimal
from enum import Enum
from typing import Any


class MessageRole(Enum):
    """Role of the message sender."""

    USER = "user"
    ASSISTANT = "assistant"
    SYSTEM = "system"


class IntentType(Enum):
    """Detected trading intent from user message."""

    BUY = "buy"
    SELL = "sell"
    ANALYZE = "analyze"
    BALANCE = "balance"
    POSITIONS = "positions"
    RISK = "risk"
    GENERAL = "general"


@dataclass(frozen=True)
class Message:
    """
    Immutable message in a conversation.

    Attributes:
        id: Unique message identifier
        role: Who sent the message (user/assistant/system)
        content: Message text content
        timestamp: When the message was created
        metadata: Additional context (intent, symbols, etc.)
    """

    id: str
    role: MessageRole
    content: str
    timestamp: datetime
    metadata: dict[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for serialization."""
        return {
            "id": self.id,
            "role": self.role.value,
            "content": self.content,
            "timestamp": self.timestamp.isoformat(),
            "metadata": self.metadata or {},
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Message:
        """Create from dictionary."""
        return cls(
            id=data["id"],
            role=MessageRole(data["role"]),
            content=data["content"],
            timestamp=datetime.fromisoformat(data["timestamp"]),
            metadata=data.get("metadata"),
        )


@dataclass
class TradingContext:
    """
    Current trading context for AI assistant.

    Provides real-time information about user's trading state.
    """

    tenant_id: str
    balances: dict[str, Decimal] = field(default_factory=dict)
    positions: list[dict[str, Any]] = field(default_factory=list)
    recent_trades: list[dict[str, Any]] = field(default_factory=list)
    current_prices: dict[str, Decimal] = field(default_factory=dict)
    risk_metrics: dict[str, Any] = field(default_factory=dict)
    monthly_pnl: Decimal = Decimal("0")

    def to_system_prompt(self) -> str:
        """Generate system prompt with trading context."""
        context_parts = [
            "You are Robson, an AI trading assistant for cryptocurrency.",
            "You help traders analyze markets, manage risk, and execute trades.",
            "Always prioritize risk management and never encourage reckless trading.",
            "",
            "CURRENT TRADING CONTEXT:",
        ]

        # Balances
        if self.balances:
            context_parts.append("\nBALANCES:")
            for asset, amount in self.balances.items():
                context_parts.append(f"  - {asset}: {amount}")

        # Positions
        if self.positions:
            context_parts.append("\nOPEN POSITIONS:")
            for pos in self.positions:
                context_parts.append(
                    f"  - {pos.get('symbol')}: {pos.get('side')} "
                    f"{pos.get('quantity')} @ ${pos.get('entry_price')} "
                    f"(P&L: {pos.get('pnl_percent', 0):.2f}%)"
                )
        else:
            context_parts.append("\nOPEN POSITIONS: None")

        # Recent trades
        if self.recent_trades:
            context_parts.append("\nRECENT TRADES (last 5):")
            for trade in self.recent_trades[:5]:
                context_parts.append(
                    f"  - {trade.get('side')} {trade.get('symbol')} "
                    f"P&L: ${trade.get('pnl', 0):.2f}"
                )

        # Current prices
        if self.current_prices:
            context_parts.append("\nCURRENT PRICES:")
            for symbol, price in self.current_prices.items():
                context_parts.append(f"  - {symbol}: ${price}")

        # Risk metrics
        context_parts.append(f"\nMONTHLY P&L: ${self.monthly_pnl}")
        if self.risk_metrics:
            context_parts.append(f"RISK METRICS: {self.risk_metrics}")

        context_parts.extend(
            [
                "",
                "RULES:",
                "1. Always recommend stop-loss for any trade",
                "2. Maximum risk per trade: 1% of capital",
                "3. Maximum monthly drawdown: 4%",
                "4. If user wants to trade, ask for confirmation before executing",
                "5. Provide clear, actionable advice",
                "6. Respond in the same language as the user (Portuguese or English)",
            ]
        )

        return "\n".join(context_parts)


@dataclass
class Conversation:
    """
    A conversation between user and AI assistant.

    Attributes:
        id: Unique conversation identifier
        tenant_id: User/tenant who owns this conversation
        messages: List of messages in order
        context: Current trading context
        created_at: When conversation started
        updated_at: Last activity timestamp
    """

    id: str
    tenant_id: str
    messages: list[Message] = field(default_factory=list)
    context: TradingContext | None = None
    created_at: datetime = field(default_factory=datetime.now)
    updated_at: datetime = field(default_factory=datetime.now)

    def add_message(self, message: Message) -> None:
        """Add a message to the conversation."""
        self.messages.append(message)
        self.updated_at = datetime.now()

    def get_messages_for_ai(self, max_messages: int = 20) -> list[dict[str, str]]:
        """
        Get messages formatted for AI API.

        Returns recent messages in the format expected by LLM APIs.
        """
        recent = (
            self.messages[-max_messages:] if len(self.messages) > max_messages else self.messages
        )
        return [{"role": msg.role.value, "content": msg.content} for msg in recent]

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for serialization."""
        return {
            "id": self.id,
            "tenant_id": self.tenant_id,
            "messages": [m.to_dict() for m in self.messages],
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
        }


@dataclass(frozen=True)
class ChatResponse:
    """
    Response from AI assistant.

    Attributes:
        message: The assistant's response message
        detected_intent: What the user wants to do (if trading-related)
        suggested_action: Recommended action (if any)
        requires_confirmation: Whether action needs user confirmation
    """

    message: Message
    detected_intent: IntentType | None = None
    suggested_action: dict[str, Any] | None = None
    requires_confirmation: bool = False

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for serialization."""
        return {
            "message": self.message.to_dict(),
            "detected_intent": self.detected_intent.value if self.detected_intent else None,
            "suggested_action": self.suggested_action,
            "requires_confirmation": self.requires_confirmation,
        }

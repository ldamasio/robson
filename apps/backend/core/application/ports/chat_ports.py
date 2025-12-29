"""
Port definitions for AI Chat functionality.

These are interfaces (Protocols) that define the contracts for adapters.
"""

from __future__ import annotations

from decimal import Decimal
from typing import Any, Protocol

from core.domain.conversation import Conversation, TradingContext


class AIProviderPort(Protocol):
    """
    Interface for AI/LLM providers (Groq, OpenAI, Anthropic, etc.).

    Implementations should handle:
    - API communication
    - Rate limiting
    - Error handling
    - Response parsing
    """

    def generate_response(
        self,
        messages: list[dict[str, str]],
        system_prompt: str,
        max_tokens: int = 1024,
        temperature: float = 0.7,
    ) -> str:
        """
        Generate a response from the AI model.

        Args:
            messages: Conversation history in API format
            system_prompt: System instructions for the AI
            max_tokens: Maximum response length
            temperature: Creativity level (0-1)

        Returns:
            The AI's response text
        """
        ...

    def get_model_name(self) -> str:
        """Get the name of the model being used."""
        ...


class ConversationRepositoryPort(Protocol):
    """
    Interface for conversation persistence.

    Handles storage and retrieval of conversations.
    """

    def save(self, conversation: Conversation) -> Conversation:
        """Save or update a conversation."""
        ...

    def find_by_id(
        self,
        conversation_id: str,
        tenant_id: str,
    ) -> Conversation | None:
        """
        Find conversation by ID.

        CRITICAL: Must filter by tenant_id for multi-tenant isolation.
        """
        ...

    def list_by_tenant(
        self,
        tenant_id: str,
        limit: int = 10,
    ) -> list[Conversation]:
        """List recent conversations for a tenant."""
        ...

    def delete(self, conversation_id: str, tenant_id: str) -> bool:
        """Delete a conversation."""
        ...


class TradingContextPort(Protocol):
    """
    Interface for gathering trading context.

    Collects real-time data about user's trading state.
    """

    def get_context(self, tenant_id: str) -> TradingContext:
        """
        Get current trading context for a user.

        Gathers:
        - Account balances
        - Open positions
        - Recent trades
        - Current prices
        - Risk metrics
        """
        ...

    def get_current_price(self, symbol: str) -> Decimal:
        """Get current price for a symbol."""
        ...

    def get_balances(self, tenant_id: str) -> dict[str, Decimal]:
        """Get account balances."""
        ...


class TradeExecutorPort(Protocol):
    """
    Interface for executing trades from chat commands.

    IMPORTANT: All executions require explicit confirmation.
    """

    def execute_buy(
        self,
        tenant_id: str,
        symbol: str,
        quantity: Decimal,
        stop_price: Decimal | None = None,
    ) -> dict[str, Any]:
        """Execute a buy order."""
        ...

    def execute_sell(
        self,
        tenant_id: str,
        symbol: str,
        quantity: Decimal | None = None,
    ) -> dict[str, Any]:
        """Execute a sell order."""
        ...

    def get_position(
        self,
        tenant_id: str,
        symbol: str,
    ) -> dict[str, Any] | None:
        """Get current position for a symbol."""
        ...

"""
Use case: Chat with Robson AI Assistant.

Orchestrates conversation flow between user and AI.
"""

from __future__ import annotations

import logging
import uuid
from datetime import datetime
from typing import Any

from core.application.ports.chat_ports import (
    AIProviderPort,
    ConversationRepositoryPort,
    TradingContextPort,
)
from core.domain.conversation import (
    ChatResponse,
    Conversation,
    IntentType,
    Message,
    MessageRole,
    TradingContext,
)

logger = logging.getLogger(__name__)


class ChatWithRobsonUseCase:
    """
    Main use case for conversational trading assistant.

    Responsibilities:
    1. Load or create conversation
    2. Gather current trading context
    3. Generate AI response with context
    4. Detect trading intents
    5. Persist conversation
    """

    def __init__(
        self,
        ai_provider: AIProviderPort,
        conversation_repo: ConversationRepositoryPort | None = None,
        trading_context: TradingContextPort | None = None,
    ):
        """
        Initialize use case with dependencies.

        Args:
            ai_provider: AI/LLM provider for generating responses
            conversation_repo: Optional persistence for conversations
            trading_context: Optional trading data provider
        """
        self.ai_provider = ai_provider
        self.conversation_repo = conversation_repo
        self.trading_context = trading_context

    def execute(
        self,
        tenant_id: str,
        user_message: str,
        conversation_id: str | None = None,
    ) -> ChatResponse:
        """
        Process a user message and generate AI response.

        Args:
            tenant_id: User/tenant identifier (CRITICAL for isolation)
            user_message: The user's message text
            conversation_id: Optional existing conversation ID

        Returns:
            ChatResponse with AI message and detected intent
        """
        logger.info(f"Processing chat for tenant {tenant_id}")

        # 1. Load or create conversation
        conversation = self._get_or_create_conversation(
            tenant_id=tenant_id,
            conversation_id=conversation_id,
        )

        # 2. Add user message
        user_msg = Message(
            id=str(uuid.uuid4()),
            role=MessageRole.USER,
            content=user_message,
            timestamp=datetime.now(),
        )
        conversation.add_message(user_msg)

        # 3. Gather trading context
        context = self._get_trading_context(tenant_id)
        conversation.context = context

        # 4. Generate AI response
        system_prompt = context.to_system_prompt() if context else self._default_system_prompt()

        messages_for_ai = conversation.get_messages_for_ai()

        try:
            ai_response_text = self.ai_provider.generate_response(
                messages=messages_for_ai,
                system_prompt=system_prompt,
                max_tokens=1024,
                temperature=0.7,
            )
        except Exception as e:
            logger.error(f"AI generation failed: {e}")
            ai_response_text = (
                "Sorry, I am experiencing technical difficulties at the moment. "
                "Please try again in a few seconds."
            )

        # 5. Create assistant message
        assistant_msg = Message(
            id=str(uuid.uuid4()),
            role=MessageRole.ASSISTANT,
            content=ai_response_text,
            timestamp=datetime.now(),
            metadata={
                "model": self.ai_provider.get_model_name(),
                "conversation_id": conversation.id,
            },
        )
        conversation.add_message(assistant_msg)

        # 6. Detect trading intent
        intent = self._detect_intent(user_message, ai_response_text)

        # 7. Persist conversation
        if self.conversation_repo:
            self.conversation_repo.save(conversation)

        # 8. Build response
        response = ChatResponse(
            message=assistant_msg,
            detected_intent=intent,
            suggested_action=self._extract_action(intent, ai_response_text),
            requires_confirmation=intent in [IntentType.BUY, IntentType.SELL],
        )

        logger.info(
            f"Chat response generated: {len(ai_response_text)} chars, "
            f"intent: {intent.value if intent else 'none'}"
        )

        return response

    def _get_or_create_conversation(
        self,
        tenant_id: str,
        conversation_id: str | None,
    ) -> Conversation:
        """Load existing or create new conversation."""
        if conversation_id and self.conversation_repo:
            existing = self.conversation_repo.find_by_id(
                conversation_id=conversation_id,
                tenant_id=tenant_id,
            )
            if existing:
                return existing

        # Create new conversation
        return Conversation(
            id=str(uuid.uuid4()),
            tenant_id=tenant_id,
            messages=[],
            created_at=datetime.now(),
            updated_at=datetime.now(),
        )

    def _get_trading_context(self, tenant_id: str) -> TradingContext | None:
        """Gather current trading context."""
        if not self.trading_context:
            return TradingContext(tenant_id=tenant_id)

        try:
            return self.trading_context.get_context(tenant_id)
        except Exception as e:
            logger.warning(f"Failed to get trading context: {e}")
            return TradingContext(tenant_id=tenant_id)

    def _default_system_prompt(self) -> str:
        """Default system prompt without trading context."""
        return """You are Robson, an AI trading assistant for cryptocurrency.
You help traders analyze markets, manage risk, and execute trades.
Always prioritize risk management and never encourage reckless trading.

RULES:
1. Always recommend stop-loss for any trade
2. Maximum risk per trade: 1% of capital
3. Maximum monthly drawdown: 4%
4. If user wants to trade, ask for confirmation before executing
5. Provide clear, actionable advice
6. Respond in the same language as the user (Portuguese or English)
"""

    def _detect_intent(
        self,
        user_message: str,
        ai_response: str,
    ) -> IntentType | None:
        """Detect trading intent from messages."""
        user_lower = user_message.lower()

        # Buy intent
        if any(
            word in user_lower
            for word in [
                "comprar",
                "buy",
                "compra",
                "entrada",
                "long",
                "abrir posicao",
                "open position",
            ]
        ):
            return IntentType.BUY

        # Sell intent
        if any(
            word in user_lower
            for word in ["vender", "sell", "venda", "sair", "fechar", "realizar", "close", "short"]
        ):
            return IntentType.SELL

        # Analysis intent
        if any(
            word in user_lower
            for word in [
                "analisar",
                "analyze",
                "analise",
                "analysis",
                "grafico",
                "chart",
                "tendencia",
                "trend",
            ]
        ):
            return IntentType.ANALYZE

        # Balance intent
        if any(word in user_lower for word in ["saldo", "balance", "quanto tenho", "patrimonio"]):
            return IntentType.BALANCE

        # Positions intent
        if any(word in user_lower for word in ["posicoes", "positions", "posicao", "aberto"]):
            return IntentType.POSITIONS

        # Risk intent
        if any(word in user_lower for word in ["risco", "risk", "stop", "drawdown"]):
            return IntentType.RISK

        return IntentType.GENERAL

    def _extract_action(
        self,
        intent: IntentType | None,
        ai_response: str,
    ) -> dict[str, Any] | None:
        """Extract suggested action from response."""
        if not intent or intent == IntentType.GENERAL:
            return None

        # For now, return basic action structure
        # In future, parse AI response for specific values
        return {
            "intent": intent.value,
            "requires_confirmation": intent in [IntentType.BUY, IntentType.SELL],
        }

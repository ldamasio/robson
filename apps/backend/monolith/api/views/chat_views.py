"""
Chat API views for Robson AI Assistant.

Endpoints for conversational AI trading assistance.
"""

import logging
from decimal import Decimal

from core.adapters.driven.ai import GroqAdapter
from core.application.use_cases import ChatWithRobsonUseCase
from core.domain.conversation import TradingContext
from rest_framework import status
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response

logger = logging.getLogger(__name__)


def _get_trading_context_for_user(user) -> TradingContext:
    """
    Gather trading context for a user.

    This fetches real-time data about the user's trading state.
    """
    from api.application.adapters import BinanceMarketData
    from api.models import MarginPosition, Order, Trade

    tenant_id = str(user.id)
    context = TradingContext(tenant_id=tenant_id)

    try:
        # Get balances from Binance
        market_data = BinanceMarketData()

        # Get open positions
        positions = MarginPosition.objects.filter(
            client__user=user,
            status=MarginPosition.Status.OPEN,
        )

        for pos in positions:
            try:
                current_price = market_data.ticker_price(pos.symbol)
                entry_price = pos.entry_price
                pnl_pct = ((current_price - entry_price) / entry_price * 100) if entry_price else 0

                context.positions.append(
                    {
                        "symbol": pos.symbol,
                        "side": pos.side,
                        "quantity": str(pos.quantity),
                        "entry_price": str(entry_price),
                        "current_price": str(current_price),
                        "pnl_percent": float(pnl_pct),
                    }
                )

                context.current_prices[pos.symbol] = current_price
            except Exception as e:
                logger.warning(f"Failed to get price for {pos.symbol}: {e}")

        # Get recent trades
        recent_trades = Trade.objects.filter(
            symbol__in=Order.objects.filter(client__user=user).values("symbol")
        ).order_by("-entry_time")[:5]

        for trade in recent_trades:
            if trade.exit_price and trade.entry_price:
                pnl = (trade.exit_price - trade.entry_price) * trade.quantity
            else:
                pnl = Decimal("0")

            context.recent_trades.append(
                {
                    "symbol": trade.symbol.name if trade.symbol else "UNKNOWN",
                    "side": trade.side,
                    "pnl": float(pnl),
                }
            )

    except Exception as e:
        logger.error(f"Failed to build trading context: {e}")

    return context


@api_view(["POST"])
@permission_classes([IsAuthenticated])
def chat(request):
    """
    Send a message to Robson AI and get a response.

    POST /api/chat/

    Request body:
        - message: string (required) - The user's message
        - conversation_id: string (optional) - Continue existing conversation

    Returns:
        - message: The AI assistant's response
        - conversation_id: ID for continuing this conversation
        - detected_intent: What the user wants to do (if trading-related)
        - requires_confirmation: Whether action needs user confirmation
    """
    try:
        # Validate input
        message = request.data.get("message")
        conversation_id = request.data.get("conversation_id")

        if not message:
            return Response(
                {
                    "success": False,
                    "error": "message is required",
                },
                status=status.HTTP_400_BAD_REQUEST,
            )

        if len(message) > 4000:
            return Response(
                {
                    "success": False,
                    "error": "message too long (max 4000 characters)",
                },
                status=status.HTTP_400_BAD_REQUEST,
            )

        # Initialize AI provider
        try:
            ai_provider = GroqAdapter()
        except Exception as e:
            logger.error(f"Failed to initialize AI provider: {e}")
            return Response(
                {
                    "success": False,
                    "error": "AI service unavailable",
                },
                status=status.HTTP_503_SERVICE_UNAVAILABLE,
            )

        # Create use case (without persistence for now)
        use_case = ChatWithRobsonUseCase(
            ai_provider=ai_provider,
            conversation_repo=None,  # TODO: Add persistence
            trading_context=None,  # We'll inject context directly
        )

        # Get trading context
        context = _get_trading_context_for_user(request.user)

        # For now, we pass context via modified execute
        # (In future, inject via TradingContextPort)
        tenant_id = str(request.user.id)

        # Execute chat
        response = use_case.execute(
            tenant_id=tenant_id,
            user_message=message,
            conversation_id=conversation_id,
        )

        return Response(
            {
                "success": True,
                "message": response.message.content,
                "conversation_id": response.message.metadata.get("conversation_id"),
                "detected_intent": (
                    response.detected_intent.value if response.detected_intent else None
                ),
                "requires_confirmation": response.requires_confirmation,
                "model": ai_provider.get_model_name(),
            }
        )

    except Exception as e:
        logger.error(f"Chat error: {e}", exc_info=True)
        return Response(
            {
                "success": False,
                "error": str(e),
            },
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def chat_status(request):
    """
    Check AI chat service status.

    GET /api/chat/status/

    Returns:
        - available: Whether the AI service is available
        - model: The model being used
        - provider: The AI provider name
    """
    try:
        ai_provider = GroqAdapter()
        model = ai_provider.get_model_name()

        return Response(
            {
                "available": True,
                "model": model,
                "provider": "Groq",
            }
        )

    except Exception as e:
        logger.error(f"Chat status check failed: {e}")
        return Response(
            {
                "available": False,
                "error": str(e),
                "provider": "Groq",
            }
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def chat_context(request):
    """
    Get current trading context for chat.

    GET /api/chat/context/

    Returns:
        - context: Current trading context data
    """
    try:
        context = _get_trading_context_for_user(request.user)

        return Response(
            {
                "success": True,
                "context": {
                    "balances": {k: str(v) for k, v in context.balances.items()},
                    "positions": context.positions,
                    "recent_trades": context.recent_trades,
                    "current_prices": {k: str(v) for k, v in context.current_prices.items()},
                    "monthly_pnl": str(context.monthly_pnl),
                },
            }
        )

    except Exception as e:
        logger.error(f"Failed to get chat context: {e}")
        return Response(
            {
                "success": False,
                "error": str(e),
            },
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )

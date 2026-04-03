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


def _get_tenant_id_for_user(user) -> str:
    """Use client/tenant identifier when available, otherwise fall back to user id."""
    return str(getattr(user, "client_id", None) or user.id)


def _classify_margin_health(margin_level: Decimal | None) -> str:
    """Classify margin level into a simple health label."""
    if margin_level is None:
        return "UNKNOWN"
    if margin_level >= Decimal("2.0"):
        return "SAFE"
    if margin_level >= Decimal("1.5"):
        return "CAUTION"
    if margin_level >= Decimal("1.3"):
        return "WARNING"
    if margin_level >= Decimal("1.1"):
        return "CRITICAL"
    return "DANGER"


def _normalize_history(history_payload) -> list[dict[str, str]]:
    """
    Validate transient history sent by the frontend.

    History is intentionally lightweight and short-lived until we add
    server-side conversation persistence.
    """
    if history_payload in (None, ""):
        return []
    if not isinstance(history_payload, list):
        raise ValueError("history must be a list")

    normalized = []
    for item in history_payload[-20:]:
        if not isinstance(item, dict):
            continue

        role = item.get("role")
        content = item.get("content")

        if role not in {"user", "assistant"}:
            continue
        if not isinstance(content, str):
            continue

        content = content.strip()
        if not content:
            continue

        normalized.append({"role": role, "content": content[:4000]})

    return normalized


class _StaticTradingContextProvider:
    """Temporary adapter for request-scoped trading context injection."""

    def __init__(self, context: TradingContext):
        self._context = context

    def get_context(self, tenant_id: str) -> TradingContext:
        return self._context

    def get_current_price(self, symbol: str) -> Decimal:
        return self._context.current_prices.get(symbol, Decimal("0"))

    def get_balances(self, tenant_id: str) -> dict[str, Decimal]:
        return self._context.balances


def _get_trading_context_for_user(user) -> TradingContext:
    """
    Gather trading context for a user.

    This fetches real-time data about the user's trading state.
    """
    from api.application.adapters import BinanceExecution
    from api.models import MarginPosition, Order, Trade
    from api.services.market_price_cache import get_cached_bid
    from api.views.margin_views import _get_adapter
    from api.views.risk_managed_trading import _get_monthly_pnl

    tenant_id = _get_tenant_id_for_user(user)
    context = TradingContext(tenant_id=tenant_id)

    balance_errors = []
    margin_level_values = []
    critical_positions = []
    highest_risk_percent = Decimal("0")

    try:
        try:
            balance_data = BinanceExecution().get_account_balance()
            for balance in balance_data.get("balances", []):
                total = (balance.get("free") or Decimal("0")) + (
                    balance.get("locked") or Decimal("0")
                )
                if total > 0:
                    context.balances[str(balance["asset"])] = total
        except Exception as e:
            balance_errors.append(str(e))
            logger.warning("Failed to get account balances for chat context: %s", e)

        try:
            margin_adapter = _get_adapter()
        except Exception as e:
            margin_adapter = None
            logger.warning("Failed to initialize margin adapter for chat context: %s", e)

        # Get open positions
        positions = MarginPosition.objects.filter(
            status=MarginPosition.Status.OPEN,
        )
        if getattr(user, "client_id", None):
            positions = positions.filter(client_id=user.client_id)
        else:
            positions = positions.filter(client__user=user)

        for pos in positions:
            try:
                symbol = pos.symbol.replace("/", "").upper()
                current_price = get_cached_bid(symbol) or pos.current_price or pos.entry_price
                entry_price = pos.entry_price
                margin_level = pos.margin_level
                liquidation_price = Decimal("0")

                if margin_adapter:
                    try:
                        account_snapshot = margin_adapter.get_margin_account(symbol)
                        margin_level = account_snapshot.margin_level
                        liquidation_price = account_snapshot.liquidation_price
                    except Exception as e:
                        logger.warning("Failed to sync margin account for %s: %s", symbol, e)

                if pos.side in {MarginPosition.Side.SHORT, "SHORT", "SELL"}:
                    unrealized_pnl = (entry_price - current_price) * pos.quantity
                else:
                    unrealized_pnl = (current_price - entry_price) * pos.quantity

                cost_basis = entry_price * pos.quantity if entry_price and pos.quantity else Decimal("0")
                pnl_pct = (unrealized_pnl / cost_basis * Decimal("100")) if cost_basis else Decimal("0")
                margin_health = _classify_margin_health(margin_level)

                context.positions.append(
                    {
                        "symbol": symbol,
                        "side": pos.side,
                        "quantity": str(pos.quantity),
                        "entry_price": str(entry_price),
                        "current_price": str(current_price),
                        "pnl_percent": float(pnl_pct),
                        "unrealized_pnl": str(unrealized_pnl),
                        "stop_price": str(pos.stop_price),
                        "risk_percent": str(pos.risk_percent),
                        "margin_level": str(margin_level),
                        "margin_health": margin_health,
                        "liquidation_price": str(liquidation_price),
                    }
                )

                context.current_prices[symbol] = current_price
                margin_level_values.append(margin_level)
                highest_risk_percent = max(highest_risk_percent, pos.risk_percent or Decimal("0"))
                if margin_health in {"CRITICAL", "DANGER"}:
                    critical_positions.append(symbol)
            except Exception as e:
                logger.warning(f"Failed to get price for {pos.symbol}: {e}")

        # Get recent trades
        if getattr(user, "client_id", None):
            recent_trades = Trade.objects.filter(client_id=user.client_id)
        else:
            recent_trades = Trade.objects.filter(
                symbol__in=Order.objects.filter(client__user=user).values("symbol")
            )
        recent_trades = recent_trades.order_by("-entry_time")[:5]

        for trade in recent_trades:
            context.recent_trades.append(
                {
                    "symbol": trade.symbol.name if trade.symbol else "UNKNOWN",
                    "side": trade.side,
                    "pnl": float(trade.pnl or Decimal("0")),
                }
            )

        try:
            context.monthly_pnl = _get_monthly_pnl(getattr(user, "client_id", None))
        except Exception as e:
            logger.warning("Failed to get monthly P&L for chat context: %s", e)

        context.risk_metrics = {
            "open_positions_count": len(context.positions),
            "critical_positions": critical_positions,
            "lowest_margin_level": (
                str(min(margin_level_values)) if margin_level_values else None
            ),
            "highest_risk_percent": str(highest_risk_percent),
            "monthly_pnl_state": "profit" if context.monthly_pnl >= 0 else "drawdown",
        }
        if balance_errors:
            context.risk_metrics["balance_warnings"] = balance_errors

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
        - history: list (optional) - Recent messages for transient continuity

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

        try:
            history = _normalize_history(request.data.get("history"))
        except ValueError as e:
            return Response(
                {
                    "success": False,
                    "error": str(e),
                },
                status=status.HTTP_400_BAD_REQUEST,
            )

        if not isinstance(message, str) or not message.strip():
            return Response(
                {
                    "success": False,
                    "error": "message is required",
                },
                status=status.HTTP_400_BAD_REQUEST,
            )
        message = message.strip()

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
        context = _get_trading_context_for_user(request.user)
        use_case = ChatWithRobsonUseCase(
            ai_provider=ai_provider,
            conversation_repo=None,  # TODO: Add persistence
            trading_context=_StaticTradingContextProvider(context),
        )

        tenant_id = _get_tenant_id_for_user(request.user)

        # Execute chat
        response = use_case.execute(
            tenant_id=tenant_id,
            user_message=message,
            conversation_id=conversation_id,
            history=history,
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
                "context_summary": {
                    "balances_count": len(context.balances),
                    "open_positions_count": len(context.positions),
                    "monthly_pnl": str(context.monthly_pnl),
                    "lowest_margin_level": context.risk_metrics.get("lowest_margin_level"),
                },
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
                    "risk_metrics": context.risk_metrics,
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


# api/views/strategy_views.py
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from ..models import Strategy
from ..serializers import StrategySerializer
from .base import BaseAPIView

# Pre-defined strategies available for all clients
DEFAULT_STRATEGIES = [
    {
        "name": "All In",
        "description": "Go all-in with technical stop precision. Buy maximum position size with stop at second technical support (15m chart).",
        "config": {
            "timeframe": "15m",
            "indicators": ["Support/Resistance", "Technical Stop"],
            "entry_type": "manual",
            "risk_percent": 1.0,
            "use_technical_stop": True,
            "leverage": 3,
            "account_type": "isolated_margin"
        },
        "risk_config": {
            "max_risk_per_trade": 1.0,
            "use_technical_stop": True,
            "stop_placement": "second_support_15m"
        }
    },
    {
        "name": "Rescue Forces",
        "description": "Automatic rescue on bullish momentum. Enters when MA4 crosses above MA9 with short-term uptrend confirmed.",
        "config": {
            "timeframe": "15m",
            "indicators": ["MA4", "MA9", "Trend"],
            "entry_type": "auto",
            "entry_conditions": {
                "ma_cross": "MA4 > MA9",
                "trend": "short_term_bullish",
                "confirmation": "volume_spike"
            },
            "risk_percent": 1.0,
            "leverage": 3,
            "account_type": "isolated_margin"
        },
        "risk_config": {
            "max_risk_per_trade": 1.0,
            "use_technical_stop": True,
            "stop_placement": "below_ma9"
        }
    },
    {
        "name": "Smooth Sailing",
        "description": "Ride the calm waves of trending markets with moving average crossovers.",
        "config": {
            "timeframe": "1h",
            "indicators": ["MA50", "MA200"],
            "entry_type": "trend",
            "risk_percent": 0.5,
            "account_type": "spot"
        }
    },
    {
        "name": "Bounce Back",
        "description": "Catch the bounce when price returns to mean in range-bound markets.",
        "config": {
            "timeframe": "30m",
            "indicators": ["Bollinger Bands", "RSI"],
            "entry_type": "reversion",
            "risk_percent": 0.5,
            "account_type": "spot"
        }
    }
]

@api_view(['GET'])
@permission_classes([IsAuthenticated])
def get_strategies(request):
    """Get strategies for authenticated client.

    Returns all strategies associated with the client.
    If no strategies exist, creates default strategies automatically.
    """
    try:
        # Multi-tenant: get client from authenticated user
        client_id = request.user.client.id if hasattr(request.user, 'client') else None

        if not client_id:
            return Response({"error": "Client not found"}, status=400)

        client = request.user.client

        # Check if client has strategies, create defaults if not
        existing_count = Strategy.objects.filter(client=client).count()
        if existing_count == 0:
            # Auto-create default strategies for new clients
            for strategy_data in DEFAULT_STRATEGIES:
                Strategy.objects.create(
                    client=client,
                    name=strategy_data["name"],
                    description=strategy_data["description"],
                    config=strategy_data.get("config", {}),
                    risk_config=strategy_data.get("risk_config", {}),
                    is_active=True
                )

        strategies = Strategy.objects.filter(client=client)
        serializer = StrategySerializer(strategies, many=True)

        return Response(serializer.data)
    except Exception as e:
        return Response(
            {"error": f"Failed to get strategies: {str(e)}"},
            status=500
        )

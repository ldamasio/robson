
# api/views/market_views.py
from decimal import Decimal, ROUND_HALF_UP

from django.views.decorators.cache import cache_page
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response

from ..services import BinanceService, MarketDataService
from ..services.market_price_cache import get_cached_quotes
from .base import BaseAPIView

class MarketViews(BaseAPIView):
    
    def __init__(self):
        self.binance_service = BinanceService()
        self.market_service = MarketDataService()

def _format_usd(value: Decimal) -> str:
    return str(value.quantize(Decimal("0.01"), rounding=ROUND_HALF_UP))

@api_view(['GET'])
def ping(request):
    """Testa conexão com Binance"""
    try:
        service = BinanceService()
        result = service.ping()
        return Response({"pong": result})
    except Exception as e:
        return Response(
            {"error": "Failed to ping Binance"}, 
            status=500
        )

@api_view(['GET'])
def server_time(request):
    """Obtém tempo do servidor"""
    try:
        service = BinanceService()
        result = service.get_server_time()
        return Response({"time": result})
    except Exception as e:
        return Response(
            {"error": "Failed to get server time"}, 
            status=500
        )

@api_view(['GET'])
@permission_classes([IsAuthenticated])
def historical_data(request):
    """Obtém dados históricos"""
    try:
        symbol = request.GET.get('symbol', 'BTCUSDT')
        interval = request.GET.get('interval', '1d')
        days = int(request.GET.get('days', 7))
        
        service = MarketDataService()
        result = service.get_historical_data(symbol, interval, days)
        
        return Response({"data": result})
    except Exception as e:
        return Response(
            {"error": "Failed to get historical data"}, 
            status=500
        )

@api_view(['GET'])
@permission_classes([IsAuthenticated])
@cache_page(1)
def current_price(request, symbol):
    """Get current price data with bid/ask/last."""
    try:
        normalized_symbol = symbol.upper()
        quotes = get_cached_quotes(normalized_symbol)
        bid = quotes["bid"]
        ask = quotes["ask"]
        last = (bid + ask) / Decimal("2")

        return Response({
            "symbol": normalized_symbol,
            "bid": _format_usd(bid),
            "ask": _format_usd(ask),
            "last": _format_usd(last),
            "timestamp": quotes["timestamp"],
            "source": "binance",
        })
    except Exception:
        return Response(
            {"error": "Failed to get current price"},
            status=500
        )


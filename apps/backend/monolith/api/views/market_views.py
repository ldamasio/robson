
# api/views/market_views.py
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from ..services import BinanceService, MarketDataService
from .base import BaseAPIView

class MarketViews(BaseAPIView):
    
    def __init__(self):
        self.binance_service = BinanceService()
        self.market_service = MarketDataService()

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


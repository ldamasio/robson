
# api/views/strategy_views.py
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from ..models import Strategy
from ..serializers import StrategySerializer
from .base import BaseAPIView

@api_view(['GET'])
@permission_classes([IsAuthenticated])
def get_strategies(request):
    """Obtém estratégias do cliente"""
    try:
        # Multi-tenant
        client_id = request.user.client.id if hasattr(request.user, 'client') else None
        
        if not client_id:
            return Response({"error": "Client not found"}, status=400)
        
        strategies = Strategy.objects.filter(client_id=client_id)
        serializer = StrategySerializer(strategies, many=True)
        
        return Response(serializer.data)
    except Exception as e:
        return Response(
            {"error": "Failed to get strategies"}, 
            status=500
        )

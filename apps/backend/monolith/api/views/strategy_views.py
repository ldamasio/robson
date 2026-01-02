# api/views/strategy_views.py
from django.db.models import Q
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from ..models import Strategy
from ..serializers import StrategySerializer
from .base import BaseAPIView


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def get_strategies(request):
    """
    Get strategies for authenticated client.

    Returns:
    - Global system templates (client=null): Available to all users
    - User's custom strategies (client=user.client): Created by the user

    Strategies are templates that define trading approach, risk parameters,
    and configuration. They are NOT auto-trading algorithms.

    To create global strategies, run: python manage.py create_global_strategies
    """
    try:
        # Multi-tenant: get client from authenticated user
        client = request.user.client if hasattr(request.user, 'client') else None

        if not client:
            return Response({"error": "Client not found"}, status=400)

        # Return global strategies (system templates) + user's custom strategies
        # Q(client__isnull=True) = global templates available to everyone
        # Q(client=client) = user's personalized strategies
        strategies = Strategy.objects.filter(
            Q(client__isnull=True) | Q(client=client)
        ).order_by('-is_active', 'name')

        serializer = StrategySerializer(strategies, many=True)
        return Response(serializer.data)

    except Exception as e:
        return Response(
            {"error": f"Failed to get strategies: {str(e)}"},
            status=500
        )

# api/views/symbol_views.py
from django.db.models import Q
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from ..models import Symbol
from ..serializers.symbol_serializers import SymbolSerializer


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def list_symbols(request):
    """
    List trading symbols (pairs) available for trading.

    Returns:
    - Global symbols (client=null): Available to all users
    - User's custom symbols (client=user.client): Created by the user

    Symbols represent trading pairs like BTC/USDT, ETH/USDT, etc.

    To create global symbols, run: python manage.py create_global_symbols
    """
    try:
        # Multi-tenant: get client from authenticated user
        client = request.user.client if hasattr(request.user, 'client') else None

        if not client:
            return Response({"error": "Client not found"}, status=400)

        # Return global symbols + user's custom symbols
        # Q(client__isnull=True) = global symbols available to everyone
        # Q(client=client) = user's personalized symbols
        symbols = Symbol.objects.filter(
            Q(client__isnull=True) | Q(client=client)
        ).filter(is_active=True).order_by('base_asset', 'quote_asset')

        serializer = SymbolSerializer(symbols, many=True)
        return Response(serializer.data)

    except Exception as e:
        return Response(
            {"error": f"Failed to get symbols: {str(e)}"},
            status=500
        )

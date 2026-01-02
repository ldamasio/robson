"""
Operation API Views (Read-Only).

These endpoints allow users to query their active operations (Level 2 in hierarchy).
Operations are created from LIVE TradingIntent executions.

All endpoints require authentication and enforce multi-tenant isolation.
"""

from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status
import logging

from api.models import Operation
from api.serializers.operation_serializers import OperationSerializer

logger = logging.getLogger(__name__)


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def list_operations(request):
    """
    List operations for the authenticated user's client.

    GET /api/operations/

    Query parameters:
        status (str, optional): Filter by status (PLANNED, ACTIVE, CLOSED, CANCELLED)
        strategy (int, optional): Filter by strategy ID
        symbol (int, optional): Filter by symbol ID
        limit (int, optional): Maximum results (default: 100)
        offset (int, optional): Pagination offset (default: 0)

    Returns:
        200 OK: List of Operation objects
        400 Bad Request: Invalid parameters
        500 Internal Server Error: Unexpected error
    """
    try:
        # Get client from user
        client = request.user.client
        if not client:
            return Response(
                {"error": "User has no associated client"},
                status=status.HTTP_400_BAD_REQUEST
            )

        # Parse query parameters
        status_filter = request.query_params.get("status")
        strategy_id = request.query_params.get("strategy")
        symbol_id = request.query_params.get("symbol")

        try:
            limit = int(request.query_params.get("limit", 100))
            offset = int(request.query_params.get("offset", 0))
        except ValueError:
            return Response(
                {"error": "limit and offset must be integers"},
                status=status.HTTP_400_BAD_REQUEST
            )

        # Validate limit
        if limit < 1 or limit > 1000:
            return Response(
                {"error": "limit must be between 1 and 1000"},
                status=status.HTTP_400_BAD_REQUEST
            )

        # Build query
        operations = Operation.objects.filter(client=client)

        if status_filter:
            operations = operations.filter(status=status_filter)

        if strategy_id:
            operations = operations.filter(strategy_id=int(strategy_id))

        if symbol_id:
            operations = operations.filter(symbol_id=int(symbol_id))

        # Apply ordering and pagination
        operations = operations.order_by('-created_at')[offset:offset + limit]

        # Serialize response
        serializer = OperationSerializer(operations, many=True)

        return Response(
            {
                "count": len(operations),
                "results": serializer.data,
            },
            status=status.HTTP_200_OK
        )

    except Exception as e:
        logger.error(f"Unexpected error listing operations: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def get_operation(request, operation_id):
    """
    Get a single operation by ID.

    GET /api/operations/{operation_id}/

    Returns:
        200 OK: Operation object with related movements
        404 Not Found: Operation not found
        500 Internal Server Error: Unexpected error
    """
    try:
        # Get client from user
        client = request.user.client
        if not client:
            return Response(
                {"error": "User has no associated client"},
                status=status.HTTP_400_BAD_REQUEST
            )

        # Get operation with multi-tenant filtering
        try:
            operation = Operation.objects.get(id=operation_id, client=client)
        except Operation.DoesNotExist:
            return Response(
                {"error": "Operation not found"},
                status=status.HTTP_404_NOT_FOUND
            )

        # Serialize response
        serializer = OperationSerializer(operation)

        return Response(serializer.data, status=status.HTTP_200_OK)

    except Exception as e:
        logger.error(f"Unexpected error fetching operation: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )

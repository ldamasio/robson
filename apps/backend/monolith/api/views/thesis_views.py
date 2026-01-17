"""
Thesis API views for Robson Trading Thesis feature.

Endpoints for creating and managing trading theses through the chat interface.
A Trading Thesis is a structured market hypothesis that does NOT execute orders.
"""

import logging
from datetime import datetime

from rest_framework import status
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response

from core.domain.thesis import (
    ThesisStatus,
    TradingThesis,
    get_template,
    list_templates,
)

logger = logging.getLogger(__name__)


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def thesis_templates(request):
    """
    Get available thesis templates.

    GET /api/thesis/templates/

    Returns a list of pre-defined templates for common thesis patterns
    (breakout, mean reversion, trend following, etc.).

    Templates help users structure their theses with suggested wording.
    """
    try:
        templates = list_templates()

        return Response(
            {
                "success": True,
                "templates": [t.to_dict() for t in templates],
            }
        )

    except Exception as e:
        logger.error(f"Failed to get thesis templates: {e}")
        return Response(
            {
                "success": False,
                "error": str(e),
            },
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["POST"])
@permission_classes([IsAuthenticated])
def create_thesis(request):
    """
    Create a new trading thesis.

    POST /api/thesis/create/

    Request body:
        - title: string (required) - Thesis title
        - symbol: string (required) - Trading symbol (e.g., BTCUSDT)
        - timeframe: string (required) - Timeframe (e.g., 4h, 1d)
        - market_context: string (required) - What is happening in the market
        - rationale: string (required) - Why this opportunity might exist
        - expected_trigger: string (required) - What confirms the thesis
        - invalidation: string (required) - What proves it wrong
        - hypothesis_type: string (optional) - Type: breakout, mean_reversion, etc.
        - confidence_level: string (optional) - low, medium, high
        - tags: array (optional) - Tags for categorization
        - notes: string (optional) - Additional notes

    Returns:
        - success: boolean
        - thesis: The created thesis object
        - thesis_id: ID for future reference
    """
    try:
        from api.models import TradingThesisModel

        # Get the client for this user
        client = request.user.client

        # Validate required fields
        required_fields = [
            "title",
            "symbol",
            "timeframe",
            "market_context",
            "rationale",
            "expected_trigger",
            "invalidation",
        ]

        missing_fields = [f for f in required_fields if not request.data.get(f)]
        if missing_fields:
            return Response(
                {
                    "success": False,
                    "error": f"Missing required fields: {', '.join(missing_fields)}",
                },
                status=status.HTTP_400_BAD_REQUEST,
            )

        # Create thesis model
        thesis_model = TradingThesisModel(
            client=client,
            title=request.data["title"],
            symbol=request.data["symbol"].upper(),
            timeframe=request.data["timeframe"],
            market_context=request.data["market_context"],
            rationale=request.data["rationale"],
            expected_trigger=request.data["expected_trigger"],
            invalidation=request.data["invalidation"],
            hypothesis_type=request.data.get("hypothesis_type"),
            confidence_level=request.data.get("confidence_level"),
            tags=request.data.get("tags", []),
            notes=request.data.get("notes"),
            status="draft",  # Always start as draft
        )

        # Validate and save
        thesis_model.full_clean()
        thesis_model.save()

        # Convert to domain entity
        thesis_domain = thesis_model.to_domain()

        logger.info(
            f"Created thesis {thesis_model.id} for user {request.user.id}: {thesis_model.title}"
        )

        return Response(
            {
                "success": True,
                "thesis": thesis_domain.to_dict(),
                "thesis_id": str(thesis_model.id),
            },
            status=status.HTTP_201_CREATED,
        )

    except Exception as e:
        logger.error(f"Failed to create thesis: {e}", exc_info=True)
        return Response(
            {
                "success": False,
                "error": str(e),
            },
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def list_theses(request):
    """
    List user's trading theses.

    GET /api/thesis/

    Query parameters:
        - status: Filter by status (draft, active, validated, rejected, expired, converted)
        - symbol: Filter by symbol (e.g., BTCUSDT)
        - limit: Maximum number of results (default: 50)

    Returns:
        - success: boolean
        - theses: List of thesis objects
        - count: Total number of theses
    """
    try:
        from api.models import TradingThesisModel

        client = request.user.client
        queryset = TradingThesisModel.objects.filter(client=client)

        # Apply filters
        status_filter = request.query_params.get("status")
        if status_filter:
            queryset = queryset.filter(status=status_filter)

        symbol_filter = request.query_params.get("symbol")
        if symbol_filter:
            queryset = queryset.filter(symbol__icontains=symbol_filter.upper())

        # Order by most recent first
        queryset = queryset.order_by("-created_at")

        # Apply limit
        limit = int(request.query_params.get("limit", 50))
        queryset = queryset[:limit]

        theses = [t.to_domain().to_dict() for t in queryset]

        return Response(
            {
                "success": True,
                "theses": theses,
                "count": len(theses),
            }
        )

    except Exception as e:
        logger.error(f"Failed to list theses: {e}", exc_info=True)
        return Response(
            {
                "success": False,
                "error": str(e),
            },
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def get_thesis(request, thesis_id):
    """
    Get a specific thesis by ID.

    GET /api/thesis/{thesis_id}/

    Returns:
        - success: boolean
        - thesis: The thesis object
    """
    try:
        from api.models import TradingThesisModel

        client = request.user.client

        try:
            thesis_model = TradingThesisModel.objects.get(id=thesis_id, client=client)
        except TradingThesisModel.DoesNotExist:
            return Response(
                {
                    "success": False,
                    "error": "Thesis not found",
                },
                status=status.HTTP_404_NOT_FOUND,
            )

        thesis_domain = thesis_model.to_domain()

        return Response(
            {
                "success": True,
                "thesis": thesis_domain.to_dict(),
            }
        )

    except Exception as e:
        logger.error(f"Failed to get thesis {thesis_id}: {e}", exc_info=True)
        return Response(
            {
                "success": False,
                "error": str(e),
            },
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["POST"])
@permission_classes([IsAuthenticated])
def update_thesis_status(request, thesis_id):
    """
    Update thesis status.

    POST /api/thesis/{thesis_id}/status/

    Request body:
        - status: string (required) - New status: activate, validate, reject

    Returns:
        - success: boolean
        - thesis: The updated thesis object
    """
    try:
        from api.models import TradingThesisModel

        client = request.user.client

        try:
            thesis_model = TradingThesisModel.objects.get(id=thesis_id, client=client)
        except TradingThesisModel.DoesNotExist:
            return Response(
                {
                    "success": False,
                    "error": "Thesis not found",
                },
                status=status.HTTP_404_NOT_FOUND,
            )

        action = request.data.get("status")

        if action == "activate":
            thesis_model.activate()
        elif action == "validate":
            thesis_model.validate()
        elif action == "reject":
            thesis_model.reject()
        else:
            return Response(
                {
                    "success": False,
                    "error": f"Invalid action: {action}. Use: activate, validate, or reject",
                },
                status=status.HTTP_400_BAD_REQUEST,
            )

        thesis_domain = thesis_model.to_domain()

        logger.info(
            f"Updated thesis {thesis_id} status to {thesis_model.status} for user {request.user.id}"
        )

        return Response(
            {
                "success": True,
                "thesis": thesis_domain.to_dict(),
            }
        )

    except Exception as e:
        logger.error(f"Failed to update thesis status: {e}", exc_info=True)
        return Response(
            {
                "success": False,
                "error": str(e),
            },
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def thesis_summary(request):
    """
    Get a summary of user's thesis journal.

    GET /api/thesis/summary/

    Returns counts and statistics about the user's theses.

    Returns:
        - success: boolean
        - summary: Dictionary with counts by status, symbols, etc.
    """
    try:
        from api.models import TradingThesisModel
        from django.db.models import Count

        client = request.user.client

        # Get counts by status
        status_counts = (
            TradingThesisModel.objects.filter(client=client)
            .values("status")
            .annotate(count=Count("id"))
        )

        # Get total count
        total_count = TradingThesisModel.objects.filter(client=client).count()

        # Get unique symbols
        symbols = (
            TradingThesisModel.objects.filter(client=client)
            .values_list("symbol", flat=True)
            .distinct()
            .order_by("symbol")
        )

        # Get recent activity
        recent_theses = (
            TradingThesisModel.objects.filter(client=client)
            .order_by("-updated_at")[:5]
        )

        summary = {
            "total_count": total_count,
            "by_status": {item["status"]: item["count"] for item in status_counts},
            "symbols": list(symbols),
            "recent_activity": [t.to_domain().to_dict() for t in recent_theses],
        }

        return Response(
            {
                "success": True,
                "summary": summary,
            }
        )

    except Exception as e:
        logger.error(f"Failed to get thesis summary: {e}", exc_info=True)
        return Response(
            {
                "success": False,
                "error": str(e),
            },
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )

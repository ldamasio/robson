"""
Trading Intent API Views.

These views implement the REST endpoints for the agentic workflow:
PLAN → VALIDATE → EXECUTE

All endpoints require JWT authentication and enforce multi-tenant isolation.
"""

from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status
from django.utils import timezone
import logging

from api.serializers.trading_intent_serializers import (
    CreateTradingIntentSerializer,
    TradingIntentSerializer,
    ValidationReportSerializer,
    ExecutionResultSerializer,
)
from api.application.use_cases.trading_intent import (
    CreateTradingIntentCommand,
    CreateTradingIntentUseCase,
)
from api.application.adapters import (
    DjangoSymbolRepository,
    DjangoStrategyRepository,
    DjangoTradingIntentRepository,
)
from api.application.validation_framework import ValidationFramework
from api.application.execution_framework import ExecutionFramework
from api.application.execution import ExecutionMode
from api.models import TradingIntent

logger = logging.getLogger(__name__)


@api_view(["POST"])
@permission_classes([IsAuthenticated])
def create_trading_intent(request):
    """
    Create a new trading intent (PLAN step).

    POST /api/trading-intents/create/

    Request body:
        symbol (int): Symbol ID
        strategy (int): Strategy ID
        side (str): BUY or SELL
        entry_price (Decimal): Entry price
        stop_price (Decimal): Stop-loss price
        capital (Decimal): Capital allocated
        target_price (Decimal, optional): Take-profit price
        regime (str, optional): Market regime
        confidence (float, optional): Confidence level 0.0-1.0
        reason (str, optional): Reason for this intent

    Returns:
        201 Created: TradingIntent object
        400 Bad Request: Validation errors
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

        # Validate input
        serializer = CreateTradingIntentSerializer(data=request.data)
        if not serializer.is_valid():
            return Response(serializer.errors, status=status.HTTP_400_BAD_REQUEST)

        # Create command
        command = CreateTradingIntentCommand(
            symbol_id=serializer.validated_data["symbol"],
            strategy_id=serializer.validated_data["strategy"],
            side=serializer.validated_data["side"],
            entry_price=serializer.validated_data["entry_price"],
            stop_price=serializer.validated_data["stop_price"],
            capital=serializer.validated_data["capital"],
            target_price=serializer.validated_data.get("target_price"),
            regime=serializer.validated_data.get("regime", "manual"),
            confidence=serializer.validated_data.get("confidence", 0.5),
            reason=serializer.validated_data.get("reason", "Manual entry via UI"),
            client_id=client.id,
        )

        # Execute use case
        use_case = CreateTradingIntentUseCase(
            symbol_repo=DjangoSymbolRepository(),
            strategy_repo=DjangoStrategyRepository(),
            intent_repo=DjangoTradingIntentRepository(),
        )

        intent = use_case.execute(command)

        # Serialize response
        response_serializer = TradingIntentSerializer(intent)

        logger.info(
            f"Created trading intent {intent.intent_id} for client {client.id}: "
            f"{intent.side} {intent.quantity} {intent.symbol.name} @ {intent.entry_price}"
        )

        return Response(response_serializer.data, status=status.HTTP_201_CREATED)

    except ValueError as e:
        logger.warning(f"Validation error creating trading intent: {e}")
        return Response(
            {"error": str(e)},
            status=status.HTTP_400_BAD_REQUEST
        )
    except Exception as e:
        logger.error(f"Unexpected error creating trading intent: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def get_trading_intent(request, intent_id):
    """
    Get a single trading intent by intent_id.

    GET /api/trading-intents/{intent_id}/

    Returns:
        200 OK: TradingIntent object
        404 Not Found: Intent not found
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

        # Get intent with multi-tenant filtering
        repo = DjangoTradingIntentRepository()
        intent = repo.get_by_intent_id(intent_id, client.id)

        # Serialize response
        serializer = TradingIntentSerializer(intent)
        return Response(serializer.data, status=status.HTTP_200_OK)

    except ValueError as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_404_NOT_FOUND
        )
    except Exception as e:
        logger.error(f"Unexpected error fetching trading intent: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def list_trading_intents(request):
    """
    List trading intents for the authenticated user's client.

    GET /api/trading-intents/

    Query parameters:
        status (str, optional): Filter by status
        strategy (int, optional): Filter by strategy ID
        symbol (int, optional): Filter by symbol ID
        limit (int, optional): Maximum results (default: 100)
        offset (int, optional): Pagination offset (default: 0)

    Returns:
        200 OK: List of TradingIntent objects
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

        # Get intents
        repo = DjangoTradingIntentRepository()
        intents = repo.list_by_client(
            client_id=client.id,
            status=status_filter,
            strategy_id=int(strategy_id) if strategy_id else None,
            symbol_id=int(symbol_id) if symbol_id else None,
            limit=limit,
            offset=offset,
        )

        # Serialize response
        serializer = TradingIntentSerializer(intents, many=True)

        return Response(
            {
                "count": len(intents),
                "results": serializer.data,
            },
            status=status.HTTP_200_OK
        )

    except Exception as e:
        logger.error(f"Unexpected error listing trading intents: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(["POST"])
@permission_classes([IsAuthenticated])
def validate_trading_intent(request, intent_id):
    """
    Validate a trading intent (VALIDATE step).

    POST /api/trading-intents/{intent_id}/validate/

    Returns:
        200 OK: ValidationReport
        404 Not Found: Intent not found
        400 Bad Request: Intent already validated or invalid state
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

        # Get intent
        repo = DjangoTradingIntentRepository()
        intent = repo.get_by_intent_id(intent_id, client.id)

        # Validate using the validation framework
        validation_framework = ValidationFramework(client_id=client.id)
        report = validation_framework.validate(intent)

        # Update intent with validation result
        intent.validation_result = report.to_dict()
        if not report.has_failures():
            intent.status = "VALIDATED"
            intent.validated_at = timezone.now()
        intent.save()

        # Serialize response
        serializer = ValidationReportSerializer(report.to_dict())

        logger.info(
            f"Validated trading intent {intent_id} for client {client.id}: "
            f"status={report.status.value}"
        )

        return Response(serializer.data, status=status.HTTP_200_OK)

    except ValueError as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_404_NOT_FOUND
        )
    except Exception as e:
        logger.error(f"Unexpected error validating trading intent: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(["POST"])
@permission_classes([IsAuthenticated])
def execute_trading_intent(request, intent_id):
    """
    Execute a trading intent (EXECUTE step).

    POST /api/trading-intents/{intent_id}/execute/

    Query parameters:
        mode (str, optional): "dry-run" (default) or "live"

    Returns:
        200 OK: ExecutionResult
        404 Not Found: Intent not found
        400 Bad Request: Intent not validated or invalid state
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

        # Parse mode parameter
        mode_str = request.query_params.get("mode", "dry-run")
        if mode_str == "live":
            mode = ExecutionMode.LIVE
        elif mode_str == "dry-run":
            mode = ExecutionMode.DRY_RUN
        else:
            return Response(
                {"error": f"Invalid mode: {mode_str}. Must be 'dry-run' or 'live'"},
                status=status.HTTP_400_BAD_REQUEST
            )

        # Get intent
        repo = DjangoTradingIntentRepository()
        intent = repo.get_by_intent_id(intent_id, client.id)

        # Verify intent is validated
        if intent.status != "VALIDATED":
            return Response(
                {"error": f"Intent must be VALIDATED before execution. Current status: {intent.status}"},
                status=status.HTTP_400_BAD_REQUEST
            )

        # Execute using the execution framework
        execution_framework = ExecutionFramework(client_id=client.id)
        result = execution_framework.execute(intent, mode=mode)

        # Update intent with execution result
        intent.execution_result = result.to_dict()
        if result.is_success():
            intent.status = "EXECUTED"
            intent.executed_at = timezone.now()
        elif result.is_blocked():
            intent.status = "FAILED"
            intent.error_message = "Execution blocked by safety guards"
        else:
            intent.status = "FAILED"
            intent.error_message = result.error
        intent.save()

        # Serialize response
        serializer = ExecutionResultSerializer(result.to_dict())

        logger.info(
            f"Executed trading intent {intent_id} for client {client.id}: "
            f"mode={mode.value}, status={result.status.value}"
        )

        return Response(serializer.data, status=status.HTTP_200_OK)

    except ValueError as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_404_NOT_FOUND
        )
    except Exception as e:
        logger.error(f"Unexpected error executing trading intent: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )

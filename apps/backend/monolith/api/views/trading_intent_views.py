"""
Trading Intent API Views.

These views implement the REST endpoints for the agentic workflow:
PLAN → VALIDATE → EXECUTE

All endpoints require JWT authentication and enforce multi-tenant isolation.
"""

import logging

from django.utils import timezone
from rest_framework import status
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response

from api.application.adapters import (
    BinanceAccountBalanceAdapter,
    DjangoStrategyRepository,
    DjangoSymbolRepository,
    DjangoTradingIntentRepository,
)
from api.application.execution import ExecutionMode
from api.application.execution_framework import ExecutionFramework
from api.application.use_cases.trading_intent import (
    CreateTradingIntentCommand,
    CreateTradingIntentUseCase,
)
from api.application.validation_framework import ValidationFramework
from api.models import TradingIntent
from api.models.trading import PatternTrigger
from api.serializers.trading_intent_serializers import (
    CreateTradingIntentSerializer,
    ExecutionResultSerializer,
    PatternTriggerResponseSerializer,
    PatternTriggerSerializer,
    TradingIntentSerializer,
    ValidationReportSerializer,
)

logger = logging.getLogger(__name__)


@api_view(["POST"])
@permission_classes([IsAuthenticated])
def create_trading_intent(request):
    """
    Create a new trading intent (PLAN step).

    Supports two modes:
    1. Manual mode: All fields provided (side, entry_price, stop_price, capital)
    2. Auto mode: Only symbol and strategy provided, backend auto-calculates all parameters

    POST /api/trading-intents/create/

    Request body (Manual mode):
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

    Request body (Auto mode):
        symbol (int): Symbol ID
        strategy (int): Strategy ID

    Returns:
        201 Created: TradingIntent object
        400 Bad Request: Validation errors
        500 Internal Server Error: Unexpected error
    """
    from decimal import Decimal

    from api.application.technical_stop_adapter import BinanceTechnicalStopService
    from api.models import Strategy, Symbol

    try:
        # Get client from user
        client = request.user.client
                {"error": "User has no associated client"}, status=status.HTTP_400_BAD_REQUEST
            )

        # Validate input
        serializer = CreateTradingIntentSerializer(data=request.data)
        if not serializer.is_valid():
            return Response(serializer.errors, status=status.HTTP_400_BAD_REQUEST)

        validated_data = serializer.validated_data

        # Detect auto mode: explicit mode="auto" OR all manual fields absent
        # Partial payload (some fields missing) = 400 error
        has_side = validated_data.get("side") is not None
        has_entry = validated_data.get("entry_price") is not None
        has_stop = validated_data.get("stop_price") is not None
        has_capital = validated_data.get("capital") is not None

        manual_fields_present = [has_side, has_entry, has_stop, has_capital]
        has_any_manual = any(manual_fields_present)
        has_all_manual = all(manual_fields_present)

        # Check for explicit auto mode
        mode = request.data.get("mode")
        is_explicit_auto = mode == "auto"

        # STRICT: If mode="auto" is explicit, reject ANY manual fields
        if is_explicit_auto and has_any_manual:
            provided_fields = []
            if has_side:
                provided_fields.append("side")
            if has_entry:
                provided_fields.append("entry_price")
            if has_stop:
                provided_fields.append("stop_price")
            if has_capital:
                provided_fields.append("capital")

            return Response(
                {
                    "error": "Invalid payload: mode='auto' cannot have manual fields. Remove manual fields or remove mode='auto'.",
                    "fields_not_allowed": provided_fields,
                },
                status=status.HTTP_400_BAD_REQUEST,
            )

        # Auto mode: explicit OR all fields absent
        is_auto_mode = is_explicit_auto or not has_any_manual

        # Reject partial payloads (some manual fields but not all)
        if has_any_manual and not has_all_manual and not is_explicit_auto:
            missing = []
            if not has_side:
                missing.append("side")
            if not has_entry:
                missing.append("entry_price")
            if not has_stop:
                missing.append("stop_price")
            if not has_capital:
                missing.append("capital")

            return Response(
                {
                    "error": "Invalid payload: partial manual mode. Either provide ALL manual fields (side, entry_price, stop_price, capital) or use auto mode (mode='auto' or omit all fields).",
                    "missing_fields": missing,
                },
                status=status.HTTP_400_BAD_REQUEST,
            )

        if is_auto_mode:
            # AUTO MODE: Calculate all parameters automatically
            logger.info(f"Auto mode detected for client {client.id}")

            try:
                # Get symbol and strategy
                symbol = Symbol.objects.get(id=validated_data["symbol"], client=client)
                strategy = Strategy.objects.filter(
                    id=validated_data["strategy"], client=client
                ).first()
                if strategy is None:
                    strategy = Strategy.objects.get(
                        id=validated_data["strategy"], client__isnull=True
                    )

                # Use shared auto-calculation use case
                tech_stop_service = BinanceTechnicalStopService(client_id=client.id, timeout=5.0)

                # Create balance provider for BALANCE mode
                balance_provider = BinanceAccountBalanceAdapter(
                    use_testnet=None,  # Uses settings.BINANCE_USE_TESTNET
                    timeout=5.0,
                )

                from api.application.use_cases import AutoCalculateTradingParametersUseCase

                auto_calc_use_case = AutoCalculateTradingParametersUseCase(
                    tech_stop_service=tech_stop_service,
                    balance_provider=balance_provider,
                )

                try:
                    result = auto_calc_use_case.execute(
                        symbol_obj=symbol, strategy_obj=strategy, client_id=client.id
                    )

                    # Log warnings if any
                    if result.get("warnings"):
                        for warning in result["warnings"]:
                            logger.warning(f"Auto-calc warning for client {client.id}: {warning}")

                    logger.info(
                        f"Auto-calculated for client {client.id}: "
                        f"side={result['side']} (source: {result['side_source']}), "
                        f"capital={result['capital']} (source: {result['capital_source']}), "
                        f"entry={result['entry_price']}, stop={result['stop_price']}, "
                        f"method={result['method_used']}, confidence={result['confidence']}"
                    )

                except Exception as calc_error:
                    # Log and return error
                    # Note: Timeout fallback is handled inside BinanceTechnicalStopService
                    logger.error(f"Auto-calculation failed: {calc_error}", exc_info=True)
                    return Response(
                        {
                            "error": f"Auto-calculation failed: {str(calc_error)}. The system could not calculate trading parameters automatically."
                        },
                        status=status.HTTP_400_BAD_REQUEST,
                    )

                # Update validated_data with auto-calculated values
                validated_data["side"] = result["side"]
                validated_data["entry_price"] = result["entry_price"]
                validated_data["stop_price"] = result["stop_price"]
                validated_data["capital"] = result["capital"]
                validated_data["confidence"] = result[
                    "confidence_float"
                ]  # P0-4: Persist confidence as float
                validated_data["quantity"] = result[
                    "quantity"
                ]  # P0-3: Use exact quantized quantity
                validated_data["regime"] = "auto"
                validated_data["reason"] = (
                    f"Auto-calculated (side: {result['side_source']}, capital: {result['capital_source']})"
                )

            except Symbol.DoesNotExist:
                return Response(
                    {"error": f"Symbol with id {validated_data['symbol']} not found"},
                    status=status.HTTP_404_NOT_FOUND,
                )
            except Strategy.DoesNotExist:
                return Response(
                    {"error": f"Strategy with id {validated_data['strategy']} not found"},
                    status=status.HTTP_404_NOT_FOUND,
                )

        # Create command (works for both manual and auto modes now)
        command = CreateTradingIntentCommand(
            symbol_id=validated_data["symbol"],
            strategy_id=validated_data["strategy"],
            side=validated_data["side"],
            entry_price=validated_data["entry_price"],
            stop_price=validated_data["stop_price"],
            capital=validated_data["capital"],
            target_price=validated_data.get("target_price"),
            regime=validated_data.get("regime", "manual"),
            confidence=validated_data.get("confidence", 0.5),
            reason=validated_data.get("reason", "Manual entry via UI"),
            client_id=client.id,
            quantity=validated_data.get(
                "quantity"
            ),  # P0 Fix #3: Use exact quantized quantity in auto mode
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
            f"{intent.side} {intent.quantity} {intent.symbol.name} @ {intent.entry_price} "
            f"(mode: {'auto' if is_auto_mode else 'manual'})"
        )

        return Response(response_serializer.data, status=status.HTTP_201_CREATED)

    except ValueError as e:
        logger.warning(f"Validation error creating trading intent: {e}")
        return Response({"error": str(e)}, status=status.HTTP_400_BAD_REQUEST)
    except Exception as e:
        logger.error(f"Unexpected error creating trading intent: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"}, status=status.HTTP_500_INTERNAL_SERVER_ERROR
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
                {"error": "User has no associated client"}, status=status.HTTP_400_BAD_REQUEST
            )

        # Get intent with multi-tenant filtering
        repo = DjangoTradingIntentRepository()
        intent = repo.get_by_intent_id(intent_id, client.id)

        # Serialize response
        serializer = TradingIntentSerializer(intent)
        return Response(serializer.data, status=status.HTTP_200_OK)

    except ValueError as e:
        return Response({"error": str(e)}, status=status.HTTP_404_NOT_FOUND)
    except Exception as e:
        logger.error(f"Unexpected error fetching trading intent: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"}, status=status.HTTP_500_INTERNAL_SERVER_ERROR
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
                {"error": "User has no associated client"}, status=status.HTTP_400_BAD_REQUEST
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
                {"error": "limit and offset must be integers"}, status=status.HTTP_400_BAD_REQUEST
            )

        # Validate limit
        if limit < 1 or limit > 1000:
            return Response(
                {"error": "limit must be between 1 and 1000"}, status=status.HTTP_400_BAD_REQUEST
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
            status=status.HTTP_200_OK,
        )

    except Exception as e:
        logger.error(f"Unexpected error listing trading intents: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"}, status=status.HTTP_500_INTERNAL_SERVER_ERROR
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
                {"error": "User has no associated client"}, status=status.HTTP_400_BAD_REQUEST
            )

        # Get intent
        repo = DjangoTradingIntentRepository()
        intent = repo.get_by_intent_id(intent_id, client.id)

        # Validate using the validation framework
        validation_framework = ValidationFramework()
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
        return Response({"error": str(e)}, status=status.HTTP_404_NOT_FOUND)
    except Exception as e:
        logger.error(f"Unexpected error validating trading intent: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"}, status=status.HTTP_500_INTERNAL_SERVER_ERROR
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
                {"error": "User has no associated client"}, status=status.HTTP_400_BAD_REQUEST
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
                status=status.HTTP_400_BAD_REQUEST,
            )

        # Get intent
        repo = DjangoTradingIntentRepository()
        intent = repo.get_by_intent_id(intent_id, client.id)

        # Verify intent is validated
        if intent.status != "VALIDATED":
            return Response(
                {
                    "error": f"Intent must be VALIDATED before execution. Current status: {intent.status}"
                },
                status=status.HTTP_400_BAD_REQUEST,
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
        return Response({"error": str(e)}, status=status.HTTP_404_NOT_FOUND)
    except Exception as e:
        logger.error(f"Unexpected error executing trading intent: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"}, status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(["POST"])
@permission_classes([IsAuthenticated])
def pattern_trigger(request):
    """
    Pattern auto-trigger endpoint (Phase 5 MVP).

    Creates a TradingIntent when a pattern is detected, with idempotency protection.

    POST /api/pattern-triggers/

    Request body:
        pattern_code (str): Pattern code (e.g., "HAMMER", "MA_CROSSOVER")
        pattern_event_id (str): Unique event ID for idempotency
        symbol (int): Symbol ID
        side (str): "BUY" or "SELL"
        entry_price (Decimal): Entry price
        stop_price (Decimal): Stop-loss price
        capital (Decimal): Capital allocated
        strategy (int, optional): Strategy ID
        target_price (Decimal, optional): Take-profit price
        auto_validate (bool, default: true): Auto-validate the intent
        auto_execute (bool, default: false): Auto-execute (MVP: must be false)
        execution_mode (str, default: "dry-run"): Execution mode

    Returns:
        200 OK: Pattern trigger response
        400 Bad Request: Validation error or LIVE auto-execution attempt
        500 Internal Server Error: Unexpected error

    Idempotency:
        If the same pattern_event_id is sent twice, returns ALREADY_PROCESSED
        with the original intent_id.
    """
    try:
        # Get client from user
        client = request.user.client
        if not client:
            return Response(
                {"error": "User has no associated client"}, status=status.HTTP_400_BAD_REQUEST
            )

        # Validate input
        serializer = PatternTriggerSerializer(data=request.data)
        if not serializer.is_valid():
            return Response(serializer.errors, status=status.HTTP_400_BAD_REQUEST)

        data = serializer.validated_data

        # Check idempotency (has this pattern_event_id been processed?)
        pattern_event_id = data["pattern_event_id"]
        pattern_code = data["pattern_code"]

        if PatternTrigger.has_been_processed(client.id, pattern_event_id):
            # Already processed - return the original intent
            existing_trigger = PatternTrigger.objects.get(
                client_id=client.id, pattern_event_id=pattern_event_id
            )

            response_data = {
                "status": "ALREADY_PROCESSED",
                "intent_id": existing_trigger.intent.intent_id if existing_trigger.intent else None,
                "message": f"Pattern event {pattern_event_id} was already processed",
                "pattern_code": pattern_code,
            }

            logger.info(
                f"Pattern trigger {pattern_event_id} for client {client.id} was already processed"
            )

            return Response(response_data, status=status.HTTP_200_OK)

        # Create trading intent with pattern metadata
        # Use the CreateTradingIntentUseCase to create the intent
        command = CreateTradingIntentCommand(
            symbol_id=data["symbol"],
            strategy_id=data.get("strategy"),
            side=data["side"],
            entry_price=data["entry_price"],
            stop_price=data["stop_price"],
            capital=data["capital"],
            target_price=data.get("target_price"),
            regime="pattern",  # Indicate this came from pattern detection
            confidence=0.8,  # Default confidence for pattern triggers
            reason=f"Pattern trigger: {pattern_code}",
            client_id=client.id,
        )

        use_case = CreateTradingIntentUseCase(
            symbol_repo=DjangoSymbolRepository(),
            strategy_repo=DjangoStrategyRepository(),
            intent_repo=DjangoTradingIntentRepository(),
        )

        intent = use_case.execute(command)

        # Add pattern metadata to the intent
        intent.pattern_code = pattern_code
        intent.pattern_source = "pattern"
        intent.pattern_event_id = pattern_event_id
        intent.pattern_triggered_at = timezone.now()
        intent.save(
            update_fields=[
                "pattern_code",
                "pattern_source",
                "pattern_event_id",
                "pattern_triggered_at",
                "updated_at",
            ]
        )

        # Auto-validate if requested
        validation_result = None
        if data.get("auto_validate", True):
            validation_framework = ValidationFramework()
            report = validation_framework.validate(intent)

            intent.validation_result = report.to_dict()
            if not report.has_failures():
                intent.status = "VALIDATED"
                intent.validated_at = timezone.now()
            intent.save(update_fields=["status", "validated_at", "validation_result", "updated_at"])

            validation_result = report.to_dict()

        # MVP: Auto-execute is blocked (dry-run only for manual execution)
        # If auto_execute=true, we still only dry-run and mark as VALIDATED (not EXECUTED)
        # User must manually execute from the UI.

        # Record the pattern trigger (idempotency record)
        PatternTrigger.record_trigger(
            client_id=client.id,
            pattern_event_id=pattern_event_id,
            pattern_code=pattern_code,
            intent=intent,
        )

        # Prepare response
        response_data = {
            "status": "PROCESSED",
            "intent_id": intent.intent_id,
            "message": f"Pattern {pattern_code} triggered trading intent {intent.intent_id}",
            "pattern_code": pattern_code,
        }

        if validation_result:
            response_data["validation_result"] = validation_result

        logger.info(
            f"Pattern trigger {pattern_code} (event {pattern_event_id}) "
            f"for client {client.id}: created intent {intent.intent_id}"
        )

        return Response(response_data, status=status.HTTP_201_CREATED)

    except ValueError as e:
        logger.warning(f"Validation error in pattern trigger: {e}")
        return Response({"error": str(e)}, status=status.HTTP_400_BAD_REQUEST)
    except Exception as e:
        logger.error(f"Unexpected error in pattern trigger: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"}, status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(["POST"])
@permission_classes([IsAuthenticated])
def auto_calculate_parameters(request):
    """
    Auto-calculate trading parameters (compatibility wrapper).

    This endpoint calculates parameters WITHOUT creating a TradingIntent.
    Useful for preview/validation flows.

    POST /api/trading-intents/auto-calculate/

    Request body:
        symbol_id (int): Symbol ID
        strategy_id (int): Strategy ID

    Returns:
        200 OK: Calculated parameters
        {
            "symbol_id": int,
            "strategy_id": int,
            "side": "BUY" | "SELL",
            "entry_price": str,
            "stop_price": str,
            "capital": str,
            "quantity": str,
            "risk_amount": str,
            "position_value": str,
            "timeframe": str,
            "method_used": str,
            "confidence": str,
            "side_source": str,
            "capital_source": str
        }
        400 Bad Request: Invalid input or calculation failed
        408 Request Timeout: Binance API timeout
        500 Internal Server Error: Unexpected error
    """
    from decimal import Decimal
    from api.models import Symbol, Strategy
    from api.application.technical_stop_adapter import BinanceTechnicalStopService

    try:
        # Get client from user
        client = request.user.client
        if not client:
            return Response(
                {"error": "User has no associated client"}, status=status.HTTP_400_BAD_REQUEST
            )

        # Validate input
        symbol_id = request.data.get("symbol_id")
        strategy_id = request.data.get("strategy_id")

        if not symbol_id or not strategy_id:
            return Response(
                {"error": "symbol_id and strategy_id are required"},
                status=status.HTTP_400_BAD_REQUEST,
            )

        # Get symbol and strategy
        try:
            symbol = Symbol.objects.get(id=symbol_id, client=client)
            strategy = Strategy.objects.filter(id=strategy_id, client=client).first()
            if strategy is None:
                strategy = Strategy.objects.get(id=strategy_id, client__isnull=True)
        except Symbol.DoesNotExist:
            return Response(
                {"error": f"Symbol with id {symbol_id} not found"}, status=status.HTTP_404_NOT_FOUND
            )
        except Strategy.DoesNotExist:
            return Response(
                {"error": f"Strategy with id {strategy_id} not found"},
                status=status.HTTP_404_NOT_FOUND,
            )

        # Use shared auto-calculation use case
        tech_stop_service = BinanceTechnicalStopService(client_id=client.id, timeout=5.0)

        # Create balance provider for BALANCE mode
        balance_provider = BinanceAccountBalanceAdapter(
            use_testnet=None,  # Uses settings.BINANCE_USE_TESTNET
            timeout=5.0,
        )

        from api.application.use_cases import AutoCalculateTradingParametersUseCase

        auto_calc_use_case = AutoCalculateTradingParametersUseCase(
            tech_stop_service=tech_stop_service,
            balance_provider=balance_provider,
        )

        try:
            result = auto_calc_use_case.execute(
                symbol_obj=symbol, strategy_obj=strategy, client_id=client.id
            )
        except Exception as calc_error:
            # Log and return error
            # Note: Timeout fallback is handled inside BinanceTechnicalStopService
            logger.error(f"Auto-calculation failed: {calc_error}", exc_info=True)
            return Response(
                {"error": f"Auto-calculation failed: {str(calc_error)}"},
                status=status.HTTP_400_BAD_REQUEST,
            )

        # Format response
        # P0 Fix #1: Ensure confidence_float is ALWAYS numeric string
        confidence_float = result.get("confidence_float")
        if confidence_float is None:
            # Compute from confidence string using same mapping as use case
            from decimal import Decimal

            CONFIDENCE_MAP = {
                "HIGH": Decimal("0.8"),
                "MEDIUM": Decimal("0.6"),
                "MED": Decimal("0.6"),
                "LOW": Decimal("0.4"),
            }
            key = result.get("confidence", "LOW").upper()
            confidence_float = CONFIDENCE_MAP.get(key, Decimal("0.4"))

        response_data = {
            "symbol_id": symbol.id,
            "strategy_id": strategy.id,
            "side": result["side"],
            "entry_price": str(result["entry_price"]),
            "stop_price": str(result["stop_price"]),
            "capital": str(result["capital"]),
            "capital_used": str(result.get("capital_used", result["capital"])),
            "quantity": str(result["quantity"]),
            "risk_amount": str(result["risk_amount"]),
            "position_value": str(result["position_value"]),
            "timeframe": result["timeframe"],
            "method_used": result["method_used"],
            "confidence": result["confidence"],
            "confidence_float": str(confidence_float),  # P0 Fix #1: Always numeric
            "side_source": result["side_source"],
            "capital_source": result["capital_source"],
            "warnings": result.get("warnings", []),
        }

        logger.info(
            f"Auto-calculated parameters for client {client.id}: "
            f"{result['side']} {symbol.name} @ {result['entry_price']}, "
            f"stop @ {result['stop_price']}, qty {result['quantity']}"
        )

        return Response(response_data, status=status.HTTP_200_OK)

    except Exception as e:
        logger.error(f"Unexpected error in auto-calculate: {e}", exc_info=True)
        return Response(
            {"error": "Internal server error"}, status=status.HTTP_500_INTERNAL_SERVER_ERROR
        return Response(
            {"error": "Internal server error"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )

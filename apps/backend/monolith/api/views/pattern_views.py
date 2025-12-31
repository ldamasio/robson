"""
Pattern Detection Engine API Views.

Provides endpoints for:
- Listing pattern catalog and instances
- Managing pattern alerts
- Configuring strategy-pattern auto-entry
- Triggering manual pattern scans
"""

import logging
from datetime import timedelta

from django.utils import timezone
from django.conf import settings
from django.core.exceptions import ValidationError
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status

from api.models.patterns.base import (
    PatternCatalog,
    PatternInstance,
    PatternAlert,
    PatternStatus,
)
from api.models.patterns.strategy_config import StrategyPatternConfig
from api.models.trading import Strategy
from api.serializers.patterns import (
    PatternCatalogSerializer,
    PatternInstanceSerializer,
    PatternAlertSerializer,
    StrategyPatternConfigSerializer,
    CreateStrategyPatternConfigSerializer,
    UpdateStrategyPatternConfigSerializer,
    PatternScanRequestSerializer,
    PatternScanResultSerializer,
)
from api.application.pattern_engine import PatternScanCommand, PatternScanUseCase
from api.application.pattern_engine.adapters import (
    BinanceCandleProvider,
    DjangoPatternRepository,
)
from api.application.pattern_engine.detectors import (
    EngulfingDetector,
    HammerDetector,
    HeadAndShouldersDetector,
    InvertedHammerDetector,
    InvertedHeadAndShouldersDetector,
    MorningStarDetector,
)
from api.services.binance_service import BinanceService

logger = logging.getLogger(__name__)


# ==========================================
# PATTERN CATALOG ENDPOINTS
# ==========================================

@api_view(["GET"])
@permission_classes([IsAuthenticated])
def list_pattern_catalog(request):
    """
    List all available patterns in the catalog.

    Returns pattern metadata including:
    - Pattern code and name
    - Category (candlestick, chart, harmonic, etc.)
    - Direction bias (bullish, bearish, neutral)
    - Confirmation and invalidation methods

    GET /api/patterns/catalog/
    """
    try:
        catalog = PatternCatalog.objects.all().order_by("category", "pattern_code")
        serializer = PatternCatalogSerializer(catalog, many=True)
        return Response(
            {"count": catalog.count(), "results": serializer.data},
            status=status.HTTP_200_OK,
        )
    except Exception as e:
        logger.exception("Failed to list pattern catalog")
        return Response(
            {"error": "Failed to list pattern catalog", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def get_pattern_detail(request, pattern_code: str):
    """
    Get detailed information about a specific pattern.

    GET /api/patterns/catalog/{pattern_code}/
    """
    try:
        pattern = PatternCatalog.objects.get(pattern_code=pattern_code.upper())
        serializer = PatternCatalogSerializer(pattern)
        return Response(serializer.data, status=status.HTTP_200_OK)
    except PatternCatalog.DoesNotExist:
        return Response(
            {"error": f"Pattern '{pattern_code}' not found"},
            status=status.HTTP_404_NOT_FOUND,
        )


# ==========================================
# PATTERN INSTANCE ENDPOINTS
# ==========================================

@api_view(["GET"])
@permission_classes([IsAuthenticated])
def list_pattern_instances(request):
    """
    List detected pattern instances with optional filters.

    Query Parameters:
    - symbol: Filter by symbol (e.g., BTCUSDT)
    - timeframe: Filter by timeframe (e.g., 15m, 1h, 4h)
    - status: Filter by status (FORMING, CONFIRMED, INVALIDATED, etc.)
    - pattern_code: Filter by pattern code
    - limit: Maximum number of results (default: 50)

    GET /api/patterns/instances/?symbol=BTCUSDT&status=CONFIRMED
    """
    try:
        qs = PatternInstance.objects.select_related("pattern", "symbol").order_by(
            "-created_at"
        )

        # Apply filters
        symbol = request.query_params.get("symbol")
        if symbol:
            qs = qs.filter(symbol__name__icontains=symbol.upper())

        timeframe = request.query_params.get("timeframe")
        if timeframe:
            qs = qs.filter(timeframe=timeframe)

        status_filter = request.query_params.get("status")
        if status_filter:
            qs = qs.filter(status=status_filter.upper())

        pattern_code = request.query_params.get("pattern_code")
        if pattern_code:
            qs = qs.filter(pattern__pattern_code=pattern_code.upper())

        # Limit results
        limit = int(request.query_params.get("limit", 50))
        qs = qs[:limit]

        serializer = PatternInstanceSerializer(qs, many=True)
        return Response(
            {"count": len(serializer.data), "results": serializer.data},
            status=status.HTTP_200_OK,
        )
    except Exception as e:
        logger.exception("Failed to list pattern instances")
        return Response(
            {"error": "Failed to list pattern instances", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def get_pattern_instance(request, instance_id: int):
    """
    Get detailed information about a specific pattern instance.

    Includes pattern points and related alerts.

    GET /api/patterns/instances/{id}/
    """
    try:
        instance = PatternInstance.objects.select_related("pattern", "symbol").get(
            id=instance_id
        )
        serializer = PatternInstanceSerializer(instance)
        return Response(serializer.data, status=status.HTTP_200_OK)
    except PatternInstance.DoesNotExist:
        return Response(
            {"error": f"Pattern instance {instance_id} not found"},
            status=status.HTTP_404_NOT_FOUND,
        )


# ==========================================
# PATTERN ALERT ENDPOINTS
# ==========================================

@api_view(["GET"])
@permission_classes([IsAuthenticated])
def list_pattern_alerts(request):
    """
    List pattern alerts with optional filters.

    Query Parameters:
    - symbol: Filter by symbol
    - timeframe: Filter by timeframe
    - alert_type: Filter by alert type (DETECT, CONFIRM, INVALIDATE, etc.)
    - hours: Only show alerts from last N hours (default: 24)
    - limit: Maximum number of results (default: 50)

    GET /api/patterns/alerts/?alert_type=CONFIRM&hours=6
    """
    try:
        qs = PatternAlert.objects.select_related(
            "instance__pattern", "instance__symbol"
        ).order_by("-alert_ts")

        # Apply filters
        symbol = request.query_params.get("symbol")
        if symbol:
            qs = qs.filter(instance__symbol__name__icontains=symbol.upper())

        timeframe = request.query_params.get("timeframe")
        if timeframe:
            qs = qs.filter(instance__timeframe=timeframe)

        alert_type = request.query_params.get("alert_type")
        if alert_type:
            qs = qs.filter(alert_type=alert_type.upper())

        # Time filter
        hours = int(request.query_params.get("hours", 24))
        cutoff = timezone.now() - timedelta(hours=hours)
        qs = qs.filter(alert_ts__gte=cutoff)

        # Limit results
        limit = int(request.query_params.get("limit", 50))
        qs = qs[:limit]

        serializer = PatternAlertSerializer(qs, many=True)
        return Response(
            {"count": len(serializer.data), "results": serializer.data},
            status=status.HTTP_200_OK,
        )
    except Exception as e:
        logger.exception("Failed to list pattern alerts")
        return Response(
            {"error": "Failed to list pattern alerts", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def get_recent_confirms(request):
    """
    Get recent CONFIRMED pattern alerts for trading opportunities.

    Query Parameters:
    - hours: Time window in hours (default: 6)
    - symbol: Filter by symbol (optional)
    - pattern_code: Filter by pattern code (optional)

    GET /api/patterns/alerts/recent-confirms/
    """
    try:
        hours = int(request.query_params.get("hours", 6))
        cutoff = timezone.now() - timedelta(hours=hours)

        qs = PatternAlert.objects.filter(
            alert_type=PatternAlert.AlertType.CONFIRM,
            alert_ts__gte=cutoff,
        ).select_related("instance__pattern", "instance__symbol")

        symbol = request.query_params.get("symbol")
        if symbol:
            qs = qs.filter(instance__symbol__name__icontains=symbol.upper())

        pattern_code = request.query_params.get("pattern_code")
        if pattern_code:
            qs = qs.filter(instance__pattern__pattern_code=pattern_code.upper())

        qs = qs.order_by("-alert_ts")

        serializer = PatternAlertSerializer(qs, many=True)
        return Response(
            {"count": qs.count(), "results": serializer.data},
            status=status.HTTP_200_OK,
        )
    except Exception as e:
        logger.exception("Failed to get recent confirms")
        return Response(
            {"error": "Failed to get recent confirms", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


# ==========================================
# STRATEGY PATTERN CONFIG ENDPOINTS
# ==========================================

@api_view(["GET"])
@permission_classes([IsAuthenticated])
def list_strategy_configs(request):
    """
    List all strategy-pattern configurations for the user's client.

    GET /api/patterns/configs/
    """
    try:
        client_id = request.user.client.id if hasattr(request.user, "client") else None
        if not client_id:
            return Response(
                {"error": "Client not found"}, status=status.HTTP_400_BAD_REQUEST
            )

        configs = StrategyPatternConfig.objects.filter(
            strategy__client_id=client_id
        ).select_related("strategy", "pattern")

        serializer = StrategyPatternConfigSerializer(configs, many=True)
        return Response(
            {"count": configs.count(), "results": serializer.data},
            status=status.HTTP_200_OK,
        )
    except Exception as e:
        logger.exception("Failed to list strategy configs")
        return Response(
            {"error": "Failed to list strategy configs", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["POST"])
@permission_classes([IsAuthenticated])
def create_strategy_config(request):
    """
    Create a new strategy-pattern configuration.

    Request body:
    {
        "strategy": 1,
        "pattern": "HAMMER",
        "auto_entry_enabled": false,
        "entry_mode": "SUGGEST",
        "min_confidence": 0.75,
        "timeframes": ["15m", "1h"],
        "symbols": ["BTCUSDT", "ETHUSDT"]
    }

    POST /api/patterns/configs/
    """
    try:
        serializer = CreateStrategyPatternConfigSerializer(data=request.data)
        serializer.is_valid(raise_exception=True)

        # Validate strategy belongs to user's client
        strategy = serializer.validated_data["strategy"]
        client_id = request.user.client.id if hasattr(request.user, "client") else None

        if strategy.client_id != client_id:
            return Response(
                {"error": "Strategy does not belong to your client"},
                status=status.HTTP_403_FORBIDDEN,
            )

        config = serializer.save()

        response_serializer = StrategyPatternConfigSerializer(config)
        return Response(
            response_serializer.data, status=status.HTTP_201_CREATED
        )
    except ValidationError as e:
        return Response(
            {"error": "Validation failed", "detail": str(e)},
            status=status.HTTP_400_BAD_REQUEST,
        )
    except Exception as e:
        logger.exception("Failed to create strategy config")
        return Response(
            {"error": "Failed to create strategy config", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["GET", "PUT", "DELETE"])
@permission_classes([IsAuthenticated])
def manage_strategy_config(request, config_id: int):
    """
    Get, update, or delete a specific strategy-pattern configuration.

    GET /api/patterns/configs/{id}/
    PUT /api/patterns/configs/{id}/
    DELETE /api/patterns/configs/{id}/
    """
    try:
        config = StrategyPatternConfig.objects.select_related("strategy").get(id=config_id)

        # Verify ownership
        client_id = request.user.client.id if hasattr(request.user, "client") else None
        if config.strategy.client_id != client_id:
            return Response(
                {"error": "Config does not belong to your client"},
                status=status.HTTP_403_FORBIDDEN,
            )

        if request.method == "GET":
            serializer = StrategyPatternConfigSerializer(config)
            return Response(serializer.data, status=status.HTTP_200_OK)

        elif request.method == "PUT":
            serializer = UpdateStrategyPatternConfigSerializer(
                config, data=request.data, partial=True
            )
            serializer.is_valid(raise_exception=True)
            serializer.save()

            response_serializer = StrategyPatternConfigSerializer(config)
            return Response(response_serializer.data, status=status.HTTP_200_OK)

        elif request.method == "DELETE":
            config.delete()
            return Response(
                {"message": "Config deleted successfully"},
                status=status.HTTP_204_NO_CONTENT,
            )

    except StrategyPatternConfig.DoesNotExist:
        return Response(
            {"error": f"Config {config_id} not found"},
            status=status.HTTP_404_NOT_FOUND,
        )
    except Exception as e:
        logger.exception(f"Failed to manage strategy config {config_id}")
        return Response(
            {"error": "Failed to manage config", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def get_active_configs_for_pattern(request):
    """
    Get active strategy configs for a specific pattern.

    Query Parameters:
    - pattern_code: Pattern code (required)
    - symbol: Optional symbol filter
    - timeframe: Optional timeframe filter

    GET /api/patterns/configs/active-for-pattern/?pattern_code=HAMMER&symbol=BTCUSDT
    """
    try:
        pattern_code = request.query_params.get("pattern_code")
        if not pattern_code:
            return Response(
                {"error": "pattern_code parameter is required"},
                status=status.HTTP_400_BAD_REQUEST,
            )

        symbol = request.query_params.get("symbol")
        timeframe = request.query_params.get("timeframe")

        configs = StrategyPatternConfig.get_active_configs_for_pattern(
            pattern_code=pattern_code.upper(),
            symbol=symbol,
            timeframe=timeframe,
        )

        serializer = StrategyPatternConfigSerializer(configs, many=True)
        return Response(
            {"count": configs.count(), "results": serializer.data},
            status=status.HTTP_200_OK,
        )
    except Exception as e:
        logger.exception("Failed to get active configs for pattern")
        return Response(
            {"error": "Failed to get active configs", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


# ==========================================
# PATTERN SCAN ENDPOINTS
# ==========================================

@api_view(["POST"])
@permission_classes([IsAuthenticated])
def trigger_pattern_scan(request):
    """
    Trigger a manual pattern scan on specified symbols/timeframes.

    Request body:
    {
        "symbols": "BTCUSDT,ETHUSDT",
        "timeframes": "15m,1h",
        "all_detectors": true
    }

    POST /api/patterns/scan/
    """
    try:
        serializer = PatternScanRequestSerializer(data=request.data)
        serializer.is_valid(raise_exception=True)

        # Initialize adapters
        # BinanceService is a singleton that uses settings for credentials
        use_testnet = getattr(settings, "BINANCE_USE_TESTNET", False)
        binance_service = BinanceService(use_testnet=use_testnet)
        candle_provider = BinanceCandleProvider(binance_service)
        # For API-triggered scans, use the user's client for multi-tenancy
        client_id = request.user.client.id if hasattr(request.user, "client") else None
        # Note: DjangoPatternRepository now accepts client object, not client_id
        # For system-wide scans from API, pass None (system-owned patterns)
        # In production, you may want to pass the actual client object
        pattern_repository = DjangoPatternRepository(client=None)

        # Select detectors
        if serializer.validated_data["all_detectors"]:
            detectors = [
                HammerDetector(),
                InvertedHammerDetector(),
                EngulfingDetector(),
                MorningStarDetector(),
                HeadAndShouldersDetector(),
                InvertedHeadAndShouldersDetector(),
            ]
        elif serializer.validated_data["candlestick"]:
            detectors = [
                HammerDetector(),
                InvertedHammerDetector(),
                EngulfingDetector(),
                MorningStarDetector(),
            ]
        elif serializer.validated_data["chart"]:
            detectors = [
                HeadAndShouldersDetector(),
                InvertedHeadAndShouldersDetector(),
            ]
        else:
            detectors = []

        if not detectors:
            return Response(
                {"error": "No detectors selected"},
                status=status.HTTP_400_BAD_REQUEST,
            )

        # Initialize use case
        use_case = PatternScanUseCase(
            candle_provider=candle_provider,
            pattern_repository=pattern_repository,
        )

        # Parse symbols and timeframes
        symbols = [
            s.strip().upper()
            for s in serializer.validated_data["symbols"].split(",")
        ]
        timeframes = [t.strip() for t in serializer.validated_data["timeframes"].split(",")]

        # Scan each combination
        results = []
        total_patterns = 0
        total_confirmations = 0
        total_invalidations = 0

        for symbol in symbols:
            for timeframe in timeframes:
                try:
                    command = PatternScanCommand(
                        symbol=symbol,
                        timeframe=timeframe,
                        detectors=detectors,
                        candle_limit=100,
                    )

                    result = use_case.execute(command)
                    total_patterns += result.patterns_detected
                    total_confirmations += result.confirmations_found
                    total_invalidations += result.invalidations_found

                    results.append(
                        {
                            "symbol": symbol,
                            "timeframe": timeframe,
                            "patterns_detected": result.patterns_detected,
                            "confirmations_found": result.confirmations_found,
                            "invalidations_found": result.invalidations_found,
                        }
                    )
                except Exception as e:
                    logger.exception(f"Scan failed for {symbol} {timeframe}")
                    results.append(
                        {
                            "symbol": symbol,
                            "timeframe": timeframe,
                            "error": str(e),
                        }
                    )

        return Response(
            {
                "summary": {
                    "total_patterns": total_patterns,
                    "total_confirmations": total_confirmations,
                    "total_invalidations": total_invalidations,
                },
                "results": results,
            },
            status=status.HTTP_200_OK,
        )

    except Exception as e:
        logger.exception("Failed to trigger pattern scan")
        return Response(
            {"error": "Failed to trigger pattern scan", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


# ==========================================
# PATTERN TO PLAN ENDPOINTS
# ==========================================

@api_view(["POST"])
@permission_classes([IsAuthenticated])
def process_pattern_to_plan(request):
    """
    Process a CONFIRMED pattern into a trading plan.

    Request body:
    {
        "pattern_instance_id": 123,
        "force_create": false
    }

    POST /api/patterns/to-plan/
    """
    try:
        from api.application.pattern_engine.pattern_to_plan import (
            PatternToPlanCommand,
            PatternToPlanUseCase,
        )

        pattern_instance_id = request.data.get("pattern_instance_id")
        if not pattern_instance_id:
            return Response(
                {"error": "pattern_instance_id is required"},
                status=status.HTTP_400_BAD_REQUEST,
            )

        use_case = PatternToPlanUseCase()
        command = PatternToPlanCommand(
            pattern_instance_id=pattern_instance_id,
            force_create=request.data.get("force_create", False),
        )

        result = use_case.execute(command)

        return Response(
            {
                "success": result.success,
                "plans_created": result.plans_created,
                "strategies_matched": result.strategies_matched,
                "errors": result.errors,
                "details": result.details,
            },
            status=status.HTTP_200_OK if result.success else status.HTTP_400_BAD_REQUEST,
        )

    except Exception as e:
        logger.exception("Failed to process pattern to plan")
        return Response(
            {"error": "Failed to process pattern to plan", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )


@api_view(["GET"])
@permission_classes([IsAuthenticated])
def pattern_dashboard(request):
    """
    Get pattern detection dashboard summary.

    Returns statistics about recent patterns, alerts, and active configurations.

    GET /api/patterns/dashboard/
    """
    try:
        # Recent patterns (last 24 hours)
        cutoff = timezone.now() - timedelta(hours=24)

        recent_patterns = PatternInstance.objects.filter(detected_at__gte=cutoff)
        recent_alerts = PatternAlert.objects.filter(alert_ts__gte=cutoff)

        # Status breakdown
        status_counts = {}
        for status_choice in PatternStatus.choices:
            status_counts[status_choice[0]] = recent_patterns.filter(
                status=status_choice[0]
            ).count()

        # Alert type breakdown
        alert_counts = {}
        for alert_type in PatternAlert.AlertType.choices:
            alert_counts[alert_type[0]] = recent_alerts.filter(
                alert_type=alert_type[0]
            ).count()

        # Active configs for user's client
        client_id = request.user.client.id if hasattr(request.user, "client") else None
        active_configs = 0
        if client_id:
            active_configs = StrategyPatternConfig.objects.filter(
                strategy__client_id=client_id,
                auto_entry_enabled=True,
            ).count()

        return Response(
            {
                "period": "Last 24 hours",
                "patterns": {
                    "total_detected": recent_patterns.count(),
                    "by_status": status_counts,
                },
                "alerts": {
                    "total": recent_alerts.count(),
                    "by_type": alert_counts,
                },
                "configs": {
                    "active_auto_entry": active_configs,
                },
            },
            status=status.HTTP_200_OK,
        )

    except Exception as e:
        logger.exception("Failed to get pattern dashboard")
        return Response(
            {"error": "Failed to get dashboard", "detail": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR,
        )

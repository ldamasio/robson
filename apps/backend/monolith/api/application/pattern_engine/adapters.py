"""
Pattern Engine Adapters.

Concrete implementations of ports using Django ORM and Binance API.
This is the ONLY file that imports Django.

Adapted to work with existing model structure:
- PatternInstance.pattern is ForeignKey to PatternCatalog
- PatternInstance.symbol is ForeignKey to Symbol
- PatternInstance.features stores evidence and confidence
- PatternPoint.label stores pivot type (not point_type)
"""

from __future__ import annotations

from datetime import datetime
from decimal import Decimal
import logging

from django.db import transaction
from django.core.exceptions import ValidationError

from api.models.patterns.base import (
    PatternAlert,
    PatternCatalog,
    PatternInstance,
    PatternPoint,
)
from api.models.patterns.candlestick import CandlestickPatternDetail
from api.models.patterns.chart import ChartPatternDetail
from api.models import Symbol
from api.services.binance_service import BinanceService

from .domain import OHLCV, CandleWindow, PatternSignature, PivotPoint
from .ports import CandleProviderError

logger = logging.getLogger(__name__)


class BinanceCandleProvider:
    """
    Fetch candles from Binance via BinanceService.

    All timestamps come from exchange data (kline[0]).
    """

    def __init__(self, binance_service: BinanceService):
        """
        Initialize provider.

        Args:
            binance_service: BinanceService instance for API access
        """
        self._binance_service = binance_service

    def get_candles(self, symbol: str, timeframe: str, limit: int) -> CandleWindow:
        """
        Fetch recent candles from Binance.

        Args:
            symbol: Trading pair (e.g., "BTCUSDT")
            timeframe: Interval (e.g., "15m", "1h", "1d")
            limit: Number of candles to fetch

        Returns:
            CandleWindow with candles ordered oldest-first

        Raises:
            CandleProviderError: If fetch fails or data invalid
        """
        try:
            # Fetch klines from Binance
            client = self._binance_service.client
            klines = client.get_klines(symbol=symbol, interval=timeframe, limit=limit)

            if not klines:
                msg = f"No candles returned for {symbol} {timeframe}"
                raise CandleProviderError(msg)

            # Convert to OHLCV entities
            candles = []
            for kline in klines:
                ts_ms = int(kline[0])  # Timestamp in milliseconds
                ts = datetime.fromtimestamp(ts_ms / 1000.0)  # Convert to datetime

                candle = OHLCV(
                    ts=ts,
                    open=Decimal(str(kline[1])),
                    high=Decimal(str(kline[2])),
                    low=Decimal(str(kline[3])),
                    close=Decimal(str(kline[4])),
                    volume=Decimal(str(kline[5])),
                )
                candles.append(candle)

            # Create CandleWindow (validates chronological order)
            window = CandleWindow(
                symbol=symbol,
                timeframe=timeframe,
                candles=tuple(candles),  # Immutable
                start_ts=candles[0].ts,
                end_ts=candles[-1].ts,
            )

            return window

        except Exception as e:
            msg = f"Failed to fetch candles for {symbol} {timeframe}: {e}"
            raise CandleProviderError(msg) from e


def _get_or_create_pattern_catalog(pattern_code: str) -> PatternCatalog:
    """
    Get or create PatternCatalog entry for pattern code.

    Maps detector pattern codes to catalog entries.

    Args:
        pattern_code: Pattern code from detector

    Returns:
        PatternCatalog instance
    """
    # Mapping of detector codes to catalog entries
    pattern_names = {
        "HAMMER": ("Hammer", "CANDLESTICK", "BULLISH"),
        "INVERTED_HAMMER": ("Inverted Hammer", "CANDLESTICK", "BULLISH"),
        "BULLISH_ENGULFING": ("Bullish Engulfing", "CANDLESTICK", "BULLISH"),
        "BEARISH_ENGULFING": ("Bearish Engulfing", "CANDLESTICK", "BEARISH"),
        "MORNING_STAR": ("Morning Star", "CANDLESTICK", "BULLISH"),
        "HEAD_AND_SHOULDERS": ("Head and Shoulders", "CHART", "BEARISH"),
        "INVERTED_HEAD_AND_SHOULDERS": ("Inverted Head and Shoulders", "CHART", "BULLISH"),
    }

    if pattern_code not in pattern_names:
        # Create generic entry for unknown patterns
        return PatternCatalog.objects.get_or_create(
            pattern_code=pattern_code,
            defaults={
                "name": pattern_code.replace("_", " ").title(),
                "category": "HYBRID",
                "direction_bias": "NEUTRAL",
                "min_bars": 1,
            },
        )[0]

    name, category, bias = pattern_names[pattern_code]

    return PatternCatalog.objects.get_or_create(
        pattern_code=pattern_code,
        defaults={
            "name": name,
            "category": category,
            "direction_bias": bias,
            "min_bars": 1,
        },
    )[0]


def _get_or_create_symbol(symbol_name: str, client) -> Symbol:
    """
    Get or create Symbol entry for trading pair.

    Multi-tenant aware: uses client from context.

    Args:
        symbol_name: Trading pair name (e.g., "BTCUSDT")
        client: Client instance for multi-tenancy (can be None for system-wide patterns)

    Returns:
        Symbol instance

    Note:
        Symbol parsing uses a reliable list of known quote assets.
        For unknown symbols, attempts to infer from Binance naming convention.
        Works for: BTCUSDT, ETHUSDT, 1000PEPEUSDT, BTCFDUSD, etc.
    """
    # Known quote assets in order of specificity (longest first)
    KNOWN_QUOTES = [
        "FDUSD", "USDT", "BUSD", "USDC",  # Stablecoins
        "BTC", "ETH", "BNB",  # Major assets as quotes
        "USDD", "DAI", "TUSD",  # Other stablecoins
    ]

    # Try to find known quote asset in symbol name
    base = symbol_name
    quote = None

    for known_quote in sorted(KNOWN_QUOTES, key=len, reverse=True):
        if symbol_name.endswith(known_quote) and len(symbol_name) > len(known_quote):
            base = symbol_name[: -len(known_quote)]
            quote = known_quote
            break

    # Fallback: if no known quote found, use last 4 chars (may fail for 3-char quotes)
    if not quote:
        logger.warning(f"Unknown quote asset in {symbol_name}, assuming last 4 chars")
        if len(symbol_name) > 4:
            base = symbol_name[:-4]
            quote = symbol_name[-4:]
        else:
            raise ValueError(f"Cannot parse symbol: {symbol_name}")

    # Build defaults for Symbol creation
    defaults = {
        "base_asset": base,
        "quote_asset": quote,
        "is_active": True,
    }

    # Try to find existing symbol (with or without client filter)
    if client:
        symbol, created = Symbol.objects.get_or_create(
            name=symbol_name,
            client=client,
            defaults=defaults,
        )
    else:
        # For system-wide patterns (CronJob), try to find any existing symbol
        # or create without a specific client (client will be set to None/null)
        try:
            symbol = Symbol.objects.filter(name=symbol_name).first()
            if symbol:
                return symbol
            # Create with client=None (system-wide)
            symbol = Symbol.objects.create(name=symbol_name, client=None, **defaults)
            created = True
        except Exception as e:
            logger.error(f"Failed to get_or_create symbol {symbol_name}: {e}")
            # Last resort: try with first available client
            from clients.models import Client
            first_client = Client.objects.first()
            if first_client:
                symbol, created = Symbol.objects.get_or_create(
                    name=symbol_name,
                    client=first_client,
                    defaults=defaults,
                )
            else:
                raise RuntimeError("No client available for symbol creation")

    return symbol


class DjangoPatternRepository:
    """
    Persist patterns using Django ORM.

    Implements idempotent operations with get_or_create protected by UniqueConstraint.
    All timestamps come from candle data (passed in from domain layer).

    Adapted for existing model structure with PatternCatalog and Symbol ForeignKeys.

    Multi-tenant aware: accepts client object for operations.
    """

    def __init__(self, client=None):
        """
        Initialize repository.

        Args:
            client: Client instance for multi-tenancy (None for system-wide patterns)
        """
        self._client = client

    def get_or_create_instance(self, signature: PatternSignature) -> tuple[PatternInstance, bool]:
        """
        Idempotent instance creation protected by UniqueConstraint.

        Uniqueness: (client, pattern, symbol, timeframe, start_ts)
        Uses get_or_create for atomicity - concurrent scans will not create duplicates.

        Args:
            signature: Pattern signature from detector

        Returns:
            (PatternInstance, created) tuple

        Raises:
            ValidationError: If uniqueness constraint is violated (should not happen with get_or_create)
        """
        # Get or create PatternCatalog entry
        pattern_catalog = _get_or_create_pattern_catalog(signature.pattern_code)

        # Get or create Symbol (uses client from repository)
        symbol = _get_or_create_symbol(signature.symbol, self._client)

        # Build features dict with evidence and confidence
        features = {
            "evidence": signature.evidence,
            "confidence": float(signature.confidence),
        }

        # Use get_or_create for atomic idempotent creation
        # Protected by UniqueConstraint on (client, pattern, symbol, timeframe, start_ts)
        instance, created = PatternInstance.objects.get_or_create(
            pattern=pattern_catalog,
            symbol=symbol,
            timeframe=signature.timeframe,
            start_ts=signature.start_ts,
            defaults={
                "client": self._client,
                "end_ts": signature.end_ts,
                "status": "FORMING",
                "features": features,
            },
        )

        # If instance already existed, update features with new evidence (merge)
        if not created and instance.features:
            instance.features["evidence"].update(features.get("evidence", {}))
            instance.save(update_fields=["features"])

        return instance, created

    def update_status(
        self,
        instance_id: int,
        status: str,
        event_ts: datetime,  # From candle timestamp
        evidence: dict,
    ) -> None:
        """
        Update pattern instance status.

        Args:
            instance_id: Pattern instance ID
            status: New status (CONFIRMED, INVALIDATED, etc.)
            event_ts: Event timestamp FROM CANDLE DATA
            evidence: Evidence payload
        """
        instance = PatternInstance.objects.get(id=instance_id)

        # Update features with new evidence
        features = instance.features or {}
        features["evidence"] = evidence
        features["last_event_ts"] = event_ts.isoformat()

        # Set breakout_ts if confirming
        update_fields = {"features": features, "status": status}
        if status == "CONFIRMED" and not instance.breakout_ts:
            update_fields["breakout_ts"] = event_ts

        PatternInstance.objects.filter(id=instance_id).update(**update_fields)

    @transaction.atomic
    def emit_alert(
        self,
        instance_id: int,
        alert_type: str,
        alert_ts: datetime,  # From candle timestamp
        confidence: Decimal,
        payload: dict,
    ) -> tuple[PatternAlert, bool]:
        """
        Emit pattern alert (idempotent).

        Uniqueness key: (instance_id, alert_type, alert_ts)

        CRITICAL: alert_ts MUST come from candle data, NOT datetime.now()

        Args:
            instance_id: Pattern instance ID
            alert_type: Alert type (FORMING, CONFIRM, INVALIDATE, etc.)
            alert_ts: Alert timestamp FROM CANDLE DATA
            confidence: Confidence score [0-1]
            payload: Evidence and thresholds

        Returns:
            (PatternAlert, created) tuple for idempotency tracking
        """
        alert, created = PatternAlert.objects.get_or_create(
            instance_id=instance_id,
            alert_type=alert_type,
            alert_ts=alert_ts,  # From candle, NOT datetime.now()
            defaults={
                "confidence": confidence,
                "payload": payload,
            },
        )
        return alert, created

    def store_candlestick_detail(self, instance_id: int, metrics: dict) -> None:
        """
        Create CandlestickPatternDetail record.

        Args:
            instance_id: Pattern instance ID
            metrics: Candle metrics (body_pct, wick_pcts, etc.)
        """
        CandlestickPatternDetail.objects.create(
            instance_id=instance_id,
            body_pct_main=metrics.get("body_pct"),
            upper_wick_pct_main=metrics.get("upper_wick_pct"),
            lower_wick_pct_main=metrics.get("lower_wick_pct"),
            engulf_ratio=metrics.get("engulf_ratio"),
        )

    def store_chart_detail(self, instance_id: int, metrics: dict) -> None:
        """
        Create ChartPatternDetail record.

        Args:
            instance_id: Pattern instance ID
            metrics: Chart pattern metrics (neckline_slope, etc.)
        """
        ChartPatternDetail.objects.create(
            instance_id=instance_id,
            neckline_slope=metrics.get("neckline_slope"),
            head_prominence_pct=metrics.get("head_prominence_pct"),
            shoulder_symmetry=metrics.get("shoulder_symmetry"),
        )

    def store_pattern_points(self, instance_id: int, points: list[PivotPoint]) -> None:
        """
        Create PatternPoint records (for chart patterns).

        Note: Model uses 'label' not 'point_type', and 'bar_index_offset' not 'bar_index'.

        Args:
            instance_id: Pattern instance ID
            points: List of pivot points (LS, HEAD, RS, neckline, etc.)
        """
        for point in points:
            PatternPoint.objects.create(
                instance_id=instance_id,
                label=point.pivot_type,  # Model uses 'label'
                ts=point.ts,
                price=point.price,
                bar_index_offset=point.bar_index,  # Model uses 'bar_index_offset'
            )

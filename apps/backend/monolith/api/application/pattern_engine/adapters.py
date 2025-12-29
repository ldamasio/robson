"""
Pattern Engine Adapters.

Concrete implementations of ports using Django ORM and Binance API.
This is the ONLY file that imports Django.
"""

from __future__ import annotations

from datetime import datetime
from decimal import Decimal
from typing import TYPE_CHECKING

from django.db import transaction

from api.models.patterns.base import PatternAlert, PatternInstance, PatternPoint
from api.models.patterns.candlestick import CandlestickPatternDetail
from api.models.patterns.chart import ChartPatternDetail
from api.services.market_data_service import MarketDataService

from .domain import OHLCV, CandleWindow, PatternSignature, PivotPoint
from .ports import CandleProviderError

if TYPE_CHECKING:
    from api.models import BinanceClient


class BinanceCandleProvider:
    """
    Fetch candles from Binance via MarketDataService.

    Uses existing MarketDataService.get_historical_data() with caching.
    All timestamps come from exchange data (kline[0]).
    """

    def __init__(self, client: BinanceClient):
        """
        Initialize provider.

        Args:
            client: BinanceClient instance for API access
        """
        self._client = client
        self._market_data_service = MarketDataService(client)

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
            # Returns: [[timestamp, open, high, low, close, volume, ...], ...]
            klines = self._market_data_service.get_historical_data(
                symbol=symbol,
                interval=timeframe,
                limit=limit,
            )

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


class DjangoPatternRepository:
    """
    Persist patterns using Django ORM.

    Implements idempotent operations with get_or_create.
    All timestamps come from candle data (passed in from domain layer).
    """

    def get_or_create_instance(self, signature: PatternSignature) -> tuple[PatternInstance, bool]:
        """
        Idempotent instance creation.

        Uniqueness key: (pattern_code, symbol, timeframe, start_ts)

        Args:
            signature: Pattern signature from detector

        Returns:
            (PatternInstance, created) tuple
        """
        instance, created = PatternInstance.objects.get_or_create(
            pattern_code=signature.pattern_code,
            symbol=signature.symbol,
            timeframe=signature.timeframe,
            start_ts=signature.start_ts,
            defaults={
                "end_ts": signature.end_ts,
                "status": "FORMING",
                "confidence": signature.confidence,
                "evidence": signature.evidence,
            },
        )

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
        PatternInstance.objects.filter(id=instance_id).update(
            status=status,
            last_checked_at=event_ts,  # From candle, NOT datetime.now()
            evidence=evidence,
        )

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
            body_pct_main=metrics.get("body_pct_main"),
            upper_wick_pct_main=metrics.get("upper_wick_pct_main"),
            lower_wick_pct_main=metrics.get("lower_wick_pct_main"),
            body_pct_second=metrics.get("body_pct_second"),
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
            target_price=metrics.get("target_price"),
            breakout_price=metrics.get("breakout_price"),
        )

    def store_pattern_points(self, instance_id: int, points: list[PivotPoint]) -> None:
        """
        Create PatternPoint records (for chart patterns).

        Args:
            instance_id: Pattern instance ID
            points: List of pivot points (LS, HEAD, RS, neckline, etc.)
        """
        for point in points:
            PatternPoint.objects.create(
                instance_id=instance_id,
                point_type=point.pivot_type,  # "HIGH" or "LOW"
                price=point.price,
                ts=point.ts,  # From candle timestamp
                bar_index=point.bar_index,
            )

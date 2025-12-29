"""
Pattern Engine Use Cases.

Orchestrates pattern detection workflow:
1. Fetch candles from exchange
2. Run detectors on candle window
3. Persist pattern instances (idempotent)
4. Emit pattern alerts (idempotent)
5. Check confirmation/invalidation for existing patterns

NO ORDER PLACEMENT - this is pure detection + alerting.
EntryGate (CORE 1.2) consumes these alerts for trade decisions.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from decimal import Decimal

from .domain import PatternLifecycleEvent
from .ports import CandleProvider, PatternDetector, PatternRepository

logger = logging.getLogger(__name__)


@dataclass
class PatternScanCommand:
    """
    Command to scan for patterns on symbol/timeframe.

    Args:
        symbol: Trading pair (e.g., "BTCUSDT")
        timeframe: Candle interval (e.g., "15m", "1h")
        detectors: List of detector instances to run
        candle_limit: Number of candles to fetch (default: 100)
    """

    symbol: str
    timeframe: str
    detectors: list[PatternDetector]
    candle_limit: int = 100


@dataclass
class PatternScanResult:
    """
    Result of pattern scan operation.

    Contains summary of detection, persistence, and alert emission.
    """

    symbol: str
    timeframe: str
    candles_fetched: int
    detectors_run: int
    patterns_detected: int
    instances_created: int
    instances_existing: int
    alerts_created: int  # New alerts emitted (idempotent tracking)
    alerts_existing: int  # Alerts already exist (duplicates)
    confirmations_checked: int
    confirmations_found: int
    invalidations_checked: int
    invalidations_found: int
    events: list[PatternLifecycleEvent]


class PatternScanUseCase:
    """
    Orchestrate pattern detection workflow.

    This is a PURE DETECTION ENGINE:
    - Fetches candles
    - Runs detectors
    - Persists instances
    - Emits alerts

    Does NOT:
    - Place orders
    - Call EntryGate
    - Make trading decisions

    EntryGate (CORE 1.2) consumes PatternAlerts for trade entry decisions.
    """

    def __init__(
        self,
        candle_provider: CandleProvider,
        pattern_repository: PatternRepository,
    ):
        """
        Initialize use case.

        Args:
            candle_provider: Candle fetching adapter (Binance)
            pattern_repository: Pattern persistence adapter (Django ORM)
        """
        self._candle_provider = candle_provider
        self._pattern_repository = pattern_repository

    def execute(self, command: PatternScanCommand) -> PatternScanResult:
        """
        Execute pattern scan workflow.

        Steps:
        1. Fetch candles from provider
        2. Run each detector on candle window
        3. For each detected signature:
           a. Get or create PatternInstance (idempotent)
           b. Emit FORMING alert (idempotent)
           c. Store pattern-specific details
        4. Check confirmation for FORMING patterns
        5. Check invalidation for active patterns

        Args:
            command: Scan command with symbol, timeframe, detectors

        Returns:
            PatternScanResult with summary and events
        """
        logger.info(
            f"Starting pattern scan: {command.symbol} {command.timeframe} "
            f"with {len(command.detectors)} detectors"
        )

        # Step 1: Fetch candles
        window = self._candle_provider.get_candles(
            symbol=command.symbol,
            timeframe=command.timeframe,
            limit=command.candle_limit,
        )

        logger.debug(f"Fetched {len(window)} candles for {command.symbol} {command.timeframe}")

        # Initialize result counters
        patterns_detected = 0
        instances_created = 0
        instances_existing = 0
        alerts_created = 0
        alerts_existing = 0
        events: list[PatternLifecycleEvent] = []

        # Step 2: Run each detector
        for detector in command.detectors:
            signatures = detector.detect(window)

            if not signatures:
                logger.debug(f"No {detector.pattern_code} patterns found")
                continue

            logger.info(f"Detected {len(signatures)} {detector.pattern_code} pattern(s)")

            # Step 3: Persist each detected signature
            for signature in signatures:
                patterns_detected += 1

                # 3a. Get or create instance (idempotent)
                instance, created = self._pattern_repository.get_or_create_instance(signature)

                if created:
                    instances_created += 1
                    logger.info(
                        f"Created new pattern instance #{instance.id}: "
                        f"{signature.pattern_code} on {signature.symbol}"
                    )
                else:
                    instances_existing += 1
                    logger.debug(
                        f"Pattern instance #{instance.id} already exists (idempotent duplicate)"
                    )

                # 3b. Emit FORMING alert (idempotent)
                alert, alert_created = self._pattern_repository.emit_alert(
                    instance_id=instance.id,
                    alert_type="FORMING",
                    alert_ts=signature.end_ts,  # From candle timestamp
                    confidence=signature.confidence,
                    payload={
                        "pattern_code": signature.pattern_code,
                        "symbol": signature.symbol,
                        "timeframe": signature.timeframe,
                        "evidence": signature.evidence,
                        "alert_message": self._generate_alert_message(signature),
                    },
                )
                if alert_created:
                    alerts_created += 1
                else:
                    alerts_existing += 1

                # Create lifecycle event
                event = PatternLifecycleEvent(
                    instance_id=instance.id,
                    event_type="FORMING",
                    event_ts=signature.end_ts,
                    confidence=signature.confidence,
                    evidence=signature.evidence,
                    version="pattern_engine_v1.0.0",
                )
                events.append(event)

                # 3c. Store pattern-specific details
                if signature.pattern_code in [
                    "HAMMER",
                    "INVERTED_HAMMER",
                    "BULLISH_ENGULFING",
                    "BEARISH_ENGULFING",
                    "MORNING_STAR",
                ]:
                    # Candlestick pattern details
                    self._pattern_repository.store_candlestick_detail(
                        instance_id=instance.id,
                        metrics=self._extract_candlestick_metrics(signature),
                    )

                elif signature.pattern_code in [
                    "HEAD_AND_SHOULDERS",
                    "INVERTED_HEAD_AND_SHOULDERS",
                ]:
                    # Chart pattern details
                    self._pattern_repository.store_chart_detail(
                        instance_id=instance.id,
                        metrics=self._extract_chart_metrics(signature),
                    )

                    # Store pivot points
                    self._pattern_repository.store_pattern_points(
                        instance_id=instance.id,
                        points=list(signature.key_points),
                    )

        # Step 4: Check confirmations for FORMING patterns
        (
            confirmations_checked,
            confirmations_found,
            confirmation_alerts_created,
            confirmation_alerts_existing,
        ) = self._check_confirmations(command, window, events)
        alerts_created += confirmation_alerts_created
        alerts_existing += confirmation_alerts_existing

        # Step 5: Check invalidations for active patterns
        (
            invalidations_checked,
            invalidations_found,
            invalidation_alerts_created,
            invalidation_alerts_existing,
        ) = self._check_invalidations(command, window, events)
        alerts_created += invalidation_alerts_created
        alerts_existing += invalidation_alerts_existing

        # Build result
        result = PatternScanResult(
            symbol=command.symbol,
            timeframe=command.timeframe,
            candles_fetched=len(window),
            detectors_run=len(command.detectors),
            patterns_detected=patterns_detected,
            instances_created=instances_created,
            instances_existing=instances_existing,
            alerts_created=alerts_created,
            alerts_existing=alerts_existing,
            confirmations_checked=confirmations_checked,
            confirmations_found=confirmations_found,
            invalidations_checked=invalidations_checked,
            invalidations_found=invalidations_found,
            events=events,
        )

        logger.info(
            f"Pattern scan complete: {patterns_detected} detected, "
            f"{instances_created} created, {confirmations_found} confirmed, "
            f"{invalidations_found} invalidated"
        )

        return result

    def _check_confirmations(
        self,
        command: PatternScanCommand,
        window,
        events: list[PatternLifecycleEvent],
    ) -> tuple[int, int, int, int]:
        """
        Check confirmation for FORMING patterns.

        Queries all FORMING instances for this symbol/timeframe,
        runs confirmation checks, updates status if confirmed.

        Args:
            command: Scan command
            window: Current candle window
            events: Event list to append to

        Returns:
            (checked_count, confirmed_count, alerts_created, alerts_existing) tuple
        """
        # Import here to avoid circular dependency
        from api.models.patterns.base import PatternInstance

        forming_instances = PatternInstance.objects.filter(
            symbol=command.symbol,
            timeframe=command.timeframe,
            status="FORMING",
        )

        checked = 0
        confirmed = 0
        alerts_created = 0
        alerts_existing = 0

        for instance in forming_instances:
            checked += 1

            # Find matching detector
            detector = self._find_detector(command.detectors, instance.pattern_code)
            if not detector:
                continue

            # Run confirmation check
            confirmation_evidence = detector.check_confirmation(instance, window)
            if confirmation_evidence:
                confirmed += 1

                # Update status to CONFIRMED
                self._pattern_repository.update_status(
                    instance_id=instance.id,
                    status="CONFIRMED",
                    event_ts=window[-1].ts,  # From latest candle timestamp
                    evidence=confirmation_evidence,
                )

                # Emit CONFIRM alert (track idempotency)
                alert, alert_created = self._pattern_repository.emit_alert(
                    instance_id=instance.id,
                    alert_type="CONFIRM",
                    alert_ts=window[-1].ts,  # From candle timestamp
                    confidence=instance.confidence + Decimal("0.10"),  # Boost confidence
                    payload=confirmation_evidence,
                )
                if alert_created:
                    alerts_created += 1
                else:
                    alerts_existing += 1

                # Create lifecycle event
                event = PatternLifecycleEvent(
                    instance_id=instance.id,
                    event_type="CONFIRMED",
                    event_ts=window[-1].ts,
                    confidence=instance.confidence + Decimal("0.10"),
                    evidence=confirmation_evidence,
                    version="pattern_engine_v1.0.0",
                )
                events.append(event)

                logger.info(f"Pattern #{instance.id} CONFIRMED: {instance.pattern_code}")

        return checked, confirmed, alerts_created, alerts_existing

    def _check_invalidations(
        self,
        command: PatternScanCommand,
        window,
        events: list[PatternLifecycleEvent],
    ) -> tuple[int, int, int, int]:
        """
        Check invalidation for active patterns.

        Queries all FORMING/CONFIRMED instances for this symbol/timeframe,
        runs invalidation checks, updates status if invalidated.

        Args:
            command: Scan command
            window: Current candle window
            events: Event list to append to

        Returns:
            (checked_count, invalidated_count, alerts_created, alerts_existing) tuple
        """
        # Import here to avoid circular dependency
        from api.models.patterns.base import PatternInstance

        active_instances = PatternInstance.objects.filter(
            symbol=command.symbol,
            timeframe=command.timeframe,
            status__in=["FORMING", "CONFIRMED"],
        )

        checked = 0
        invalidated = 0
        alerts_created = 0
        alerts_existing = 0

        for instance in active_instances:
            checked += 1

            # Find matching detector
            detector = self._find_detector(command.detectors, instance.pattern_code)
            if not detector:
                continue

            # Run invalidation check
            invalidation_evidence = detector.check_invalidation(instance, window)
            if invalidation_evidence:
                invalidated += 1

                # Update status to INVALIDATED
                self._pattern_repository.update_status(
                    instance_id=instance.id,
                    status="INVALIDATED",
                    event_ts=window[-1].ts,  # From latest candle timestamp
                    evidence=invalidation_evidence,
                )

                # Emit INVALIDATE alert (track idempotency)
                alert, alert_created = self._pattern_repository.emit_alert(
                    instance_id=instance.id,
                    alert_type="INVALIDATE",
                    alert_ts=window[-1].ts,  # From candle timestamp
                    confidence=Decimal("0"),  # Zero confidence when invalidated
                    payload=invalidation_evidence,
                )
                if alert_created:
                    alerts_created += 1
                else:
                    alerts_existing += 1

                # Create lifecycle event
                event = PatternLifecycleEvent(
                    instance_id=instance.id,
                    event_type="INVALIDATED",
                    event_ts=window[-1].ts,
                    confidence=Decimal("0"),
                    evidence=invalidation_evidence,
                    version="pattern_engine_v1.0.0",
                )
                events.append(event)

                logger.info(f"Pattern #{instance.id} INVALIDATED: {instance.pattern_code}")

        return checked, invalidated, alerts_created, alerts_existing

    def _find_detector(
        self, detectors: list[PatternDetector], pattern_code: str
    ) -> PatternDetector | None:
        """
        Find detector by pattern code.

        Args:
            detectors: List of detector instances
            pattern_code: Pattern code to search for

        Returns:
            Matching detector or None
        """
        for detector in detectors:
            if detector.pattern_code == pattern_code:
                return detector
            # Handle BULLISH/BEARISH_ENGULFING variants
            if pattern_code in ["BULLISH_ENGULFING", "BEARISH_ENGULFING"]:
                if detector.pattern_code == "ENGULFING":
                    return detector
        return None

    def _generate_alert_message(self, signature) -> str:
        """
        Generate human-readable alert message.

        Args:
            signature: Pattern signature

        Returns:
            Alert message string
        """
        pattern_name = signature.pattern_code.replace("_", " ").title()
        return (
            f"{pattern_name} pattern detected on {signature.symbol} "
            f"{signature.timeframe} at {signature.end_ts.strftime('%Y-%m-%d %H:%M')}"
        )

    def _extract_candlestick_metrics(self, signature) -> dict:
        """
        Extract candlestick-specific metrics from signature.

        Args:
            signature: Pattern signature

        Returns:
            Dict with candlestick metrics
        """
        evidence = signature.evidence

        metrics = {
            "body_pct_main": evidence.get("body_pct"),
            "upper_wick_pct_main": evidence.get("upper_wick_pct"),
            "lower_wick_pct_main": evidence.get("lower_wick_pct"),
        }

        # Engulfing pattern has second candle metrics
        if "second_candle_body_pct" in evidence:
            metrics["body_pct_second"] = evidence.get("second_candle_body_pct")
            metrics["engulf_ratio"] = evidence.get("engulf_ratio")

        return metrics

    def _extract_chart_metrics(self, signature) -> dict:
        """
        Extract chart pattern metrics from signature.

        Args:
            signature: Pattern signature

        Returns:
            Dict with chart pattern metrics
        """
        evidence = signature.evidence

        return {
            "neckline_slope": evidence.get("neckline_slope_pct"),
            "head_prominence_pct": evidence.get("head_prominence_pct"),
            "shoulder_symmetry": evidence.get("shoulder_symmetry"),
            "target_price": evidence.get("target_price"),
            "breakout_price": None,  # Set when confirmed
        }

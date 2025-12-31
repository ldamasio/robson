"""
Django Management Command: Scan Patterns (Continuous Mode)

Usage:
    python manage.py scan_patterns --continuous --all
    python manage.py scan_patterns --symbols BTCUSDT,ETHUSDT --timeframes 15m,1h

Continuous scanning mode for CronJob.
Scans for pattern detections and updates existing instances.
"""

import logging
from datetime import timedelta

from django.core.management.base import BaseCommand, CommandError
from django.utils import timezone
from django.conf import settings

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
from api.application.pattern_engine.pattern_to_plan import (
    PatternAlertProcessor,
    PatternToPlanResult,
)
from api.models.patterns.base import PatternInstance, PatternStatus
from api.services.binance_service import BinanceService

logger = logging.getLogger(__name__)


class Command(BaseCommand):
    help = "Continuous pattern scanning for CronJob (detect + update lifecycle)"

    def add_arguments(self, parser):
        parser.add_argument(
            "--continuous",
            action="store_true",
            help="Run in continuous mode (check lifecycle for existing patterns)",
        )
        parser.add_argument(
            "--symbols",
            type=str,
            default="BTCUSDT",
            help="Comma-separated symbols (default: BTCUSDT)",
        )
        parser.add_argument(
            "--timeframes",
            type=str,
            default="15m,1h",
            help="Comma-separated timeframes (default: 15m,1h)",
        )
        parser.add_argument(
            "--all",
            action="store_true",
            help="Run all detectors",
        )
        parser.add_argument(
            "--candlestick",
            action="store_true",
            help="Run candlestick detectors only",
        )
        parser.add_argument(
            "--chart",
            action="store_true",
            help="Run chart detectors only",
        )
        parser.add_argument(
            "--testnet",
            action="store_true",
            help="Use Binance testnet instead of production",
        )
        parser.add_argument(
            "--process-plans",
            action="store_true",
            help="Process CONFIRMED patterns into trading plans",
        )
        parser.add_argument(
            "--verbose",
            action="store_true",
            help="Enable verbose output",
        )

    def handle(self, *args, **options):
        # Configure logging
        if options["verbose"]:
            logging.basicConfig(level=logging.DEBUG)
        else:
            logging.basicConfig(level=logging.INFO)

        symbols = [s.strip().upper() for s in options["symbols"].split(",")]
        timeframes = [t.strip() for t in options["timeframes"].split(",")]

        self.stdout.write(
            self.style.SUCCESS(
                f"\n{'='*60}\n"
                f"PATTERN SCAN ENGINE (Continuous)\n"
                f"{'='*60}\n"
                f"Symbols:    {', '.join(symbols)}\n"
                f"Timeframes: {', '.join(timeframes)}\n"
                f"Continuous: {options['continuous']}\n"
                f"Process Plans: {options['process_plans']}\n"
                f"Testnet: {options['testnet']}\n"
                f"{'='*60}\n"
            )
        )

        # Initialize adapters
        # BinanceService is a singleton that uses settings for credentials
        use_testnet = options.get("testnet", getattr(settings, "BINANCE_USE_TESTNET", False))
        binance_service = BinanceService(use_testnet=use_testnet)
        candle_provider = BinanceCandleProvider(binance_service)
        # For CronJob/system-wide scans, client=None creates system-owned patterns
        pattern_repository = DjangoPatternRepository(client=None)

        # Initialize detectors
        detectors = self._select_detectors(options)

        if not detectors:
            raise CommandError("No detectors selected")

        self.stdout.write(f"Detectors: {len(detectors)}\n")

        # Initialize use case
        use_case = PatternScanUseCase(
            candle_provider=candle_provider,
            pattern_repository=pattern_repository,
        )

        # Scan each symbol/timeframe combination
        total_patterns = 0
        total_confirmations = 0
        total_invalidations = 0
        plans_created = 0

        for symbol in symbols:
            for timeframe in timeframes:
                self.stdout.write(f"\nScanning {symbol} {timeframe}...")

                try:
                    # Create scan command
                    command = PatternScanCommand(
                        symbol=symbol,
                        timeframe=timeframe,
                        detectors=detectors,
                        candle_limit=100,
                    )

                    # Execute scan
                    result = use_case.execute(command)

                    total_patterns += result.patterns_detected
                    total_confirmations += result.confirmations_found
                    total_invalidations += result.invalidations_found

                    self.stdout.write(
                        f"  Detected: {result.patterns_detected}, "
                        f"Confirmed: {result.confirmations_found}, "
                        f"Invalidated: {result.invalidations_found}"
                    )

                except Exception as e:
                    self.stderr.write(
                        self.style.ERROR(f"  Scan failed: {e}")
                    )
                    logger.exception(f"Pattern scan failed for {symbol} {timeframe}")

        # Continuous mode: update lifecycle for existing patterns
        if options["continuous"]:
            self.stdout.write(f"\n{'='*60}")
            self.stdout.write("Updating pattern lifecycle...")
            self._update_lifecycle(candle_provider, pattern_repository, detectors)

        # Process CONFIRMED patterns into trading plans
        if options["process_plans"]:
            self.stdout.write(f"\n{'='*60}")
            self.stdout.write("Processing CONFIRMED patterns...")
            plans_created = self._process_confirmed_patterns()

        # Final summary
        self.stdout.write(f"\n{'='*60}")
        self.stdout.write(self.style.SUCCESS("SUMMARY"))
        self.stdout.write(f"  Patterns detected: {total_patterns}")
        self.stdout.write(f"  Confirmations: {total_confirmations}")
        self.stdout.write(f"  Invalidations: {total_invalidations}")
        self.stdout.write(f"  Plans created: {plans_created}")
        self.stdout.write(f"{'='*60}\n")

    def _select_detectors(self, options):
        """Select detectors based on options."""
        if options["all"] or not (options["candlestick"] or options["chart"]):
            return [
                HammerDetector(),
                InvertedHammerDetector(),
                EngulfingDetector(),
                MorningStarDetector(),
                HeadAndShouldersDetector(),
                InvertedHeadAndShouldersDetector(),
            ]
        if options["candlestick"]:
            return [
                HammerDetector(),
                InvertedHammerDetector(),
                EngulfingDetector(),
                MorningStarDetector(),
            ]
        if options["chart"]:
            return [
                HeadAndShouldersDetector(),
                InvertedHeadAndShouldersDetector(),
            ]
        return []

    def _update_lifecycle(self, candle_provider, pattern_repository, detectors):
        """Update lifecycle for existing FORMING patterns."""
        from api.application.pattern_engine.domain import CandleWindow

        # Get FORMING patterns
        forming_patterns = PatternInstance.objects.filter(
            status=PatternStatus.FORMING
        ).select_related("pattern", "symbol")

        self.stdout.write(f"  Checking {forming_patterns.count()} FORMING patterns...")

        for pattern in forming_patterns:
            try:
                # Fetch recent candles for confirmation check
                window = candle_provider.get_candles(
                    pattern.symbol.name,
                    pattern.timeframe,
                    limit=20,  # Last 20 candles
                )

                # Get detector for this pattern
                detector = self._get_detector_for_pattern(pattern.pattern.pattern_code, detectors)
                if not detector:
                    continue

                # Check confirmation
                confirmation = detector.check_confirmation(pattern, window)
                if confirmation:
                    pattern_repository.update_status(
                        pattern.id,
                        "CONFIRMED",
                        window[-1].ts,
                        confirmation,
                    )
                    self.stdout.write(
                        self.style.SUCCESS(f"    CONFIRMED: {pattern.pattern.pattern_code}")
                    )
                    continue

                # Check invalidation
                invalidation = detector.check_invalidation(pattern, window)
                if invalidation:
                    pattern_repository.update_status(
                        pattern.id,
                        "INVALIDATED",
                        window[-1].ts,
                        invalidation,
                    )
                    self.stdout.write(
                        self.style.ERROR(f"    INVALIDATED: {pattern.pattern.pattern_code}")
                    )

            except Exception as e:
                logger.exception(f"Failed to update pattern {pattern.id}")

    def _get_detector_for_pattern(self, pattern_code, detectors):
        """Get detector for pattern code."""
        for detector in detectors:
            if detector.pattern_code == pattern_code:
                return detector
            # Handle engulfing subtypes
            if pattern_code in ("BULLISH_ENGULFING", "BEARISH_ENGULFING"):
                if detector.pattern_code == "ENGULFING":
                    return detector
        return None

    def _process_confirmed_patterns(self):
        """Process CONFIRMED patterns into trading plans."""
        from api.models.patterns.base import PatternAlert

        # Get recent CONFIRM alerts not yet processed
        # (In production, add a 'processed_for_plan' flag to PatternAlert)
        cutoff = timezone.now() - timedelta(minutes=5)

        recent_confirms = PatternAlert.objects.filter(
            alert_type=PatternAlert.AlertType.CONFIRM,
            alert_ts__gte=cutoff,
        ).select_related("instance__pattern", "instance__symbol")

        processor = PatternAlertProcessor()
        plans_created = 0

        for alert in recent_confirms:
            try:
                result = processor.process_confirmed_alert(alert.id)
                if result.success:
                    plans_created += result.plans_created
                    self.stdout.write(
                        self.style.SUCCESS(f"    Plan created for {alert.instance.pattern.pattern_code}")
                    )
            except Exception as e:
                self.stderr.write(f"    Plan creation failed: {e}")
                logger.exception(f"Failed to process alert {alert.id}")

        return plans_created

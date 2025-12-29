"""
Django Management Command: Detect Patterns

Usage:
    python manage.py detect_patterns BTCUSDT 15m --all
    python manage.py detect_patterns BTCUSDT 1h --hammer --hns
    python manage.py detect_patterns ETHUSDT 15m --candlestick
    python manage.py detect_patterns BTCUSDT 15m --chart

Scans for pattern detections using the Pattern Engine (CORE 1.0).
Output: PatternInstances, PatternAlerts in database (NO order placement).
"""

import logging

from django.core.management.base import BaseCommand, CommandError

from api.application.pattern_engine import PatternScanCommand, PatternScanUseCase
from api.application.pattern_engine.adapters import BinanceCandleProvider, DjangoPatternRepository
from api.application.pattern_engine.detectors import (
    EngulfingDetector,
    HammerDetector,
    HeadAndShouldersDetector,
    InvertedHammerDetector,
    InvertedHeadAndShouldersDetector,
    MorningStarDetector,
)
from api.models import BinanceClient

logger = logging.getLogger(__name__)


class Command(BaseCommand):
    help = "Detect chart and candlestick patterns for a given symbol/timeframe"

    def add_arguments(self, parser):
        # Positional arguments
        parser.add_argument(
            "symbol",
            type=str,
            help="Trading pair (e.g., BTCUSDT, ETHUSDT)",
        )
        parser.add_argument(
            "timeframe",
            type=str,
            help="Candle interval (e.g., 15m, 1h, 4h, 1d)",
        )

        # Detector selection (mutually exclusive groups)
        detector_group = parser.add_mutually_exclusive_group()
        detector_group.add_argument(
            "--all",
            action="store_true",
            help="Run all detectors (default)",
        )
        detector_group.add_argument(
            "--candlestick",
            action="store_true",
            help="Run all candlestick pattern detectors",
        )
        detector_group.add_argument(
            "--chart",
            action="store_true",
            help="Run all chart pattern detectors",
        )

        # Individual detector flags
        parser.add_argument(
            "--hammer",
            action="store_true",
            help="Detect Hammer patterns",
        )
        parser.add_argument(
            "--inverted-hammer",
            action="store_true",
            help="Detect Inverted Hammer patterns",
        )
        parser.add_argument(
            "--engulfing",
            action="store_true",
            help="Detect Bullish/Bearish Engulfing patterns",
        )
        parser.add_argument(
            "--morning-star",
            action="store_true",
            help="Detect Morning Star patterns",
        )
        parser.add_argument(
            "--hns",
            action="store_true",
            help="Detect Head & Shoulders patterns",
        )
        parser.add_argument(
            "--ihns",
            action="store_true",
            help="Detect Inverted Head & Shoulders patterns",
        )

        # Additional options
        parser.add_argument(
            "--client-id",
            type=int,
            help="BinanceClient ID (defaults to first active client)",
        )
        parser.add_argument(
            "--candle-limit",
            type=int,
            default=100,
            help="Number of candles to fetch (default: 100)",
        )
        parser.add_argument(
            "--verbose",
            action="store_true",
            help="Enable verbose output",
        )

    def handle(self, *args, **options):
        symbol = options["symbol"].upper()
        timeframe = options["timeframe"]
        candle_limit = options["candle_limit"]

        # Configure logging
        if options["verbose"]:
            logging.basicConfig(level=logging.DEBUG)
        else:
            logging.basicConfig(level=logging.INFO)

        self.stdout.write(
            self.style.SUCCESS(
                f"\n{'='*60}\n"
                f"PATTERN DETECTION ENGINE v1.0.0\n"
                f"{'='*60}\n"
                f"Symbol:    {symbol}\n"
                f"Timeframe: {timeframe}\n"
                f"Candles:   {candle_limit}\n"
                f"{'='*60}\n"
            )
        )

        # Get BinanceClient
        try:
            if options["client_id"]:
                client = BinanceClient.objects.get(id=options["client_id"])
            else:
                client = BinanceClient.objects.filter(is_active=True).first()
                if not client:
                    raise CommandError("No active BinanceClient found")

            self.stdout.write(f"Using client: {client.name} (ID: {client.id})\n")

        except BinanceClient.DoesNotExist:
            raise CommandError(f"BinanceClient with ID {options['client_id']} not found")

        # Initialize adapters
        candle_provider = BinanceCandleProvider(client)
        pattern_repository = DjangoPatternRepository()

        # Initialize detectors based on arguments
        detectors = self._select_detectors(options)

        if not detectors:
            raise CommandError(
                "No detectors selected. Use --all, --candlestick, --chart, or specific flags."
            )

        self.stdout.write(f"Detectors: {len(detectors)} active\n")
        for detector in detectors:
            self.stdout.write(f"  - {detector.pattern_code}\n")

        self.stdout.write(f"\n{'='*60}\n")

        # Initialize use case
        use_case = PatternScanUseCase(
            candle_provider=candle_provider,
            pattern_repository=pattern_repository,
        )

        # Create command
        command = PatternScanCommand(
            symbol=symbol,
            timeframe=timeframe,
            detectors=detectors,
            candle_limit=candle_limit,
        )

        # Execute scan
        try:
            self.stdout.write("Executing pattern scan...\n")
            result = use_case.execute(command)

            # Display results
            self._display_results(result)

        except Exception as e:
            raise CommandError(f"Pattern scan failed: {e}")

    def _select_detectors(self, options):
        """
        Select detectors based on command-line arguments.

        Args:
            options: Parsed command-line options

        Returns:
            List of detector instances
        """
        detectors = []

        # Check if any individual flags are set
        individual_flags = (
            options["hammer"]
            or options["inverted_hammer"]
            or options["engulfing"]
            or options["morning_star"]
            or options["hns"]
            or options["ihns"]
        )

        # If --all or no specific flags, use all detectors
        if options["all"] or not (options["candlestick"] or options["chart"] or individual_flags):
            detectors = [
                HammerDetector(),
                InvertedHammerDetector(),
                EngulfingDetector(),
                MorningStarDetector(),
                HeadAndShouldersDetector(),
                InvertedHeadAndShouldersDetector(),
            ]
            return detectors

        # If --candlestick, add all candlestick detectors
        if options["candlestick"]:
            detectors.extend(
                [
                    HammerDetector(),
                    InvertedHammerDetector(),
                    EngulfingDetector(),
                    MorningStarDetector(),
                ]
            )
            return detectors

        # If --chart, add all chart detectors
        if options["chart"]:
            detectors.extend(
                [
                    HeadAndShouldersDetector(),
                    InvertedHeadAndShouldersDetector(),
                ]
            )
            return detectors

        # Individual detector selection
        if options["hammer"]:
            detectors.append(HammerDetector())
        if options["inverted_hammer"]:
            detectors.append(InvertedHammerDetector())
        if options["engulfing"]:
            detectors.append(EngulfingDetector())
        if options["morning_star"]:
            detectors.append(MorningStarDetector())
        if options["hns"]:
            detectors.append(HeadAndShouldersDetector())
        if options["ihns"]:
            detectors.append(InvertedHeadAndShouldersDetector())

        return detectors

    def _display_results(self, result):
        """
        Display pattern scan results in terminal.

        Args:
            result: PatternScanResult instance
        """
        self.stdout.write(self.style.SUCCESS(f"\n{'='*60}\nSCAN RESULTS\n{'='*60}\n"))

        # Detection summary
        self.stdout.write(self.style.WARNING("\n📊 DETECTION SUMMARY\n"))
        self.stdout.write(f"  Candles fetched:      {result.candles_fetched}\n")
        self.stdout.write(f"  Detectors run:        {result.detectors_run}\n")
        self.stdout.write(
            self.style.SUCCESS(f"  Patterns detected:    {result.patterns_detected}\n")
        )

        # Persistence summary
        self.stdout.write(self.style.WARNING("\n💾 PERSISTENCE SUMMARY\n"))
        self.stdout.write(
            self.style.SUCCESS(f"  Instances created:    {result.instances_created}\n")
        )
        self.stdout.write(f"  Instances existing:   {result.instances_existing}\n")
        self.stdout.write(self.style.SUCCESS(f"  Alerts created:       {result.alerts_created}\n"))
        self.stdout.write(f"  Alerts existing:      {result.alerts_existing}\n")

        # Lifecycle summary
        self.stdout.write(self.style.WARNING("\n🔄 LIFECYCLE SUMMARY\n"))
        self.stdout.write(f"  Confirmations checked:  {result.confirmations_checked}\n")
        self.stdout.write(
            self.style.SUCCESS(f"  Confirmations found:    {result.confirmations_found}\n")
        )
        self.stdout.write(f"  Invalidations checked:  {result.invalidations_checked}\n")
        self.stdout.write(
            self.style.ERROR(f"  Invalidations found:    {result.invalidations_found}\n")
        )

        # Event details
        if result.events:
            self.stdout.write(self.style.WARNING(f"\n📝 LIFECYCLE EVENTS ({len(result.events)})\n"))
            for event in result.events:
                event_style = {
                    "FORMING": self.style.NOTICE,
                    "CONFIRMED": self.style.SUCCESS,
                    "INVALIDATED": self.style.ERROR,
                }
                style_func = event_style.get(event.event_type, lambda x: x)

                self.stdout.write(
                    style_func(
                        f"  [{event.event_type}] "
                        f"Instance #{event.instance_id} "
                        f"at {event.event_ts.strftime('%Y-%m-%d %H:%M')} "
                        f"(confidence: {event.confidence})\n"
                    )
                )

        # Idempotency note
        if result.instances_existing > 0 or result.alerts_existing > 0:
            self.stdout.write(
                self.style.NOTICE(
                    f"\n⚠️  IDEMPOTENCY: "
                    f"{result.instances_existing} duplicate instances and "
                    f"{result.alerts_existing} duplicate alerts were skipped "
                    f"(already exist in database)\n"
                )
            )

        # Final summary
        self.stdout.write(f"\n{'='*60}\n")
        if result.patterns_detected > 0:
            self.stdout.write(
                self.style.SUCCESS(
                    f"✅ Pattern scan complete! "
                    f"{result.patterns_detected} pattern(s) detected.\n"
                )
            )
        else:
            self.stdout.write(
                self.style.WARNING("✅ Pattern scan complete. No patterns detected.\n")
            )

        self.stdout.write(
            f"\n💡 Next steps:\n"
            f"   - View instances: PatternInstance.objects.filter(symbol='{result.symbol}')\n"
            f"   - View alerts:    PatternAlert.objects.filter(instance__symbol='{result.symbol}')\n"
            f"   - EntryGate (CORE 1.2) will consume these alerts for trade decisions\n"
        )
        self.stdout.write(f"{'='*60}\n\n")

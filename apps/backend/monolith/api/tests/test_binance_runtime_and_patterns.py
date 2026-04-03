from datetime import timedelta
from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import MagicMock

from django.test import SimpleTestCase, TestCase, override_settings
from django.utils import timezone

from api.application.pattern_engine.adapters import DjangoPatternRepository
from api.application.pattern_engine.use_cases import PatternScanUseCase
from api.management.commands.detect_patterns import Command as DetectPatternsCommand
from api.management.commands.scan_patterns import Command as ScanPatternsCommand
from api.models import Symbol
from api.models.patterns.base import PatternCatalog, PatternInstance
from api.services.binance_service import (
    get_binance_runtime_config,
    has_binance_credentials,
)
from clients.models import Client


class TestBinanceRuntimeConfig(SimpleTestCase):
    @override_settings(
        BINANCE_ENV="production",
        BINANCE_MODE="PRODUCTION",
        BINANCE_USE_TESTNET=False,
        BINANCE_API_KEY_ACTIVE="prod-key",
        BINANCE_SECRET_KEY_ACTIVE="prod-secret",
        BINANCE_API_URL_ACTIVE="https://api.binance.com",
    )
    def test_runtime_config_uses_active_settings_by_default(self):
        runtime = get_binance_runtime_config()

        self.assertFalse(runtime.use_testnet)
        self.assertEqual(runtime.environment, "production")
        self.assertEqual(runtime.mode, "PRODUCTION")
        self.assertEqual(runtime.api_key, "prod-key")
        self.assertEqual(runtime.secret_key, "prod-secret")
        self.assertTrue(runtime.has_credentials)
        self.assertTrue(has_binance_credentials())

    @override_settings(
        BINANCE_ENV="production",
        BINANCE_MODE="PRODUCTION",
        BINANCE_USE_TESTNET=False,
        BINANCE_API_KEY_ACTIVE="prod-key",
        BINANCE_SECRET_KEY_ACTIVE="prod-secret",
        BINANCE_API_KEY_TEST="test-key",
        BINANCE_SECRET_KEY_TEST="test-secret",
        BINANCE_API_URL_TEST="https://testnet.binance.vision/api",
    )
    def test_runtime_config_override_switches_to_matching_credentials(self):
        runtime = get_binance_runtime_config(use_testnet=True)

        self.assertTrue(runtime.use_testnet)
        self.assertEqual(runtime.environment, "testnet")
        self.assertEqual(runtime.mode, "TESTNET")
        self.assertEqual(runtime.api_key, "test-key")
        self.assertEqual(runtime.secret_key, "test-secret")
        self.assertEqual(runtime.api_url, "https://testnet.binance.vision/api")


class TestPatternCommandFlags(SimpleTestCase):
    def test_scan_patterns_cli_lets_settings_drive_default_mode(self):
        parser = ScanPatternsCommand().create_parser("manage.py", "scan_patterns")
        options = vars(parser.parse_args([]))

        self.assertIsNone(options["testnet"])

    def test_detect_patterns_cli_lets_settings_drive_default_mode(self):
        parser = DetectPatternsCommand().create_parser("manage.py", "detect_patterns")
        options = vars(parser.parse_args(["BTCUSDT", "15m"]))

        self.assertIsNone(options["testnet"])


class TestPatternEngineCompatibility(TestCase):
    def setUp(self):
        self.client_tenant = Client.objects.create(
            name="Pattern Tenant",
            email="patterns@example.com",
        )
        self.symbol = Symbol.objects.create(
            client=self.client_tenant,
            name="BTCUSDT",
            description="Bitcoin / Tether",
            base_asset="BTC",
            quote_asset="USDT",
        )
        self.pattern = PatternCatalog.objects.create(
            pattern_code="HAMMER",
            name="Hammer",
            category="CANDLESTICK",
            direction_bias="BULLISH",
        )

    def test_pattern_instance_exposes_legacy_compatibility_properties(self):
        instance = PatternInstance.objects.create(
            client=self.client_tenant,
            pattern=self.pattern,
            symbol=self.symbol,
            timeframe="1h",
            start_ts=timezone.now() - timedelta(hours=1),
            end_ts=timezone.now(),
            status="FORMING",
            features={
                "evidence": {"candle_high": "101.5"},
                "confidence": 0.82,
            },
        )

        self.assertEqual(instance.pattern_code, "HAMMER")
        self.assertEqual(instance.evidence["candle_high"], "101.5")
        self.assertEqual(instance.confidence, Decimal("0.82"))

    def test_store_candlestick_detail_is_idempotent(self):
        instance = PatternInstance.objects.create(
            client=self.client_tenant,
            pattern=self.pattern,
            symbol=self.symbol,
            timeframe="1h",
            start_ts=timezone.now() - timedelta(hours=1),
            end_ts=timezone.now(),
            status="FORMING",
            features={"evidence": {}, "confidence": 0.5},
        )
        repository = DjangoPatternRepository(client=self.client_tenant)

        repository.store_candlestick_detail(
            instance.id,
            {
                "body_pct": Decimal("0.10"),
                "upper_wick_pct": Decimal("0.15"),
                "lower_wick_pct": Decimal("0.75"),
                "engulf_ratio": None,
            },
        )
        repository.store_candlestick_detail(
            instance.id,
            {
                "body_pct": Decimal("0.11"),
                "upper_wick_pct": Decimal("0.14"),
                "lower_wick_pct": Decimal("0.75"),
                "engulf_ratio": None,
            },
        )

        instance.refresh_from_db()
        self.assertEqual(instance.candlestick_detail.body_pct_main, Decimal("0.11"))

    def test_check_confirmations_filters_by_symbol_name_without_fk_error(self):
        PatternInstance.objects.create(
            client=self.client_tenant,
            pattern=self.pattern,
            symbol=self.symbol,
            timeframe="1h",
            start_ts=timezone.now() - timedelta(hours=1),
            end_ts=timezone.now(),
            status="FORMING",
            features={"evidence": {"candle_high": "101.5"}, "confidence": 0.5},
        )
        use_case = PatternScanUseCase(
            candle_provider=MagicMock(),
            pattern_repository=MagicMock(),
        )
        command = SimpleNamespace(symbol="BTCUSDT", timeframe="1h", detectors=[])

        checked, confirmed, alerts_created, alerts_existing = use_case._check_confirmations(
            command=command,
            window=[],
            events=[],
        )

        self.assertEqual((checked, confirmed, alerts_created, alerts_existing), (1, 0, 0, 0))

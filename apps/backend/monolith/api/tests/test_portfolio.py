"""Tests for portfolio and market endpoints."""

from decimal import Decimal
from unittest.mock import patch

from django.contrib.auth import get_user_model
from django.test import TestCase, override_settings
from django.utils import timezone
from rest_framework.test import APIClient

from clients.models import Client
from api.models import Symbol, Strategy, Order, Operation


class PortfolioEndpointsTestCase(TestCase):
    """Test portfolio positions and market price endpoints."""

    def setUp(self):
        self.client_record = Client.objects.create(
            name="Test Client",
            email="client@example.com",
        )
        user_model = get_user_model()
        self.user = user_model.objects.create_user(
            username="tester",
            password="password",
            client=self.client_record,
        )
        self.symbol = Symbol.objects.create(
            client=self.client_record,
            name="BTCUSDC",
            description="Bitcoin/USDC pair",
            base_asset="BTC",
            quote_asset="USDC",
        )
        self.strategy = Strategy.objects.create(
            client=self.client_record,
            name="Test Strategy",
            description="Strategy for tests",
            config={},
            risk_config={},
        )
        self.order = Order.objects.create(
            client=self.client_record,
            symbol=self.symbol,
            strategy=self.strategy,
            side="BUY",
            order_type="MARKET",
            quantity=Decimal("2"),
            price=Decimal("100"),
            status="FILLED",
            filled_quantity=Decimal("2"),
            avg_fill_price=Decimal("100"),
            filled_at=timezone.now(),
        )
        self.operation = Operation.objects.create(
            client=self.client_record,
            strategy=self.strategy,
            symbol=self.symbol,
            side="BUY",
            status="ACTIVE",
            stop_loss_percent=Decimal("2"),
            stop_gain_percent=Decimal("4"),
        )
        self.operation.entry_orders.add(self.order)

        self.api_client = APIClient()
        self.api_client.force_authenticate(user=self.user)

    @patch("api.views.portfolio.get_cached_bid")
    def test_positions_endpoint_returns_calculated_fields(self, mock_bid):
        mock_bid.return_value = Decimal("110")

        response = self.api_client.get("/api/portfolio/positions/")

        self.assertEqual(response.status_code, 200)
        payload = response.json()
        self.assertIn("positions", payload)
        self.assertEqual(len(payload["positions"]), 1)

        position = payload["positions"][0]
        self.assertEqual(position["symbol"], "BTCUSDC")
        self.assertEqual(position["side"], "BUY")
        self.assertEqual(position["quantity"], "2.00000000")
        self.assertEqual(position["entry_price"], "100.00")
        self.assertEqual(position["current_price"], "110.00")
        self.assertEqual(position["unrealized_pnl"], "20.00")
        self.assertEqual(position["unrealized_pnl_percent"], "10.00")
        self.assertEqual(position["stop_loss"], "98.00")
        self.assertEqual(position["take_profit"], "104.00")
        self.assertEqual(position["distance_to_stop_percent"], "-10.91")
        self.assertEqual(position["distance_to_target_percent"], "-5.45")
        self.assertEqual(position["status"], "OPEN")

    @patch("api.views.portfolio.get_cached_bid")
    def test_positions_endpoint_filters_by_client(self, mock_bid):
        other_client = Client.objects.create(
            name="Other Client",
            email="other@example.com",
        )
        other_symbol = Symbol.objects.create(
            client=other_client,
            name="ETHUSDC",
            description="Ethereum/USDC pair",
            base_asset="ETH",
            quote_asset="USDC",
        )
        other_strategy = Strategy.objects.create(
            client=other_client,
            name="Other Strategy",
            description="Other strategy",
            config={},
            risk_config={},
        )
        other_order = Order.objects.create(
            client=other_client,
            symbol=other_symbol,
            strategy=other_strategy,
            side="BUY",
            order_type="MARKET",
            quantity=Decimal("1"),
            price=Decimal("50"),
            status="FILLED",
            filled_quantity=Decimal("1"),
            avg_fill_price=Decimal("50"),
            filled_at=timezone.now(),
        )
        other_operation = Operation.objects.create(
            client=other_client,
            strategy=other_strategy,
            symbol=other_symbol,
            side="BUY",
            status="ACTIVE",
        )
        other_operation.entry_orders.add(other_order)

        mock_bid.return_value = Decimal("110")

        response = self.api_client.get("/api/portfolio/positions/")

        self.assertEqual(response.status_code, 200)
        payload = response.json()
        self.assertEqual(len(payload["positions"]), 1)
        self.assertEqual(payload["positions"][0]["symbol"], "BTCUSDC")

    @patch("api.views.market_views.get_cached_quotes")
    def test_market_price_endpoint(self, mock_quotes):
        mock_quotes.return_value = {
            "bid": Decimal("100"),
            "ask": Decimal("101"),
            "timestamp": 1700000000,
        }

        response = self.api_client.get("/api/market/price/BTCUSDC/")

        self.assertEqual(response.status_code, 200)
        payload = response.json()
        self.assertEqual(payload["symbol"], "BTCUSDC")
        self.assertEqual(payload["bid"], "100.00")
        self.assertEqual(payload["ask"], "101.00")
        self.assertEqual(payload["last"], "100.50")
        self.assertEqual(payload["timestamp"], 1700000000)
        self.assertEqual(payload["source"], "binance")

    @override_settings(
        CACHES={
            "default": {
                "BACKEND": "django.core.cache.backends.locmem.LocMemCache",
                "LOCATION": "test-cache",
                "TIMEOUT": 1,
            }
        }
    )
    @patch("api.views.market_views.get_cached_quotes")
    def test_market_price_endpoint_uses_cache(self, mock_quotes):
        mock_quotes.return_value = {
            "bid": Decimal("100"),
            "ask": Decimal("101"),
            "timestamp": 1700000000,
        }

        response1 = self.api_client.get("/api/market/price/BTCUSDC/")
        response2 = self.api_client.get("/api/market/price/BTCUSDC/")

        self.assertEqual(response1.status_code, 200)
        self.assertEqual(response2.status_code, 200)
        self.assertEqual(mock_quotes.call_count, 1)

    @patch("api.views.portfolio.get_cached_bid")
    def test_positions_endpoint_handles_short_pnl(self, mock_bid):
        sell_order = Order.objects.create(
            client=self.client_record,
            symbol=self.symbol,
            strategy=self.strategy,
            side="SELL",
            order_type="MARKET",
            quantity=Decimal("1"),
            price=Decimal("100"),
            status="FILLED",
            filled_quantity=Decimal("1"),
            avg_fill_price=Decimal("100"),
            filled_at=timezone.now(),
        )
        sell_operation = Operation.objects.create(
            client=self.client_record,
            strategy=self.strategy,
            symbol=self.symbol,
            side="SELL",
            status="ACTIVE",
        )
        sell_operation.entry_orders.add(sell_order)

        mock_bid.return_value = Decimal("110")

        response = self.api_client.get("/api/portfolio/positions/")

        self.assertEqual(response.status_code, 200)
        payload = response.json()
        sell_positions = [pos for pos in payload["positions"] if pos["side"] == "SELL"]
        self.assertEqual(len(sell_positions), 1)
        self.assertEqual(sell_positions[0]["unrealized_pnl"], "-10.00")

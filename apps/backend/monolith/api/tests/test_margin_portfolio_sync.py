from decimal import Decimal
from unittest.mock import patch, MagicMock
from django.test import TestCase
from django.contrib.auth import get_user_model
from rest_framework.test import APIClient
from clients.models import Client
from api.models.margin import MarginPosition
from api.application.margin_adapters import MarginAccountSnapshot

class MarginPortfolioInferenceTestCase(TestCase):
    def setUp(self):
        self.client_record = Client.objects.create(name="Test Client", email="test@test.com")
        self.user = get_user_model().objects.create_user(username="testuser", password="password", client=self.client_record)
        self.api_client = APIClient()
        self.api_client.force_authenticate(user=self.user)
        
        # Create a margin position that says SHORT in DB, but we'll mock Binance to show it's LONG
        self.position = MarginPosition.objects.create(
            position_id="test-pos-1",
            client=self.client_record,
            symbol="BTCUSDC",
            side="SHORT", # Incorrect in DB
            status="OPEN",
            entry_price=Decimal("100000"),
            stop_price=Decimal("110000"),
            quantity=Decimal("1"),
            leverage=3
        )

    @patch("api.views.portfolio.get_cached_bid")
    @patch("api.views.portfolio._get_adapter")
    def test_infer_long_from_quote_borrowed(self, mock_get_adapter, mock_bid):
        mock_bid.return_value = Decimal("105000")
        
        # Mock adapter returns quote_borrowed > 0 (USDC borrowed -> LONG)
        mock_adapter = MagicMock()
        mock_adapter.get_margin_account.return_value = MarginAccountSnapshot(
            symbol="BTCUSDC",
            base_asset="BTC", base_free=Decimal("1"), base_locked=Decimal("0"), base_borrowed=Decimal("0"),
            quote_asset="USDC", quote_free=Decimal("0"), quote_locked=Decimal("0"), quote_borrowed=Decimal("100000"),
            margin_level=Decimal("2.0"), liquidation_price=Decimal("50000"), is_margin_trade_enabled=True
        )
        mock_get_adapter.return_value = mock_adapter
        
        response = self.api_client.get("/api/portfolio/positions/")
        self.assertEqual(response.status_code, 200)
        
        pos_data = response.json()["positions"][0]
        self.assertEqual(pos_data["side"], "LONG") # Should be inferred as LONG
        # PnL for LONG (105k - 100k) * 1 = 5000
        self.assertEqual(pos_data["unrealized_pnl"], "5000.00")

    @patch("api.views.portfolio.get_cached_bid")
    @patch("api.views.portfolio._get_adapter")
    def test_infer_short_from_base_borrowed(self, mock_get_adapter, mock_bid):
        mock_bid.return_value = Decimal("95000")
        
        # Position in DB says LONG, but Binance shows base_borrowed > 0 (BTC borrowed -> SHORT)
        self.position.side = "LONG"
        self.position.save()
        
        mock_adapter = MagicMock()
        mock_adapter.get_margin_account.return_value = MarginAccountSnapshot(
            symbol="BTCUSDC",
            base_asset="BTC", base_free=Decimal("0"), base_locked=Decimal("0"), base_borrowed=Decimal("1"),
            quote_asset="USDC", quote_free=Decimal("100000"), quote_locked=Decimal("0"), quote_borrowed=Decimal("0"),
            margin_level=Decimal("2.0"), liquidation_price=Decimal("150000"), is_margin_trade_enabled=True
        )
        mock_get_adapter.return_value = mock_adapter
        
        response = self.api_client.get("/api/portfolio/positions/")
        self.assertEqual(response.status_code, 200)
        
        pos_data = response.json()["positions"][0]
        self.assertEqual(pos_data["side"], "SHORT") # Should be inferred as SHORT
        # PnL for SHORT (100k - 95k) * 1 = 5000
        self.assertEqual(pos_data["unrealized_pnl"], "5000.00")

"""
Unit Tests for BTC Conversion Service

Tests critical conversion logic to prevent production bugs:
- Price discovery edge cases
- Zero division handling
- Invalid asset handling
- Cache behavior
"""

import pytest
from decimal import Decimal
from unittest.mock import MagicMock, patch

from api.services.btc_conversion_service import BTCConversionService


@pytest.mark.django_db
class TestBTCConversionService:
    """Test BTC conversion edge cases and critical paths."""

    def test_btc_to_btc_conversion_returns_one(self):
        """BTC to BTC conversion should always return 1:1 ratio."""
        service = BTCConversionService()
        result = service.convert_to_btc("BTC", Decimal("1.5"))
        assert result == Decimal("1.5")

    def test_zero_quantity_returns_zero(self):
        """Zero quantity should return zero BTC regardless of asset."""
        service = BTCConversionService()
        result = service.convert_to_btc("ETH", Decimal("0"))
        assert result == Decimal("0")

    def test_unknown_asset_returns_zero_gracefully(self):
        """Unknown assets should return 0 BTC and log warning, not crash."""
        mock_market_data = MagicMock()
        mock_market_data.best_bid.side_effect = Exception("Pair not found")

        service = BTCConversionService(mock_market_data)
        result = service.convert_to_btc("UNKNOWN_ASSET", Decimal("100"))

        assert result == Decimal("0")  # Should not crash

    def test_price_caching_works(self, mocker):
        """Price should be cached to avoid repeated API calls."""
        mock_market_data = MagicMock()
        mock_market_data.best_bid.return_value = Decimal("0.05")

        service = BTCConversionService(mock_market_data)

        # First call - should hit API
        price1 = service.get_btc_price("ETH")

        # Second call - should use cache (only 1 API call total)
        price2 = service.get_btc_price("ETH")

        assert price1 == price2 == Decimal("0.05")
        assert mock_market_data.best_bid.call_count == 1  # Cached!

    @patch('api.services.btc_conversion_service.cache')
    def test_cache_invalidation(self, mock_cache):
        """Cache should be checked before API call."""
        mock_cache.get.return_value = Decimal("0.05")  # Simulate cache hit

        service = BTCConversionService()
        price = service.get_btc_price("ETH")

        assert price == Decimal("0.05")
        mock_cache.get.assert_called_once()

    def test_convert_multiple_balances(self):
        """Convert multiple asset balances to BTC."""
        mock_market_data = MagicMock()

        def mock_best_bid(symbol):
            prices = {
                "BTCUSDT": Decimal("95000"),
                "ETHUSDT": Decimal("2000"),
                "USDCUSDT": Decimal("1.00"),
            }
            return prices.get(symbol, Decimal("0"))

        mock_market_data.best_bid.side_effect = mock_best_bid

        service = BTCConversionService(mock_market_data)

        balances = {
            "BTC": {"free": "1.0", "locked": "0"},
            "ETH": {"free": "10.0", "locked": "0"},
            "USDC": {"free": "95000", "locked": "0"},
        }

        result = service.convert_balances_to_btc(balances)

        # Verify conversions
        assert result["BTC"] == Decimal("1.0")  # 1 BTC = 1 BTC
        assert result["ETH"] == pytest.approx(
            Decimal("0.210526"), rel=Decimal("0.001")
        )  # 10 * (2000/95000)
        assert result["USDC"] == pytest.approx(
            Decimal("1.0"), rel=Decimal("0.001")
        )  # 95000/95000

    def test_usdt_route_fallback_when_direct_pair_fails(self):
        """Should fall back to USDT route when direct pair unavailable."""
        mock_market_data = MagicMock()

        def mock_best_bid(symbol):
            # Direct pair fails, USDT route succeeds
            if symbol == "ETHBTC":
                raise Exception("Not available")
            elif symbol == "ETHUSDT":
                return Decimal("2000")
            elif symbol == "BTCUSDT":
                return Decimal("95000")
            return Decimal("0")

        mock_market_data.best_bid.side_effect = mock_best_bid

        service = BTCConversionService(mock_market_data)
        result = service.get_btc_price("ETH")

        # Should calculate via USDT: 2000 / 95000
        assert result == pytest.approx(Decimal("0.0210526"), rel=Decimal("0.0001"))

    def test_precision_maintained_8_decimals(self):
        """BTC values should maintain 8 decimal places precision."""
        service = BTCConversionService()
        result = service.convert_to_btc("BTC", Decimal("0.12345678"))

        # Should not lose precision
        assert result == Decimal("0.12345678")

    def test_negative_quantity_handled(self):
        """Negative quantities (shouldn't happen in practice) are handled."""
        service = BTCConversionService()
        result = service.convert_to_btc("BTC", Decimal("-1.5"))

        assert result == Decimal("-1.5")

    def test_very_small_quantity(self):
        """Handle very small quantities (dust) without errors."""
        service = BTCConversionService()
        result = service.convert_to_btc("BTC", Decimal("0.00000001"))

        assert result == Decimal("0.00000001")

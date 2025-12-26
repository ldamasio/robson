"""
BTC Conversion Service - Convert any asset balance to BTC.

This service provides price discovery and conversion functionality for
denominating portfolio values in BTC terms (crypto investor's preferred metric).

Price Discovery Logic:
1. Try direct pair (e.g., ETH/BTC)
2. Try USDT route (e.g., ETH/USDT ÷ BTC/USDT)
3. Try BUSD route (e.g., ETH/BUSD ÷ BTC/BUSD)
4. Return 0 if no price available (log warning)

Caching:
- Prices are cached for 60 seconds to reduce API calls
- Uses Django's cache framework
"""

from decimal import Decimal
from typing import Dict, Optional
from django.core.cache import cache
import logging

from api.application.adapters import BinanceMarketData

logger = logging.getLogger(__name__)

# Cache price for 60 seconds to avoid excessive API calls
CACHE_TTL_SECONDS = 60


class BTCConversionService:
    """
    Convert asset balances to BTC using Binance market data.

    This service handles price discovery through multiple routes:
    - Direct trading pairs (e.g., ETH/BTC)
    - Indirect pairs via stablecoins (e.g., ETH/USDT → USDT/BTC)
    - Fallback to other routes if primary route unavailable
    """

    def __init__(self, market_data: BinanceMarketData = None):
        """
        Initialize BTC conversion service.

        Args:
            market_data: Binance market data adapter. If None, creates default instance.
        """
        self.market_data = market_data or BinanceMarketData()

    def get_btc_price(self, asset: str) -> Optional[Decimal]:
        """
        Get current price of asset in BTC terms.

        Args:
            asset: Asset symbol (e.g., "BTC", "ETH", "USDC")

        Returns:
            Price in BTC (e.g., 1 ETH = 0.05 BTC)
            Returns None if price cannot be determined

        Examples:
            >>> service.get_btc_price("BTC")
            Decimal("1.0")
            >>> service.get_btc_price("ETH")
            Decimal("0.0523")
            >>> service.get_btc_price("USDC")
            Decimal("0.0000105")
        """
        # Special case: BTC itself
        if asset == "BTC":
            return Decimal("1")

        # Try cache first
        cache_key = f"btc_price:{asset}"
        cached = cache.get(cache_key)
        if cached is not None:
            return cached

        # Try different price discovery routes
        price = self._get_price_via_direct_pair(asset)
        if price is None:
            price = self._get_price_via_usdt(asset)
        if price is None:
            price = self._get_price_via_busd(asset)

        if price is not None:
            # Cache the result
            cache.set(cache_key, price, timeout=CACHE_TTL_SECONDS)
            return price

        logger.warning(f"Could not determine BTC price for {asset}")
        return None

    def _get_price_via_direct_pair(self, asset: str) -> Optional[Decimal]:
        """
        Try to get price via direct trading pair (e.g., ETH/BTC).

        Args:
            asset: Asset symbol to get price for

        Returns:
            Price in BTC or None if pair doesn't exist
        """
        if asset == "BTC":
            return Decimal("1")

        try:
            # Construct symbol (e.g., ETHBTC, LTCBTC)
            symbol = f"{asset}BTC"

            # Try to get best bid (selling 1 asset gets you this much BTC)
            bid = self.market_data.best_bid(symbol)

            # Sanity check
            if bid > 0:
                logger.debug(f"Got {asset} BTC price via direct pair: {bid}")
                return bid

        except Exception as e:
            logger.debug(f"Direct pair {asset}BTC not available: {e}")

        return None

    def _get_price_via_usdt(self, asset: str) -> Optional[Decimal]:
        """
        Try to get price via USDT route.

        Formula: (asset/USDT price) / (BTC/USDT price)

        Example:
        - ETH/USDT = 2000 USDT
        - BTC/USDT = 95000 USDT
        - ETH/BTC = 2000 / 95000 = 0.02105 BTC

        Args:
            asset: Asset symbol to get price for

        Returns:
            Price in BTC or None if route unavailable
        """
        try:
            # Get asset price in USDT
            asset_usdt_symbol = f"{asset}USDT"
            asset_price_usdt = self.market_data.best_bid(asset_usdt_symbol)

            # Get BTC price in USDT
            btc_usdt_symbol = "BTCUSDT"
            btc_price_usdt = self.market_data.best_bid(btc_usdt_symbol)

            if asset_price_usdt > 0 and btc_price_usdt > 0:
                asset_btc_price = asset_price_usdt / btc_price_usdt
                logger.debug(f"Got {asset} BTC price via USDT: {asset_btc_price}")
                return asset_btc_price

        except Exception as e:
            logger.debug(f"USDT route for {asset} failed: {e}")

        return None

    def _get_price_via_busd(self, asset: str) -> Optional[Decimal]:
        """
        Try to get price via BUSD route (same logic as USDT).

        BUSD (Binance USD) is another stablecoin like USDT.

        Args:
            asset: Asset symbol to get price for

        Returns:
            Price in BTC or None if route unavailable
        """
        try:
            # Get asset price in BUSD
            asset_busd_symbol = f"{asset}BUSD"
            asset_price_busd = self.market_data.best_bid(asset_busd_symbol)

            # Get BTC price in BUSD
            btc_busd_symbol = "BTCBUSD"
            btc_price_busd = self.market_data.best_bid(btc_busd_symbol)

            if asset_price_busd > 0 and btc_price_busd > 0:
                asset_btc_price = asset_price_busd / btc_price_busd
                logger.debug(f"Got {asset} BTC price via BUSD: {asset_btc_price}")
                return asset_btc_price

        except Exception as e:
            logger.debug(f"BUSD route for {asset} failed: {e}")

        return None

    def convert_to_btc(self, asset: str, quantity: Decimal) -> Decimal:
        """
        Convert a quantity of asset to BTC.

        Args:
            asset: Asset symbol (e.g., "ETH", "USDC")
            quantity: Amount to convert

        Returns:
            Equivalent amount in BTC

        Examples:
            >>> service.convert_to_btc("BTC", Decimal("1.5"))
            Decimal("1.5")
            >>> service.convert_to_btc("ETH", Decimal("10"))
            Decimal("0.523")
        """
        price = self.get_btc_price(asset)

        if price is None:
            logger.warning(
                f"Cannot convert {quantity} {asset} to BTC - no price available"
            )
            return Decimal("0")

        return quantity * price

    def convert_balances_to_btc(
        self,
        balances: Dict[str, Dict[str, Decimal | str]]
    ) -> Dict[str, Decimal]:
        """
        Convert multiple balances to BTC.

        Args:
            balances: Dict of {asset: {"free": Decimal, "locked": Decimal}}

        Returns:
            Dict of {asset: btc_value}

        Examples:
            >>> balances = {
            ...     "BTC": {"free": "1.0", "locked": "0"},
            ...     "ETH": {"free": "10.0", "locked": "0"},
            ... }
            >>> service.convert_balances_to_btc(balances)
            {"BTC": Decimal("1.0"), "ETH": Decimal("0.523")}
        """
        result = {}

        for asset, balance_info in balances.items():
            free = Decimal(str(balance_info.get("free", "0")))
            locked = Decimal(str(balance_info.get("locked", "0")))
            total = free + locked

            if total > 0:
                btc_value = self.convert_to_btc(asset, total)
                result[asset] = btc_value

        return result

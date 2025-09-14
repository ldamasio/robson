from __future__ import annotations
from decimal import Decimal
from core.application.ports import MarketDataPort, ExchangeExecutionPort

from django.conf import settings
from binance.client import Client


class BinanceMarketData(MarketDataPort):
    def __init__(self, client: Client | None = None):
        self.client = client or Client(settings.BINANCE_API_KEY_TEST, settings.BINANCE_SECRET_KEY_TEST, testnet=True)

    def best_bid(self, symbol: object) -> Decimal:
        pair = getattr(symbol, "as_pair", lambda: str(symbol))()
        ob = self.client.get_order_book(symbol=pair, limit=5)
        bid = ob["bids"][0][0]
        return Decimal(str(bid))

    def best_ask(self, symbol: object) -> Decimal:
        pair = getattr(symbol, "as_pair", lambda: str(symbol))()
        ob = self.client.get_order_book(symbol=pair, limit=5)
        ask = ob["asks"][0][0]
        return Decimal(str(ask))


class StubExecution(ExchangeExecutionPort):
    """Execution stub that returns a synthetic id (no external call)."""

    def place_limit(self, order: object) -> str:
        # do not hit external exchange here
        return f"ext_{order['id']}"


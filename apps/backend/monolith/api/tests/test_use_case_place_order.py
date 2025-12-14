from __future__ import annotations
import unittest
from decimal import Decimal
from datetime import datetime, timezone

from api.application import PlaceOrderUseCase, Symbol as DomainSymbol


class FakeRepo:
    def __init__(self):
        self.saved: list[dict] = []

    def save(self, order: dict) -> None:
        self.saved.append(order)

    def find_by_id(self, oid: str):
        return None

    def list_recent(self, since: datetime):
        return []


class FakeMarketData:
    def __init__(self, bid: Decimal, ask: Decimal):
        self._bid = Decimal(bid)
        self._ask = Decimal(ask)
        self.best_bid_called = 0
        self.best_ask_called = 0

    def best_bid(self, symbol) -> Decimal:
        self.best_bid_called += 1
        return self._bid

    def best_ask(self, symbol) -> Decimal:
        self.best_ask_called += 1
        return self._ask


class FakeExchange:
    def place_limit(self, order: dict) -> str:
        return f"ext_{order['id']}"


class FakeBus:
    def __init__(self):
        self.published: list[tuple[str, dict]] = []

    def publish(self, topic: str, payload: dict) -> None:
        self.published.append((topic, payload))


class FakeClock:
    def __init__(self, fixed: datetime):
        self._fixed = fixed

    def now(self) -> datetime:
        return self._fixed


class PlaceOrderUseCaseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.repo = FakeRepo()
        self.md = FakeMarketData(bid=Decimal("99"), ask=Decimal("101"))
        self.ex = FakeExchange()
        self.bus = FakeBus()
        self.clock = FakeClock(datetime(2024, 1, 1, 0, 0, tzinfo=timezone.utc))
        self.uc = PlaceOrderUseCase(self.repo, self.md, self.ex, self.bus, self.clock)
        self.symbol = DomainSymbol("BTC", "USDT")

    def test_buy_without_limit_uses_best_ask(self):
        order = self.uc.execute(self.symbol, "BUY", Decimal("0.1"), None)
        self.assertEqual(self.md.best_ask_called, 1)
        self.assertEqual(self.md.best_bid_called, 0)
        # saved order uses ext id
        self.assertTrue(self.repo.saved)
        saved = self.repo.saved[-1]
        self.assertEqual(saved["price"], Decimal("101"))

    def test_sell_without_limit_uses_best_bid(self):
        order = self.uc.execute(self.symbol, "SELL", Decimal("0.1"), None)
        self.assertEqual(self.md.best_bid_called, 1)
        self.assertEqual(self.md.best_ask_called, 0)
        saved = self.repo.saved[-1]
        self.assertEqual(saved["price"], Decimal("99"))

    def test_with_limit_price_does_not_call_marketdata(self):
        self.uc.execute(self.symbol, "BUY", Decimal("0.1"), Decimal("50000"))
        self.assertEqual(self.md.best_bid_called + self.md.best_ask_called, 0)
        saved = self.repo.saved[-1]
        self.assertEqual(saved["price"], Decimal("50000"))

    def test_publishes_event(self):
        self.uc.execute(self.symbol, "BUY", Decimal("0.1"), Decimal("50000"))
        self.assertTrue(self.bus.published)
        topic, payload = self.bus.published[-1]
        self.assertEqual(topic, "orders.placed")
        self.assertEqual(payload["symbol"], "BTCUSDT")
        self.assertEqual(payload["side"], "BUY")
        self.assertEqual(payload["qty"], "0.1")
        self.assertEqual(payload["price"], "50000")


if __name__ == "__main__":
    unittest.main()


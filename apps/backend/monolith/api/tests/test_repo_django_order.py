from __future__ import annotations
from decimal import Decimal
from datetime import datetime, timedelta, timezone

from django.test import TestCase

from api.application import DjangoOrderRepository, Symbol as DomainSymbol
from api.models import Order as DjangoOrder, Symbol as DjangoSymbol


class DjangoOrderRepositoryTests(TestCase):
    def setUp(self) -> None:
        self.repo = DjangoOrderRepository()

    def test_save_creates_symbol_if_missing_and_persists_order(self):
        # ensure empty DB
        self.assertEqual(DjangoOrder.objects.count(), 0)
        self.assertEqual(DjangoSymbol.objects.count(), 0)

        order = {
            "id": "ord_1",
            "symbol": DomainSymbol("BTC", "USDT"),
            "side": "BUY",
            "qty": Decimal("0.1"),
            "price": Decimal("50000"),
            "created_at": datetime.now(tz=timezone.utc),
        }
        self.repo.save(order)

        # symbol was created
        self.assertEqual(DjangoSymbol.objects.filter(name="BTCUSDT").count(), 1)
        # order persisted
        self.assertEqual(DjangoOrder.objects.count(), 1)
        m = DjangoOrder.objects.first()
        self.assertEqual(m.side, "BUY")
        self.assertEqual(m.quantity, Decimal("0.1"))
        self.assertEqual(m.price, Decimal("50000"))

    def test_list_recent_returns_serialized_dicts(self):
        # seed one order via repo
        now = datetime.now(tz=timezone.utc)
        self.repo.save(
            {
                "id": "ord_2",
                "symbol": DomainSymbol("ETH", "USDT"),
                "side": "SELL",
                "qty": Decimal("1.5"),
                "price": Decimal("2500"),
                "created_at": now,
            }
        )
        since = now - timedelta(days=1)
        items = list(self.repo.list_recent(since))
        self.assertGreaterEqual(len(items), 1)
        item = items[0]
        self.assertEqual(item["symbol"], "ETHUSDT")
        self.assertEqual(item["side"], "SELL")
        self.assertEqual(item["qty"], "1.5")


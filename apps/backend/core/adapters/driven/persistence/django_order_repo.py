from __future__ import annotations
from typing import Optional, Iterable
from datetime import datetime
from decimal import Decimal

from django.utils import timezone

from core.application.ports import OrderRepository


class DjangoOrderRepository(OrderRepository):
    """Order repository backed by Django ORM models.

    Expects a dict-like order with keys: id, symbol(as domain object with as_pair()),
    side, qty, price, created_at.
    """

    def __init__(self):
        from api.models import Order as DjangoOrder, Symbol as DjangoSymbol  # lazy import

        self._Order = DjangoOrder
        self._Symbol = DjangoSymbol

    def _get_symbol(self, pair: str):
        # Try to fetch by name; if missing, create a minimal Symbol
        base = pair[:-4] if len(pair) > 4 else pair
        quote = pair[-4:] if len(pair) > 4 else "USDT"
        sym, _ = self._Symbol.objects.get_or_create(
            name=pair,
            defaults={
                "description": f"Auto-created for {pair}",
                "base_asset": base,
                "quote_asset": quote,
            },
        )
        return sym

    def save(self, order: dict) -> None:
        symbol_pair = getattr(order["symbol"], "as_pair", lambda: str(order["symbol"]))()
        sym = self._get_symbol(symbol_pair)
        self._Order.objects.update_or_create(
            # no external id field in model; use created_at+side+symbol heuristic
            symbol=sym,
            side=order["side"],
            quantity=Decimal(order["qty"]),
            defaults={
                "price": Decimal(order["price"]),
                "order_type": "LIMIT" if order.get("price") else "MARKET",
            },
        )

    def find_by_id(self, oid: str) -> Optional[dict]:
        # Model doesn't have ext id; not implemented in this lightweight adapter
        return None

    def list_recent(self, since: datetime) -> Iterable[dict]:
        qs = self._Order.objects.filter(created_at__gte=since).order_by("-created_at")
        for m in qs:
            yield {
                "id": str(m.pk),
                "symbol": m.symbol.name,
                "side": m.side,
                "qty": str(m.quantity),
                "price": str(m.price or "0"),
                "created_at": m.created_at,
            }


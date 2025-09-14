from __future__ import annotations
from dataclasses import replace
from decimal import Decimal
from datetime import datetime
from .ports import OrderRepository, MarketDataPort, ExchangeExecutionPort, EventBusPort, ClockPort, UnitOfWork


class PlaceOrderUseCase:
    def __init__(
        self,
        repo: OrderRepository,
        md: MarketDataPort,
        ex: ExchangeExecutionPort,
        bus: EventBusPort,
        clock: ClockPort,
        uow: UnitOfWork | None = None,
    ):
        self.repo = repo
        self.md = md
        self.ex = ex
        self.bus = bus
        self.clock = clock
        self.uow = uow

    def execute(self, symbol: object, side: str, qty: Decimal, limit_price: Decimal | None = None) -> object:
        if side not in {"BUY", "SELL"}:
            raise ValueError("invalid side")

        px = limit_price
        if px is None:
            px = self.md.best_ask(symbol) if side == "BUY" else self.md.best_bid(symbol)

        # Order is a domain object; here treated opaquely to avoid coupling
        order = {
            "id": f"ord_{int(self.clock.now().timestamp()*1000)}",
            "symbol": symbol,
            "side": side,
            "qty": qty,
            "price": px,
            "created_at": self.clock.now(),
        }

        ctx = self.uow if self.uow is not None else _NullUoW()
        with ctx:
            ext_id = self.ex.place_limit(order)
            persisted = dict(order, id=ext_id)
            self.repo.save(persisted)
            self.bus.publish(
                "orders.placed",
                {
                    "id": persisted["id"],
                    "symbol": getattr(symbol, "as_pair", lambda: str(symbol))(),
                    "side": side,
                    "qty": str(qty),
                    "price": str(px),
                },
            )
            ctx.commit()
        return persisted


class _NullUoW:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

    def commit(self) -> None:
        pass


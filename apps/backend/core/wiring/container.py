"""Composition root for dependency injection.

Factories here assemble use cases with concrete adapters.
"""

from __future__ import annotations

from core.application.place_order import PlaceOrderUseCase
from core.adapters.driven.persistence.django_order_repo import DjangoOrderRepository
from core.adapters.driven.external.binance_client import BinanceMarketData, StubExecution
from core.adapters.driven.messaging.noop_bus import NoopEventBus
from core.adapters.driven.time.clock import RealClock


_singletons: dict[str, object] = {}


def get_singleton(key: str, factory):
    if key not in _singletons:
        _singletons[key] = factory()
    return _singletons[key]


def get_place_order_uc() -> PlaceOrderUseCase:
    def factory():
        repo = DjangoOrderRepository()
        md = BinanceMarketData()
        ex = StubExecution()
        bus = NoopEventBus()
        clock = RealClock()
        return PlaceOrderUseCase(repo, md, ex, bus, clock)

    return get_singleton("place_order_uc", factory)

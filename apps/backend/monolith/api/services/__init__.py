"""api/services package exports.

Provides service-layer helpers and re-exports for convenient patching in tests.
"""

# Re-export Binance SDK Client so tests can patch `api.services.Client` directly
try:
    from binance.client import Client  # noqa: F401
except Exception:  # pragma: no cover - optional dependency in some contexts
    Client = None  # type: ignore

from .binance_service import BinanceService
from .market_data_service import MarketDataService
from .portfolio_service import PortfolioService

__all__ = [
    "Client",
    "BinanceService",
    "MarketDataService",
    "PortfolioService",
]

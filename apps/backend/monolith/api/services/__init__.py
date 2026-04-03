"""api/services package exports.

Provides service-layer helpers and re-exports for convenient patching in tests.
"""

# Re-export Binance SDK Client so tests can patch `api.services.Client` directly
try:
    from binance.client import Client  # noqa: F401
except Exception:  # pragma: no cover - optional dependency in some contexts
    Client = None  # type: ignore

from .binance_service import (
    BinanceRuntimeConfig,
    BinanceService,
    get_binance_credentials,
    get_binance_runtime_config,
    has_binance_credentials,
)
from .market_data_service import MarketDataService
from .portfolio_service import PortfolioService
from .derivatives_data_service import DerivativesDataService

__all__ = [
    "Client",
    "BinanceRuntimeConfig",
    "BinanceService",
    "get_binance_credentials",
    "get_binance_runtime_config",
    "has_binance_credentials",
    "MarketDataService",
    "PortfolioService",
    "DerivativesDataService",
]

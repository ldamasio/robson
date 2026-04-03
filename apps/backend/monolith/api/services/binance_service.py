"""Binance service wrapper used by the app and tests.

Imports `Client` from the package root so tests can patch `api.services.Client`.
Implements a simple singleton to share the underlying client.

Multi-tenant aware: Can use system credentials (from settings/K8s secrets)
or per-client credentials (from database).
"""

# api/services/binance_service.py
from dataclasses import dataclass
from importlib import import_module
from django.conf import settings
import logging

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class BinanceRuntimeConfig:
    """Resolved Binance runtime configuration for one execution context."""

    api_key: str
    secret_key: str
    use_testnet: bool
    environment: str
    mode: str
    api_url: str

    @property
    def has_credentials(self) -> bool:
        return bool(self.api_key and self.secret_key)


def get_binance_runtime_config(use_testnet: bool = None) -> BinanceRuntimeConfig:
    """
    Resolve the effective Binance runtime configuration.

    Args:
        use_testnet: Optional override. When omitted, uses the canonical
            settings-derived environment.
    """
    default_use_testnet = getattr(settings, "BINANCE_USE_TESTNET", True)
    resolved_use_testnet = (
        default_use_testnet if use_testnet is None else bool(use_testnet)
    )

    if resolved_use_testnet == default_use_testnet:
        api_key = getattr(settings, "BINANCE_API_KEY_ACTIVE", "")
        secret_key = getattr(settings, "BINANCE_SECRET_KEY_ACTIVE", "")
        api_url = getattr(settings, "BINANCE_API_URL_ACTIVE", "")
        environment = getattr(
            settings,
            "BINANCE_ENV",
            "testnet" if resolved_use_testnet else "production",
        )
        mode = getattr(
            settings,
            "BINANCE_MODE",
            "TESTNET" if resolved_use_testnet else "PRODUCTION",
        )
    else:
        environment = "testnet" if resolved_use_testnet else "production"
        mode = "TESTNET" if resolved_use_testnet else "PRODUCTION"
        api_key = (
            getattr(settings, "BINANCE_API_KEY_TEST", "")
            if resolved_use_testnet
            else getattr(settings, "BINANCE_API_KEY", "")
        )
        secret_key = (
            getattr(settings, "BINANCE_SECRET_KEY_TEST", "")
            if resolved_use_testnet
            else getattr(settings, "BINANCE_SECRET_KEY", "")
        )
        api_url = (
            getattr(settings, "BINANCE_API_URL_TEST", "")
            if resolved_use_testnet
            else getattr(settings, "BINANCE_API_URL_PROD", "")
        )

    return BinanceRuntimeConfig(
        api_key=api_key,
        secret_key=secret_key,
        use_testnet=resolved_use_testnet,
        environment=environment,
        mode=mode,
        api_url=api_url,
    )


def get_binance_credentials(use_testnet: bool = None) -> tuple[str, str, bool]:
    """
    Get Binance API credentials based on configuration.
    
    Args:
        use_testnet: Override setting. If None, uses settings.BINANCE_USE_TESTNET
        
    Returns:
        Tuple of (api_key, secret_key, is_testnet)
    """
    runtime = get_binance_runtime_config(use_testnet)
    return runtime.api_key, runtime.secret_key, runtime.use_testnet


def has_binance_credentials(use_testnet: bool = None) -> bool:
    """Return whether the effective runtime configuration has credentials."""
    return get_binance_runtime_config(use_testnet).has_credentials


class BinanceService:
    """
    Service for interacting with the Binance API (singleton for system credentials).
    
    Can operate in two modes:
    1. System mode: Uses credentials from K8s secrets (admin/operator credentials)
    2. Client mode: Uses per-client credentials from database (future multi-tenant)
    
    For production trading, set BINANCE_USE_TESTNET=False in environment.
    """

    _instance = None
    _client = None
    _current_testnet_mode = None

    def __new__(cls, *args, **kwargs):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance
    
    def __init__(self, use_testnet: bool = None):
        """
        Initialize BinanceService.
        
        Args:
            use_testnet: Override testnet setting. If None, uses settings.BINANCE_USE_TESTNET
        """
        # Determine mode from settings if not explicitly provided
        runtime = get_binance_runtime_config(use_testnet)
        use_testnet = runtime.use_testnet
        
        # If mode changed, we need to reinitialize the client
        if BinanceService._current_testnet_mode is not None and BinanceService._current_testnet_mode != use_testnet:
            logger.warning(f"Binance mode changed from testnet={BinanceService._current_testnet_mode} to testnet={use_testnet}. Reinitializing client.")
            BinanceService._client = None
        
        self.use_testnet = use_testnet
        BinanceService._current_testnet_mode = use_testnet
    
    @property
    def client(self):
        """Lazy-initialize Binance client (uses class-level cache)."""
        if not BinanceService._client:
            runtime = get_binance_runtime_config(self.use_testnet)
            
            if not runtime.has_credentials:
                raise RuntimeError(
                    f"Binance API credentials not configured for {runtime.environment} mode"
                )

            # Resolve Client dynamically to respect test patching of `api.services.Client`
            services_pkg = import_module('api.services')
            client_cls = getattr(services_pkg, 'Client', None)
            if client_cls is None:
                raise RuntimeError('Binance Client not available')
            
            BinanceService._client = client_cls(
                runtime.api_key,
                runtime.secret_key,
                testnet=runtime.use_testnet,
            )
            logger.info(f"Binance client initialized in {runtime.mode} mode")

        return BinanceService._client
    
    @classmethod
    def reset(cls):
        """Reset singleton instance. Useful for testing or mode switching."""
        cls._client = None
        cls._instance = None
        cls._current_testnet_mode = None
    
    @property
    def is_production(self) -> bool:
        """Check if service is running in production mode."""
        return not self.use_testnet
    
    def ping(self):
        """Ping Binance API to test connectivity."""
        try:
            return self.client.ping()
        except Exception as e:
            logger.error(f"Binance ping failed: {e}")
            raise
    
    def get_server_time(self):
        """Get Binance server time."""
        try:
            return self.client.get_server_time()
        except Exception as e:
            logger.error(f"Failed to get server time: {e}")
            raise
    
    def get_account_info(self):
        """Get account information."""
        try:
            return self.client.get_account()
        except Exception as e:
            logger.error(f"Failed to get account info: {e}")
            raise

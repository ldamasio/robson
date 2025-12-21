"""Binance service wrapper used by the app and tests.

Imports `Client` from the package root so tests can patch `api.services.Client`.
Implements a simple singleton to share the underlying client.

Multi-tenant aware: Can use system credentials (from settings/K8s secrets)
or per-client credentials (from database).
"""

# api/services/binance_service.py
from importlib import import_module
from django.conf import settings
from django.core.cache import cache
import logging

logger = logging.getLogger(__name__)


def get_binance_credentials(use_testnet: bool = None) -> tuple[str, str, bool]:
    """
    Get Binance API credentials based on configuration.
    
    Args:
        use_testnet: Override setting. If None, uses settings.BINANCE_USE_TESTNET
        
    Returns:
        Tuple of (api_key, secret_key, is_testnet)
    """
    if use_testnet is None:
        use_testnet = getattr(settings, 'BINANCE_USE_TESTNET', True)
    
    if use_testnet:
        api_key = settings.BINANCE_API_KEY_TEST
        secret_key = settings.BINANCE_SECRET_KEY_TEST
    else:
        api_key = settings.BINANCE_API_KEY
        secret_key = settings.BINANCE_SECRET_KEY
    
    return api_key, secret_key, use_testnet


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
        if use_testnet is None:
            use_testnet = getattr(settings, 'BINANCE_USE_TESTNET', True)
        
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
            api_key, secret_key, is_testnet = get_binance_credentials(self.use_testnet)
            
            if not api_key or not secret_key:
                mode = "testnet" if is_testnet else "production"
                raise RuntimeError(f'Binance API credentials not configured for {mode} mode')

            # Resolve Client dynamically to respect test patching of `api.services.Client`
            services_pkg = import_module('api.services')
            client_cls = getattr(services_pkg, 'Client', None)
            if client_cls is None:
                raise RuntimeError('Binance Client not available')
            
            BinanceService._client = client_cls(api_key, secret_key, testnet=is_testnet)
            
            mode_str = "TESTNET" if is_testnet else "PRODUCTION"
            logger.info(f"Binance client initialized in {mode_str} mode")

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

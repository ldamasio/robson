"""Binance service wrapper used by the app and tests.

Imports `Client` from the package root so tests can patch `api.services.Client`.
Implements a simple singleton to share the underlying client.
"""

# api/services/binance_service.py
from . import Client  # re-exported in api.services.__init__ for patching
from django.conf import settings
from django.core.cache import cache
import logging

logger = logging.getLogger(__name__)

class BinanceService:
    """Service for interacting with the Binance API (singleton)."""

    _instance = None
    _client = None

    def __new__(cls, *args, **kwargs):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance
    
    def __init__(self, use_testnet=True):
        # Allow reconfiguration if explicitly provided, otherwise keep existing
        if getattr(self, "use_testnet", None) is None:
            self.use_testnet = use_testnet
    
    @property
    def client(self):
        """Lazy-initialize Binance client (uses class-level cache)."""
        if not BinanceService._client:
            if self.use_testnet:
                api_key = settings.BINANCE_API_KEY_TEST
                secret_key = settings.BINANCE_SECRET_KEY_TEST
            else:
                api_key = settings.BINANCE_API_KEY
                secret_key = settings.BINANCE_SECRET_KEY

            # Client is imported from api.services for test patching
            BinanceService._client = Client(api_key, secret_key, testnet=self.use_testnet)

        return BinanceService._client
    
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

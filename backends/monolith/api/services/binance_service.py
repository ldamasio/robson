# api/services/binance_service.py
from binance.client import Client
from django.conf import settings
from django.core.cache import cache
import logging

logger = logging.getLogger(__name__)

class BinanceService:
    """Service for interacting with the Binance API."""
    
    def __init__(self, use_testnet=True):
        self.use_testnet = use_testnet
        self._client = None
    
    @property
    def client(self):
        """Lazy-initialize Binance client."""
        if not self._client:
            if self.use_testnet:
                api_key = settings.BINANCE_API_KEY_TEST
                secret_key = settings.BINANCE_SECRET_KEY_TEST
            else:
                api_key = settings.BINANCE_API_KEY
                secret_key = settings.BINANCE_SECRET_KEY
            
            self._client = Client(api_key, secret_key, testnet=self.use_testnet)
        
        return self._client
    
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

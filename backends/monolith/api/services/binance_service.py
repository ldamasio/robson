# api/services/binance_service.py
from binance.client import Client
from django.conf import settings
from django.core.cache import cache
import logging

logger = logging.getLogger(__name__)

class BinanceService:
    """Serviço para interação com a API da Binance"""
    
    def __init__(self, use_testnet=True):
        self.use_testnet = use_testnet
        self._client = None
    
    @property
    def client(self):
        """Lazy loading do cliente Binance"""
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
        """Testa conexão com a Binance"""
        try:
            return self.client.ping()
        except Exception as e:
            logger.error(f"Binance ping failed: {e}")
            raise
    
    def get_server_time(self):
        """Obtém tempo do servidor Binance"""
        try:
            return self.client.get_server_time()
        except Exception as e:
            logger.error(f"Failed to get server time: {e}")
            raise
    
    def get_account_info(self):
        """Obtém informações da conta"""
        try:
            return self.client.get_account()
        except Exception as e:
            logger.error(f"Failed to get account info: {e}")
            raise


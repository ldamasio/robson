# api/services.py - NOVO ARQUIVO (começar simples)
from binance.client import Client
from django.conf import settings
import logging

logger = logging.getLogger(__name__)

class BinanceService:
    """Serviço centralizado para Binance API"""
    
    _instance = None
    _client = None
    
    def __new__(cls):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance
    
    @property
    def client(self):
        """Singleton do cliente Binance"""
        if not self._client:
            self._client = Client(
                settings.BINANCE_API_KEY_TEST, 
                settings.BINANCE_SECRET_KEY_TEST
            )
        return self._client
    
    def ping(self):
        """Testa conexão"""
        try:
            return self.client.ping()
        except Exception as e:
            logger.error(f"Binance ping failed: {e}")
            raise
    
    def get_server_time(self):
        """Tempo do servidor"""
        try:
            return self.client.get_server_time()
        except Exception as e:
            logger.error(f"Server time failed: {e}")
            raise

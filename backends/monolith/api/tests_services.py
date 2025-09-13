# api/tests_services.py - NOVO ARQUIVO
from django.test import TestCase
from unittest.mock import patch, MagicMock
from .services import BinanceService

class TestBinanceService(TestCase):
    """Tests for BinanceService."""
    
    def setUp(self):
        # Reset instance for each test
        BinanceService._instance = None
        BinanceService._client = None
    
    @patch('api.services.Client')
    def test_binance_service_singleton(self, mock_client):
        """Ensure BinanceService behaves as a singleton."""
        service1 = BinanceService()
        service2 = BinanceService()
        
        self.assertIs(service1, service2)
    
    @patch('api.services.Client')
    def test_ping_success(self, mock_client):
        """Ping succeeds when client returns 200-like response."""
        mock_instance = MagicMock()
        mock_instance.ping.return_value = {}
        mock_client.return_value = mock_instance
        
        service = BinanceService()
        result = service.ping()
        
        self.assertEqual(result, {})
        mock_instance.ping.assert_called_once()
    
    @patch('api.services.Client')
    def test_ping_failure(self, mock_client):
        """Ping raises on client error."""
        mock_instance = MagicMock()
        mock_instance.ping.side_effect = Exception("Connection failed")
        mock_client.return_value = mock_instance
        
        service = BinanceService()
        
        with self.assertRaises(Exception):
            service.ping()

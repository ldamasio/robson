# api/tests_services.py - Unit tests for BinanceService
from django.test import TestCase
from unittest.mock import patch, MagicMock
from .services import BinanceService

# Fake credentials for testing
FAKE_CREDENTIALS = ('fake_api_key', 'fake_secret_key', True)


class TestBinanceService(TestCase):
    """Tests for BinanceService."""
    
    def setUp(self):
        # Reset singleton for each test
        BinanceService.reset()
    
    @patch('api.services.binance_service.get_binance_credentials', return_value=FAKE_CREDENTIALS)
    @patch('api.services.Client')
    def test_binance_service_singleton(self, mock_client, mock_creds):
        """Ensure BinanceService behaves as a singleton."""
        service1 = BinanceService()
        service2 = BinanceService()
        
        self.assertIs(service1, service2)
    
    @patch('api.services.binance_service.get_binance_credentials', return_value=FAKE_CREDENTIALS)
    @patch('api.services.Client')
    def test_ping_success(self, mock_client, mock_creds):
        """Ping succeeds when client returns 200-like response."""
        mock_instance = MagicMock()
        mock_instance.ping.return_value = {}
        mock_client.return_value = mock_instance
        
        service = BinanceService()
        result = service.ping()
        
        self.assertEqual(result, {})
        mock_instance.ping.assert_called_once()
    
    @patch('api.services.binance_service.get_binance_credentials', return_value=FAKE_CREDENTIALS)
    @patch('api.services.Client')
    def test_ping_failure(self, mock_client, mock_creds):
        """Ping raises on client error."""
        mock_instance = MagicMock()
        mock_instance.ping.side_effect = Exception("Connection failed")
        mock_client.return_value = mock_instance
        
        service = BinanceService()
        
        with self.assertRaises(Exception):
            service.ping()

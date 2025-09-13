
# api/services/market_data_service.py
import pandas as pd
import datetime
from django.core.cache import cache
from .binance_service import BinanceService

class MarketDataService:
    """Service for market data."""
    
    def __init__(self):
        self.binance = BinanceService()
    
    def get_historical_data(self, symbol, interval, days=7):
        """Get historical data with caching."""
        cache_key = f"historical_{symbol}_{interval}_{days}"
        
        # Try cache first
        cached_data = cache.get(cache_key)
        if cached_data:
            return cached_data
        
        try:
            end_date = datetime.date.today()
            start_date = end_date - datetime.timedelta(days=days)
            
            # Format dates for API
            start_str = start_date.strftime("%Y.%m.%d")
            end_str = end_date.strftime("%Y.%m.%d")
            
            # Fetch Binance data
            klines = self.binance.client.get_historical_klines(
                symbol, interval, start_str, end_str
            )
            
            # Process with pandas
            df = pd.DataFrame(klines)
            df = df.iloc[:, :6]
            df.columns = ["Date", "Open", "High", "Low", "Close", "Volume"]
            df = df.set_index("Date")
            df.index = pd.to_datetime(df.index, unit="ms")
            df = df.astype("float")
            
            # Convert to JSON
            result = df.to_json(orient='records', date_format='iso')
            
            # Cache for 5 minutes
            cache.set(cache_key, result, 300)
            
            return result
            
        except Exception as e:
            logger.error(f"Failed to get historical data: {e}")
            raise

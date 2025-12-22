# api/urls/__init__.py - SEGUINDO O PADRÃO DO PROJETO
"""
Organized URL configuration following the project's established patterns.
Uses the existing strategy_views.py and market_views.py structure.

Includes production trading endpoints for executing real trades.
"""

from django.urls import path, include
from django.http import JsonResponse
from ..views.auth import user_profile, logout, login, token_test, MyTokenObtainPairView
from ..views.strategy_views import get_strategies
from ..views.market_views import ping, server_time, historical_data
from rest_framework_simplejwt.views import (
    TokenRefreshView,
    TokenVerifyView,
    TokenBlacklistView
)

# Import from old views.py for compatibility (until fully migrated)
try:
    from .. import views as old_views
    OLD_VIEWS_AVAILABLE = True
except ImportError:
    OLD_VIEWS_AVAILABLE = False

# Import trading views for production trading
try:
    from ..views.trading import (
        trading_status,
        account_balance as trading_balance,
        buy_btc,
        sell_btc,
        trade_history,
        pnl_summary,
        calculate_position_size,
    )
    TRADING_VIEWS_AVAILABLE = True
except ImportError as e:
    print(f"⚠️  Could not import trading views: {e}")
    TRADING_VIEWS_AVAILABLE = False

# Fallback function for missing views
def fallback_view(request):
    from django.http import JsonResponse
    return JsonResponse({"message": "Endpoint under migration", "status": "maintenance"})

urlpatterns = [
    # ==========================================
    # AUTHENTICATION ROUTES
    # ==========================================
    # Authentication routes (from auth.py)
    path('auth/', include('api.urls.auth')),
    
    # Legacy JWT endpoints for backwards compatibility
    path('token/', MyTokenObtainPairView.as_view(), name='legacy_token_obtain_pair'),
    path('token/refresh/', TokenRefreshView.as_view(), name='legacy_token_refresh'),
    path('token/verify/', TokenVerifyView.as_view(), name='legacy_token_verify'),
    path('token/blacklist/', TokenBlacklistView.as_view(), name='legacy_token_blacklist'),
    
    # Alternative auth endpoints
    path('login/', login, name='api_login'),
    path('logout/', logout, name='api_logout'),
    path('user/', user_profile, name='user_profile'),
    path('test-auth/', token_test, name='token_test'),
    
    # ==========================================
    # TRADING/STRATEGY ROUTES (NEW PATTERN)
    # ==========================================
    path('strategies/', get_strategies, name='get_strategies'),
    
    # ==========================================
    # MARKET DATA ROUTES (NEW PATTERN)
    # ==========================================
    path('ping/', ping, name='binance_ping'),
    path('server-time/', server_time, name='server_time'),
    path('historical-data/', historical_data, name='historical_data'),
    
    # ==========================================
    # LEGACY ROUTES (FROM OLD VIEWS.PY)
    # ==========================================
    # These use the old views.py until migration is complete
    path('system-status/', 
         getattr(old_views, 'SystemStatus', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='system_status'),
    path('exchange-info/', 
         getattr(old_views, 'ExchangeInfo', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='exchange_info'),
    path('symbol-info/', 
         getattr(old_views, 'SymbolInfo', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='symbol_info'),
    path('all-coin-info/', 
         getattr(old_views, 'AllCoinInfo', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='all_coin_info'),
    path('week/', 
         getattr(old_views, 'Week', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='week_data'),
    path('chart/', 
         getattr(old_views, 'Chart', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='chart_data'),
    
    # ==========================================
    # ACCOUNT ROUTES (FROM OLD VIEWS.PY)
    # ==========================================
    path('account/snapshot/', 
         getattr(old_views, 'AccountSnapshot', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='account_snapshot'),
    path('account/info/', 
         getattr(old_views, 'Info', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='account_info'),
    path('account/balance/', 
         getattr(old_views, 'Balance', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='account_balance'),
    path('account/status/', 
         getattr(old_views, 'Status', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='account_status'),
    
    # ==========================================
    # TRADING ORDERS ROUTES (FROM OLD VIEWS.PY)
    # ==========================================
    path('orders/', 
         getattr(old_views, 'Orders', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='orders'),
    path('orders/place/', 
         getattr(old_views, 'PlaceOrder', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='place_order'),
    path('orders/open/', 
         getattr(old_views, 'OpenOrders', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='open_orders'),
    
    # ==========================================
    # PORTFOLIO & ANALYTICS ROUTES
    # ==========================================
    path('portfolio/patrimony/', 
         getattr(old_views, 'Patrimony', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='patrimony'),
]

# ==========================================
# PRODUCTION TRADING ROUTES (REAL MONEY!)
# ==========================================
# These endpoints execute real trades when BINANCE_USE_TESTNET=False
if TRADING_VIEWS_AVAILABLE:
    urlpatterns += [
        path('trade/status/', trading_status, name='trading_status'),
        path('trade/balance/', trading_balance, name='trading_balance'),
        path('trade/buy-btc/', buy_btc, name='buy_btc'),
        path('trade/sell-btc/', sell_btc, name='sell_btc'),
        path('trade/history/', trade_history, name='trade_history'),
        path('trade/pnl/', pnl_summary, name='pnl_summary'),
        path('trade/position-size/', calculate_position_size, name='calculate_position_size'),
    ]
    print("✅ Trading views loaded: /api/trade/status/, /api/trade/buy-btc/, etc.")
else:
    print("⚠️  Trading views not available")

# Debug info
print("🎯 URLs loaded following project patterns!")
print("📁 Using organized views: strategy_views.py, market_views.py")
print("🔗 Key routes:")
print("   - POST /api/token/ (JWT login)")
print("   - GET /api/strategies/ (using strategy_views.py)")
print("   - GET /api/ping/ (using market_views.py)")
print("   - GET /api/historical-data/ (using market_views.py)")

# Working URL structure following project patterns:
"""
✅ POST /api/token/               - JWT Login 
✅ POST /api/token/refresh/       - JWT Refresh
✅ GET /api/strategies/           - get_strategies from strategy_views.py
✅ GET /api/ping/                 - ping from market_views.py
✅ GET /api/server-time/          - server_time from market_views.py
✅ GET /api/historical-data/      - historical_data from market_views.py
✅ Legacy routes from old views.py (for gradual migration)

New routes use snake_case and follow established patterns!
"""
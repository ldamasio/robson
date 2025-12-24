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
from ..views.market_views import ping, server_time, historical_data, current_price
from ..views.portfolio import active_positions
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

# Import margin trading views
try:
    from ..views.margin_views import (
        margin_account,
        transfer_to_margin,
        transfer_from_margin,
        calculate_position_size as margin_position_size,
        open_position,
        close_position,
        list_positions,
        get_position,
        monitor_margins,
    )
    MARGIN_VIEWS_AVAILABLE = True
except ImportError as e:
    print(f"⚠️  Could not import margin views: {e}")
    MARGIN_VIEWS_AVAILABLE = False

# Import emotional trading guard views
try:
    from ..views.emotional_guard import (
        analyze_intent,
        list_signals,
        trading_tips,
        risk_levels,
    )
    EMOTIONAL_GUARD_AVAILABLE = True
except ImportError as e:
    print(f"⚠️  Could not import emotional guard views: {e}")
    EMOTIONAL_GUARD_AVAILABLE = False

# Import risk-managed trading views (PRODUCTION-SAFE)
try:
    from ..views.risk_managed_trading import (
        validate_trade,
        risk_managed_buy,
        risk_managed_sell,
        risk_status,
    )
    RISK_MANAGED_TRADING_AVAILABLE = True
except ImportError as e:
    print(f"⚠️  Could not import risk-managed trading views: {e}")
    RISK_MANAGED_TRADING_AVAILABLE = False

# Import audit trail views (COMPLETE TRANSPARENCY)
try:
    from ..views.audit_views import (
        transaction_history,
        all_activity,
        balance_history,
        sync_transactions,
    )
    AUDIT_VIEWS_AVAILABLE = True
except ImportError as e:
    print(f"⚠️  Could not import audit views: {e}")
    AUDIT_VIEWS_AVAILABLE = False

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
    
    # Demo routes
    path('demo/', include('api.urls.demo')),
    
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
    path('market/price/<str:symbol>/', current_price, name='current_price'),
    
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
    path('last-week/', 
         getattr(old_views, 'Week', fallback_view) if OLD_VIEWS_AVAILABLE else fallback_view, 
         name='last_week_data'),
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
    path('portfolio/positions/', active_positions, name='portfolio_positions'),

    # ==========================================
    # ANALYTICS ROUTES (NEW)
    # ==========================================
    # User operations endpoints (user-initiated trading)
    path('', include('api.urls.user_operations')),

    # Analytics endpoints (performance tracking)
    path('', include('api.urls.analytics')),
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

# ==========================================
# MARGIN TRADING ROUTES (ISOLATED MARGIN!)
# ==========================================
# These endpoints handle isolated margin trading operations
if MARGIN_VIEWS_AVAILABLE:
    urlpatterns += [
        # Account status
        path('margin/account/<str:symbol>/', margin_account, name='margin_account'),
        
        # Transfers
        path('margin/transfer/to/', transfer_to_margin, name='transfer_to_margin'),
        path('margin/transfer/from/', transfer_from_margin, name='transfer_from_margin'),
        
        # Position sizing
        path('margin/position/calculate/', margin_position_size, name='margin_position_size'),
        
        # Position management
        path('margin/position/open/', open_position, name='margin_open_position'),
        path('margin/position/<str:position_id>/close/', close_position, name='margin_close_position'),
        path('margin/positions/', list_positions, name='margin_positions_list'),
        path('margin/positions/<str:position_id>/', get_position, name='margin_position_detail'),
        
        # Monitoring
        path('margin/monitor/', monitor_margins, name='margin_monitor'),
    ]
    print("✅ Margin views loaded: /api/margin/account/, /api/margin/positions/, etc.")
else:
    print("⚠️  Margin views not available")

# ==========================================
# EMOTIONAL TRADING GUARD ROUTES
# ==========================================
# Protects traders from emotional decision-making
if EMOTIONAL_GUARD_AVAILABLE:
    urlpatterns += [
        path('guard/analyze/', analyze_intent, name='guard_analyze_intent'),
        path('guard/signals/', list_signals, name='guard_list_signals'),
        path('guard/tips/', trading_tips, name='guard_trading_tips'),
        path('guard/risk-levels/', risk_levels, name='guard_risk_levels'),
    ]
    print("✅ Emotional Guard views loaded: /api/guard/analyze/, etc.")
else:
    print("⚠️  Emotional Guard views not available")

# ==========================================
# RISK-MANAGED TRADING ROUTES (PRODUCTION-SAFE!)
# ==========================================
# These endpoints ENFORCE mandatory risk management rules:
# - Stop-loss REQUIRED for every trade
# - Risk per trade ≤ 1% of capital
# - Monthly drawdown ≤ 4%
if RISK_MANAGED_TRADING_AVAILABLE:
    urlpatterns += [
        path('trade/risk-managed/validate/', validate_trade, name='validate_trade'),
        path('trade/risk-managed/buy/', risk_managed_buy, name='risk_managed_buy'),
        path('trade/risk-managed/sell/', risk_managed_sell, name='risk_managed_sell'),
        path('trade/risk-managed/status/', risk_status, name='risk_status'),
    ]
    print("✅ Risk-managed trading loaded: /api/trade/risk-managed/buy/, etc.")
else:
    print("⚠️  Risk-managed trading views not available")

# ==========================================
# AUDIT TRAIL ROUTES (COMPLETE TRANSPARENCY!)
# ==========================================
# These endpoints provide full auditability of all account activity
if AUDIT_VIEWS_AVAILABLE:
    urlpatterns += [
        # Transaction history - all recorded transactions
        path('audit/transactions/', transaction_history, name='audit_transactions'),
        
        # All activity - combines trades, orders, positions, transfers
        path('audit/activity/', all_activity, name='audit_all_activity'),
        
        # Balance history - historical snapshots
        path('audit/balances/', balance_history, name='audit_balance_history'),
        
        # Sync from Binance - fill in any missing transactions
        path('audit/sync/', sync_transactions, name='audit_sync'),
    ]
    print("✅ Audit views loaded: /api/audit/transactions/, /api/audit/activity/, etc.")
else:
    print("⚠️  Audit views not available")

# Debug info
print("🎯 URLs loaded following project patterns!")
print("📁 Using organized views: strategy_views.py, market_views.py")
print("🔗 Key routes:")
print("   - POST /api/token/ (JWT login)")
print("   - GET /api/strategies/ (using strategy_views.py)")
print("   - GET /api/ping/ (using market_views.py)")
print("   - GET /api/historical-data/ (using market_views.py)")
print("   - GET /api/margin/positions/ (using margin_views.py)")

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

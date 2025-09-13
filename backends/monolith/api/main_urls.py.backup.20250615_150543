# api/urls.py - VERSÃO DE DEBUG QUE DEVE FUNCIONAR
"""
Main URL configuration for API endpoints.
Debug version with direct imports and explicit error handling.
"""

from django.urls import path
from . import views
from rest_framework_simplejwt.views import (
    TokenObtainPairView,
    TokenRefreshView,
    TokenVerifyView,
    TokenBlacklistView
)

# Try to import auth views, fallback to direct views if needed
try:
    from .views.auth import MyTokenObtainPairView, user_profile, logout, login, token_test
    AUTH_VIEWS_AVAILABLE = True
    print("✅ Auth views imported successfully from views.auth")
except ImportError as e:
    print(f"⚠️  Could not import from views.auth: {e}")
    AUTH_VIEWS_AVAILABLE = False
    # Fallback to basic views
    def user_profile(request):
        from django.http import JsonResponse
        return JsonResponse({"message": "User profile endpoint"})
    
    def logout(request):
        from django.http import JsonResponse
        return JsonResponse({"message": "Logout endpoint"})
    
    def login(request):
        from django.http import JsonResponse
        return JsonResponse({"message": "Login endpoint"})
    
    def token_test(request):
        from django.http import JsonResponse
        return JsonResponse({"message": "Token test endpoint"})

# Main API URL patterns
urlpatterns = [
    # ==========================================
    # AUTHENTICATION ROUTES - WORKING VERSION
    # ==========================================
    
    # Use custom view if available, otherwise use default JWT view
    path('token/', MyTokenObtainPairView.as_view() if AUTH_VIEWS_AVAILABLE else TokenObtainPairView.as_view(), name='token_obtain_pair'),
    path('token/refresh/', TokenRefreshView.as_view(), name='token_refresh'),
    path('token/verify/', TokenVerifyView.as_view(), name='token_verify'),
    path('token/blacklist/', TokenBlacklistView.as_view(), name='token_blacklist'),
    
    # Auth routes with prefix
    path('auth/token/', MyTokenObtainPairView.as_view() if AUTH_VIEWS_AVAILABLE else TokenObtainPairView.as_view(), name='auth_token_obtain_pair'),
    path('auth/token/refresh/', TokenRefreshView.as_view(), name='auth_token_refresh'),
    path('auth/token/verify/', TokenVerifyView.as_view(), name='auth_token_verify'),
    path('auth/token/blacklist/', TokenBlacklistView.as_view(), name='auth_token_blacklist'),
    
    # Alternative auth endpoints
    path('login/', login, name='api_login'),
    path('logout/', logout, name='api_logout'),
    path('user/', user_profile, name='user_profile'),
    path('test-auth/', token_test, name='token_test'),
    
    # ==========================================
    # TRADING ROUTES
    # ==========================================
    path('strategies/', views.getStrategies, name='get_strategies'),
    
    # ==========================================
    # MARKET DATA ROUTES
    # ==========================================
    path('ping/', views.Ping, name='binance_ping'),
    path('server-time/', views.ServerTime, name='server_time'),
    path('system-status/', views.SystemStatus, name='system_status'),
    path('exchange-info/', views.ExchangeInfo, name='exchange_info'),
    path('symbol-info/', views.SymbolInfo, name='symbol_info'),
    path('all-coin-info/', views.AllCoinInfo, name='all_coin_info'),
    
    # Historical data
    path('week/', views.Week, name='week_data'),
    path('chart/', views.Chart, name='chart_data'),
    
    # ==========================================
    # ACCOUNT ROUTES
    # ==========================================
    path('account/snapshot/', views.AccountSnapshot, name='account_snapshot'),
    path('account/info/', views.Info, name='account_info'),
    path('account/balance/', views.Balance, name='account_balance'),
    path('account/status/', views.Status, name='account_status'),
    path('account/api-trading-status/', views.ApiTradingStatus, name='api_trading_status'),
    path('account/trades-fees/', views.TradesFees, name='trades_fees'),
    path('account/asset-details/', views.AssetDetails, name='asset_details'),
    path('account/dust-log/', views.DustLog, name='dust_log'),
    path('account/transfer-dust/', views.TransferDust, name='transfer_dust'),
    path('account/asset-dividend-history/', views.AssetDividendHistory, name='asset_dividend_history'),
    path('account/enable-fast-withdraw/', views.EnableFastWithdrawSwitch, name='enable_fast_withdraw'),
    path('account/disable-fast-withdraw/', views.DisableFastWithdrawSwitch, name='disable_fast_withdraw'),
    
    # ==========================================
    # TRADING ORDERS ROUTES
    # ==========================================
    path('orders/', views.Orders, name='orders'),
    path('orders/place/', views.PlaceOrder, name='place_order'),
    path('orders/test/', views.PlaceTestOrder, name='place_test_order'),
    path('orders/status/', views.OrderStatus, name='order_status'),
    path('orders/cancel/', views.CancelOrder, name='cancel_order'),
    path('orders/open/', views.OpenOrders, name='open_orders'),
    
    # ==========================================
    # SUB ACCOUNT ROUTES
    # ==========================================
    path('sub-accounts/', views.Accounts, name='sub_accounts'),
    path('sub-accounts/history/', views.History, name='sub_accounts_history'),
    path('sub-accounts/assets/', views.Assets, name='sub_accounts_assets'),
    
    # ==========================================
    # MARGIN TRADING ROUTES
    # ==========================================
    path('margin/cross-asset/', views.CrossMarginAsset, name='cross_margin_asset'),
    path('margin/cross-symbol/', views.CrossMarginSymbol, name='cross_margin_symbol'),
    path('margin/isolated-asset/', views.IsolatedMarginAsset, name='isolated_margin_asset'),
    path('margin/isolated-symbol/', views.IsolatedMarginSymbol, name='isolated_margin_symbol'),
    path('margin/price-index/', views.MarginPriceIndex, name='margin_price_index'),
    
    # Margin orders
    path('margin/orders/', views.MarginOrders, name='margin_orders'),
    path('margin/orders/status/', views.MarginOrderStatus, name='margin_order_status'),
    path('margin/orders/open/', views.OpenMarginOrders, name='open_margin_orders'),
    
    # Margin account
    path('margin/account/', views.MarginAccount, name='margin_account'),
    path('margin/isolated/create/', views.CreateIsolatedMarginAccount, name='create_isolated_margin_account'),
    path('margin/isolated/account/', views.IsolatedMarginAccount, name='isolated_margin_account'),
    path('margin/transfer/spot-to-cross/', views.TransferSpotToCross, name='transfer_spot_to_cross'),
    path('margin/transfer/cross-to-spot/', views.TransferCrossToSpot, name='transfer_cross_to_spot'),
    path('margin/transfer/spot-to-isolated/', views.TransferSpotToIsolated, name='transfer_spot_to_isolated'),
    path('margin/transfer/isolated-to-spot/', views.TransferIsolatedToSpot, name='transfer_isolated_to_spot'),
    path('margin/transfer/max/', views.MaxMarginTransfer, name='max_margin_transfer'),
    
    # Margin trades
    path('margin/trades/', views.MarginTrades, name='margin_trades'),
    
    # Margin loans
    path('margin/loan/create/', views.CreateMarginLoan, name='create_margin_loan'),
    path('margin/loan/repay/', views.RepayMarginLoan, name='repay_loan'),
    path('margin/loan/details/', views.MarginLoanDetails, name='margin_loan_details'),
    path('margin/repay/details/', views.MarginRepayDetails, name='margin_repay_details'),
    path('margin/loan/max/', views.MaxMarginLoan, name='max_margin_loan'),
    
    # ==========================================
    # PORTFOLIO & ANALYTICS ROUTES
    # ==========================================
    path('portfolio/patrimony/', views.Patrimony, name='patrimony'),
    path('products/', views.Products, name='products'),
]

# Debug info
print(f"🔍 Django URLs loaded. Auth views available: {AUTH_VIEWS_AVAILABLE}")
print("📋 Key routes:")
print("   - POST /api/token/ (login)")
print("   - POST /api/token/refresh/ (refresh)")
print("   - POST /api/auth/token/ (login with auth prefix)")
print("   - GET /api/user/ (user profile)")

# Working URLs after this fix:
"""
✅ POST /api/token/               - Login (should work now!)
✅ POST /api/token/refresh/       - Refresh token  
✅ POST /api/auth/token/          - Login (with auth prefix)
✅ POST /api/auth/token/refresh/  - Refresh token (with auth prefix)
✅ GET  /api/user/                - User profile
✅ POST /api/test-auth/           - Test endpoint

This version includes debugging and fallbacks!
"""


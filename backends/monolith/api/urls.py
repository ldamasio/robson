from django.urls import path
from . import views
from .views import MyTokenObtainPairView
from rest_framework_simplejwt.views import (
    TokenRefreshView,
)

urlpatterns = [
    path('token/', MyTokenObtainPairView.as_view(), name='token_obtain_pair'),
    path('token/refresh/', TokenRefreshView.as_view(), name='token_refresh'),

    path('patrimony/', views.Patrimony),
    # path('balance/', views.Balance),
    # path('actual-volume/', views.Volume),
    path('4h-chart/', views.Chart),
    path('last-week/', views.Week),
    # path('trend-now/', views.Trend),
    # path('best-strategies/', views.Strategies),
    # path('risk/', views.Risk),

    # General Binance Endpoints
    path('ping/', views.Ping),
    path('server-time/', views.ServerTime),
    path('system-status/', views.SystemStatus),
    path('exchange-info/', views.ExchangeInfo),
    path('symbol_info/', views.SymbolInfo),
    path('all_tickers/', views.AllCoinInfo),
    path('daily-account_snapshot/', views.AccountSnapshot),
    path('products/', views.Products),

    # Spot Account Info Endpoints
    path('info/', views.Info),
    path('balance/', views.Balance),
    path('status/', views.Status),
    path('api-trading-status/', views.ApiTradingStatus),
    path('trades-fees/', views.TradesFees),
    path('asset-details/', views.AssetDetails),
    path('dust-log/', views.DustLog),
    path('transfer-dust/', views.TransferDust),
    path('asset-dividend-history/', views.AssetDividendHistory),
    path('enable_fast_withdraw_switch/', views.EnableFastWithdrawSwitch),
    path('disable_fast_withdraw_switch/', views.DisableFastWithdrawSwitch),

    # Spot Orders Endpoints
    path('orders/', views.Orders),
    path('place-order/', views.PlaceOrder),
    path('place-test-order/', views.PlaceTestOrder),
    path('order-status/', views.OrderStatus),
    path('cancel-order/', views.CancelOrder),
    path('open-orders/', views.OpenOrders),

    # Sub Account Endpoints
    path('accounts/', views.Accounts),
    path('history/', views.History),
    path('assets/', views.Assets),

    # Margin Market Data
    path('cross_margin_asset/', views.CrossMarginAsset),
    path('cross_margin_symbol/', views.CrossMarginSymbol),
    path('isolated_margin_asset/', views.IsolatedMarginAsset),
    path('isolated_margin_symbol/', views.IsolatedMarginSymbol),
    path('margin_price_index/', views.MarginPriceIndex),

    # Margin Order
    path('margin-orders/', views.MarginOrders),
    path('place-margin-order/', views.MarginOrders),
    path('margin-order-status/', views.MarginOrderStatus),
    path('cancel-margin-order/', views.MarginOrderStatus),
    path('open-margin-orders/', views.OpenMarginOrders),

    # Margin Account
    path('margin-account/', views.MarginAccount),
    path('create_isolated_margin_account/', views.CreateIsolatedMarginAccount),
    path('isolated_margin_account/', views.IsolatedMarginAccount),
    path('transfer_spot_to_cross/', views.TransferSpotToCross),
    path('transfer_cross_to_spot/', views.TransferCrossToSpot),
    path('transfer_spot_to_isolated/', views.TransferSpotToIsolated),
    path('transfer_isolated_to_spot/', views.TransferSpotToIsolated),
    path('get_max_margin_transfer/', views.MaxMarginTransfer),

    # Margin Trades
    path('margin_trades/', views.MarginTrades),

    # Margin Loans
    path('create_margin_loan/', views.CreateMarginLoan),
    path('repay_margin_loan/', views.RepayMarginLoan),
    path('margin_loan_details/', views.MarginLoanDetails),
    path('margin_repay_details/', views.MarginRepayDetails),
    path('max_margin_loan/', views.MaxMarginLoan),
]

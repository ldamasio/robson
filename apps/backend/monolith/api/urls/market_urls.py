
# ==========================================
# REFATORAÇÃO 3: URLs ORGANIZADAS
# ==========================================

# api/urls/market_urls.py
from django.urls import path
from ..views.market_views import ping, server_time, historical_data

urlpatterns = [
    path('ping/', ping, name='market-ping'),
    path('server-time/', server_time, name='market-server-time'),
    path('historical/', historical_data, name='market-historical'),
]


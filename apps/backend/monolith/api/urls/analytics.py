# api/urls/analytics.py
"""
URL routing for analytics endpoints.

Provides performance metrics, risk analytics, and trading statistics.
"""

from django.urls import path
from api.views import analytics

urlpatterns = [
    # Strategy performance analytics
    path(
        'analytics/strategy-performance/',
        analytics.strategy_performance,
        name='strategy-performance'
    ),

    # Risk metrics and exposure
    path(
        'analytics/risk-metrics/',
        analytics.risk_metrics,
        name='risk-metrics'
    ),
]

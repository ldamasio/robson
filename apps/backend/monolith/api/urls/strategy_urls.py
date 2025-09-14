
# api/urls/strategy_urls.py
from django.urls import path
from ..views.strategy_views import get_strategies

urlpatterns = [
    path('strategies/', get_strategies, name='strategies-list'),
]


# backend/urls.py - UPDATED PROJECT URLS
"""
Main URL configuration for Robson Bot backend.
Updated to support JWT authentication and organized API routes.
"""

from django.contrib import admin
from django.urls import path, include
from django.conf import settings
from django.conf.urls.static import static

urlpatterns = [
    # Admin interface
    path('admin/', admin.site.urls),
    
    # API routes
    path('api/', include('api.urls')),
    
    # Client routes (if any)
    path('clients/', include('clients.urls')) if hasattr(settings, 'clients') else path('', lambda r: None),
]

# Serve static files in development
if settings.DEBUG:
    urlpatterns += static(settings.STATIC_URL, document_root=settings.STATIC_ROOT)

# Add health check endpoint
from django.http import JsonResponse
from django.views.decorators.csrf import csrf_exempt

@csrf_exempt
def health_check(request):
    """Health check endpoint for monitoring"""
    return JsonResponse({
        'status': 'healthy',
        'service': 'robson-bot-api',
        'version': '1.0.0',
        'environment': 'development' if settings.DEBUG else 'production'
    })

# Add health check to URL patterns
urlpatterns += [
    path('health/', health_check, name='health_check'),
]

# URL structure overview for frontend developers:
"""
Main API Routes:

Authentication:
- POST /api/auth/token/           - Login (get access & refresh tokens)
- POST /api/auth/token/refresh/   - Refresh access token
- POST /api/auth/token/verify/    - Verify token validity
- POST /api/auth/token/blacklist/ - Logout (blacklist refresh token)
- GET  /api/user/                 - Get user profile

Trading:
- GET  /api/strategies/           - Get user strategies
- POST /api/orders/place/         - Place trading order
- GET  /api/orders/               - Get orders history
- GET  /api/orders/open/          - Get open orders

Market Data:
- GET  /api/ping/                 - Test Binance connection
- GET  /api/server-time/          - Get server time
- GET  /api/week/                 - Get week historical data
- GET  /api/exchange-info/        - Get exchange information

Account:
- GET  /api/account/info/         - Get account information
- GET  /api/account/balance/      - Get account balance
- GET  /api/portfolio/patrimony/  - Get portfolio value

Admin:
- GET  /admin/                    - Django admin interface

Monitoring:
- GET  /health/                   - Health check endpoint
"""

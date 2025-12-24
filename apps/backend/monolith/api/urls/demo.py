"""
Demo routes for handling demo account creation and management.

Includes endpoints for:
- Creating demo accounts with testnet credentials
- Validating demo trial periods
- Upgrading demo accounts to Pro
"""

from django.urls import path
from ..views.demo import (
    create_demo_account,
    check_demo_status,
    upgrade_to_pro,
    validate_demo_credentials
)

# Demo URL patterns
urlpatterns = [
    # Demo account creation and management
    path('demo/create/', create_demo_account, name='demo_create'),
    path('demo/status/', check_demo_status, name='demo_status'),
    path('demo/upgrade/', upgrade_to_pro, name='demo_upgrade'),
    path('demo/validate-credentials/', validate_demo_credentials, name='demo_validate_credentials'),
]

# For easy reference in frontend
"""
Frontend should use these endpoints:

POST /api/demo/create/
{
    "username": "demo_user",
    "email": "demo@example.com",
    "password": "secure_password",
    "api_key": "binance_testnet_api_key",
    "secret_key": "binance_testnet_secret_key"
}

GET /api/demo/status/
Headers: Authorization: Bearer <token>

POST /api/demo/upgrade/
Headers: Authorization: Bearer <token>
{
    "api_key": "production_api_key",
    "secret_key": "production_secret_key"
}

POST /api/demo/validate-credentials/
{
    "api_key": "testnet_api_key",
    "secret_key": "testnet_secret_key"
}
"""
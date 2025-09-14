# api/urls/auth.py - NEW FILE
"""
JWT Authentication routes for frontend connection.
Organizes authentication endpoints separately.
"""

from django.urls import path
from rest_framework_simplejwt.views import (
    TokenRefreshView,
    TokenVerifyView,
    TokenBlacklistView
)
from ..views.auth import MyTokenObtainPairView

# Authentication URL patterns
urlpatterns = [
    # JWT Token endpoints
    path('token/', MyTokenObtainPairView.as_view(), name='token_obtain_pair'),
    path('token/refresh/', TokenRefreshView.as_view(), name='token_refresh'),
    path('token/verify/', TokenVerifyView.as_view(), name='token_verify'),
    path('token/blacklist/', TokenBlacklistView.as_view(), name='token_blacklist'),
]

# For easy reference in frontend
"""
Frontend should use these endpoints:

POST /api/auth/token/
{
    "username": "user@example.com",
    "password": "password123"
}
Response: {
    "access": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
    "refresh": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
    "username": "user@example.com"
}

POST /api/auth/token/refresh/
{
    "refresh": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
Response: {
    "access": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}

POST /api/auth/token/verify/
{
    "token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
Response: {} (200 if valid, 401 if invalid)

POST /api/auth/token/blacklist/
{
    "refresh": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
Response: {} (200 if successful)
"""

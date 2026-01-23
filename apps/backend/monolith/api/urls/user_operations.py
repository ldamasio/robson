# api/urls/user_operations.py
"""
URL routing for user-initiated operations.

Endpoints for Robson's core value: Risk Management Assistant.
"""

from django.urls import path

from api.views import user_operations

urlpatterns = [
    # Position sizing calculator (preview)
    path(
        "operations/calculate-size/",
        user_operations.calculate_position_size,
        name="calculate-position-size",
    ),
    # Create user operation (full version)
    path("operations/create/", user_operations.create_user_operation, name="create-user-operation"),
    # Register user operation intent (simplified MVP version)
    path(
        "operations/", user_operations.register_operation_intent, name="register-operation-intent"
    ),
    # List user's strategies
    path("strategies/", user_operations.list_user_strategies, name="list-strategies"),
]


# ==========================================
# REFACTOR 2: CLEAN VIEWS
# ==========================================

# api/views/base.py
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status
import logging

logger = logging.getLogger(__name__)

class BaseAPIView:
    """Base class for API views."""
    
    def get_client_id(self, request):
        """Get client (tenant) id from the authenticated user, if any."""
        try:
            return request.user.client.id
        except AttributeError:
            return None
    
    def handle_error(self, error, message="An error occurred"):
        """Standardized error handling helper."""
        logger.error(f"{message}: {error}")
        return Response(
            {"error": message, "detail": str(error)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


# ==========================================
# REFATORAÇÃO 2: VIEWS LIMPAS
# ==========================================

# api/views/base.py
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status
import logging

logger = logging.getLogger(__name__)

class BaseAPIView:
    """Classe base para views da API"""
    
    def get_client_id(self, request):
        """Obtém ID do cliente (tenant)"""
        try:
            return request.user.client.id
        except AttributeError:
            return None
    
    def handle_error(self, error, message="An error occurred"):
        """Padroniza tratamento de erros"""
        logger.error(f"{message}: {error}")
        return Response(
            {"error": message, "detail": str(error)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


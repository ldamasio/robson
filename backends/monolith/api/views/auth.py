# api/views/auth.py - NEW FILE
"""
Authentication views for JWT token management.
Extracted from main views.py for better organization.
"""

from rest_framework import status
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated, AllowAny
from rest_framework.response import Response
from rest_framework_simplejwt.serializers import TokenObtainPairSerializer
from rest_framework_simplejwt.views import TokenObtainPairView
from rest_framework_simplejwt.tokens import RefreshToken
from django.contrib.auth import authenticate
from clients.models import CustomUser
import logging

logger = logging.getLogger(__name__)

class MyTokenObtainPairSerializer(TokenObtainPairSerializer):
    """
    Custom JWT serializer that adds extra user info to token.
    Includes username and other user details in response.
    """
    
    @classmethod
    def get_token(cls, user):
        token = super().get_token(user)
        
        # Add custom claims to token
        token['username'] = user.username
        token['email'] = user.email
        
        # Add client info if available
        if hasattr(user, 'client') and user.client:
            token['client_id'] = user.client.id
            token['client_name'] = user.client.name
        
        return token
    
    def validate(self, attrs):
        """
        Custom validation that includes extra user info in response.
        """
        data = super().validate(attrs)
        
        # Add extra user information to response
        data['username'] = self.user.username
        data['email'] = self.user.email
        data['user_id'] = self.user.id
        
        # Add client information if available
        if hasattr(self.user, 'client') and self.user.client:
            data['client_id'] = self.user.client.id
            data['client_name'] = self.user.client.name
        else:
            data['client_id'] = None
            data['client_name'] = None
        
        # Log successful login
        logger.info(f"User {self.user.username} logged in successfully")
        
        return data

class MyTokenObtainPairView(TokenObtainPairView):
    """
    Custom JWT token view with enhanced error handling and logging.
    """
    serializer_class = MyTokenObtainPairSerializer
    
    def post(self, request, *args, **kwargs):
        """
        Handle login request with enhanced error handling.
        """
        try:
            response = super().post(request, *args, **kwargs)
            
            if response.status_code == 200:
                # Log successful authentication
                username = request.data.get('username', 'Unknown')
                logger.info(f"Successful authentication for user: {username}")
            
            return response
            
        except Exception as e:
            # Log authentication errors
            username = request.data.get('username', 'Unknown')
            logger.warning(f"Authentication failed for user: {username}, Error: {str(e)}")
            
            return Response(
                {
                    'error': 'Authentication failed',
                    'detail': 'Invalid credentials or server error'
                },
                status=status.HTTP_401_UNAUTHORIZED
            )

# Additional authentication endpoints
@api_view(['POST'])
@permission_classes([AllowAny])
def login(request):
    """
    Alternative login endpoint (if needed for compatibility).
    """
    username = request.data.get('username')
    password = request.data.get('password')
    
    if not username or not password:
        return Response(
            {'error': 'Username and password required'},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    # Authenticate user
    user = authenticate(username=username, password=password)
    
    if user is not None:
        if user.is_active:
            # Generate tokens
            refresh = RefreshToken.for_user(user)
            access = refresh.access_token
            
            # Add custom claims
            access['username'] = user.username
            access['email'] = user.email
            
            return Response({
                'access': str(access),
                'refresh': str(refresh),
                'username': user.username,
                'email': user.email,
                'user_id': user.id,
                'client_id': user.client.id if hasattr(user, 'client') and user.client else None,
                'client_name': user.client.name if hasattr(user, 'client') and user.client else None,
            })
        else:
            return Response(
                {'error': 'Account is disabled'},
                status=status.HTTP_401_UNAUTHORIZED
            )
    else:
        return Response(
            {'error': 'Invalid credentials'},
            status=status.HTTP_401_UNAUTHORIZED
        )

@api_view(['POST'])
@permission_classes([IsAuthenticated])
def logout(request):
    """
    Logout endpoint that blacklists the refresh token.
    """
    try:
        refresh_token = request.data.get('refresh')
        
        if refresh_token:
            token = RefreshToken(refresh_token)
            token.blacklist()
            
            logger.info(f"User {request.user.username} logged out successfully")
            
            return Response(
                {'message': 'Successfully logged out'},
                status=status.HTTP_200_OK
            )
        else:
            return Response(
                {'error': 'Refresh token required'},
                status=status.HTTP_400_BAD_REQUEST
            )
            
    except Exception as e:
        logger.warning(f"Logout error for user {request.user.username}: {str(e)}")
        return Response(
            {'error': 'Logout failed'},
            status=status.HTTP_400_BAD_REQUEST
        )

@api_view(['GET'])
@permission_classes([IsAuthenticated])
def user_profile(request):
    """
    Get current user profile information.
    """
    user = request.user
    
    return Response({
        'user_id': user.id,
        'username': user.username,
        'email': user.email,
        'first_name': user.first_name,
        'last_name': user.last_name,
        'client_id': user.client.id if hasattr(user, 'client') and user.client else None,
        'client_name': user.client.name if hasattr(user, 'client') and user.client else None,
        'is_active': user.is_active,
        'date_joined': user.date_joined,
        'last_login': user.last_login,
    })

@api_view(['POST'])
@permission_classes([AllowAny])
def token_test(request):
    """
    Test endpoint to verify JWT authentication is working.
    """
    return Response({
        'message': 'JWT authentication is working',
        'timestamp': request.META.get('HTTP_DATE'),
        'authenticated': request.user.is_authenticated,
        'user': request.user.username if request.user.is_authenticated else 'Anonymous'
    })

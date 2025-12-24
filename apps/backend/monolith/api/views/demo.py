"""
Demo views for handling demo account creation and trial management.

Includes endpoints for:
- Creating demo accounts with testnet credentials
- Validating demo trial periods
- Upgrading demo accounts to Pro
"""

from rest_framework import status
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import AllowAny, IsAuthenticated
from rest_framework.response import Response
from django.contrib.auth import authenticate
from django.utils.timezone import now
from datetime import timedelta
import logging

from clients.models import Client, CustomUser, WaitlistEntry
from .auth import MyTokenObtainPairSerializer

logger = logging.getLogger(__name__)


@api_view(['POST'])
@permission_classes([AllowAny])
def create_demo_account(request):
    """
    Create a demo account with testnet credentials.
    
    This endpoint creates a new client (tenant) and user account
    for demo purposes with a 3-day trial period.
    
    Request body:
    {
        "username": "demo_user",
        "email": "demo@example.com",
        "password": "secure_password",
        "api_key": "binance_testnet_api_key",
        "secret_key": "binance_testnet_secret_key"
    }
    """
    try:
        data = request.data
        
        # Validate required fields
        required_fields = ['username', 'email', 'password', 'api_key', 'secret_key']
        for field in required_fields:
            if field not in data or not data[field]:
                return Response(
                    {'error': f'Campo obrigatório: {field}'},
                    status=status.HTTP_400_BAD_REQUEST
                )
        
        # Check if email already exists
        if CustomUser.objects.filter(email=data['email']).exists():
            return Response(
                {'error': 'Email já está em uso'},
                status=status.HTTP_400_BAD_REQUEST
            )
        
        # Check if username already exists
        if CustomUser.objects.filter(username=data['username']).exists():
            return Response(
                {'error': 'Nome de usuário já está em uso'},
                status=status.HTTP_400_BAD_REQUEST
            )
        
        # Create client (tenant) with demo credentials
        client = Client.objects.create(
            name=f"Demo - {data['username']}",
            email=data['email'],
            is_demo_account=True
        )
        
        # Set encrypted credentials
        client.set_credentials(data['api_key'], data['secret_key'])
        
        # Start 3-day demo trial
        client.start_demo_trial(days=3)
        client.save()
        
        # Create user account associated with the client
        user = CustomUser.objects.create_user(
            username=data['username'],
            email=data['email'],
            password=data['password'],
            client=client
        )
        
        # Generate JWT tokens for immediate login
        serializer = MyTokenObtainPairSerializer()
        tokens = serializer.validate({
            'username': data['username'],
            'password': data['password']
        })
        
        logger.info(f"Demo account created for user: {data['username']} with client ID: {client.id}")
        
        return Response({
            'message': 'Conta demo criada com sucesso',
            'tokens': tokens,
            'client_id': client.id,
            'demo_expires_at': client.demo_expires_at,
            'remaining_days': client.get_demo_remaining_days()
        }, status=status.HTTP_201_CREATED)
        
    except Exception as e:
        logger.error(f"Error creating demo account: {str(e)}")
        return Response(
            {'error': 'Erro interno ao criar conta demo'},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def check_demo_status(request):
    """
    Check the status of the current user's demo trial.
    
    Returns demo expiration information and remaining time.
    """
    try:
        user = request.user
        
        if not user.client or not user.client.is_demo_account:
            return Response({
                'is_demo': False,
                'message': 'Conta não é uma demo'
            })
        
        client = user.client
        
        return Response({
            'is_demo': True,
            'demo_created_at': client.demo_created_at,
            'demo_expires_at': client.demo_expires_at,
            'remaining_days': client.get_demo_remaining_days(),
            'is_expired': client.is_demo_expired(),
            'message': 'Conta demo ativa' if not client.is_demo_expired() else 'Demo expirada'
        })
        
    except Exception as e:
        logger.error(f"Error checking demo status: {str(e)}")
        return Response(
            {'error': 'Erro ao verificar status da demo'},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def upgrade_to_pro(request):
    """
    Upgrade a demo account to a full Pro account.
    
    Request body should include production API credentials:
    {
        "api_key": "production_api_key",
        "secret_key": "production_secret_key"
    }
    """
    try:
        user = request.user
        
        if not user.client or not user.client.is_demo_account:
            return Response(
                {'error': 'Esta não é uma conta demo'},
                status=status.HTTP_400_BAD_REQUEST
            )
        
        data = request.data
        
        # Validate production credentials
        if 'api_key' not in data or 'secret_key' not in data:
            return Response(
                {'error': 'Credenciais de produção são obrigatórias'},
                status=status.HTTP_400_BAD_REQUEST
            )
        
        client = user.client
        
        # Update with production credentials
        client.set_credentials(data['api_key'], data['secret_key'])
        
        # Upgrade to Pro account
        client.upgrade_to_pro()
        
        logger.info(f"Demo account upgraded to Pro for user: {user.username}")
        
        return Response({
            'message': 'Conta atualizada para Pro com sucesso',
            'is_demo': False,
            'upgraded_at': now()
        })
        
    except Exception as e:
        logger.error(f"Error upgrading to Pro: {str(e)}")
        return Response(
            {'error': 'Erro ao atualizar para Pro'},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['POST'])
@permission_classes([AllowAny])
def validate_demo_credentials(request):
    """
    Validate demo credentials before account creation.
    
    This endpoint tests if the provided testnet credentials
    are valid before creating a demo account.
    
    Request body:
    {
        "api_key": "testnet_api_key",
        "secret_key": "testnet_secret_key"
    }
    """
    try:
        data = request.data
        
        if 'api_key' not in data or 'secret_key' not in data:
            return Response(
                {'error': 'API Key e Secret Key são obrigatórias'},
                status=status.HTTP_400_BAD_REQUEST
            )
        
        # TODO: Implement actual Binance Testnet API validation
        # For now, we'll just do basic validation
        
        if len(data['api_key']) < 10 or len(data['secret_key']) < 10:
            return Response(
                {'error': 'Credenciais inválidas'},
                status=status.HTTP_400_BAD_REQUEST
            )
        
        return Response({
            'valid': True,
            'message': 'Credenciais de testnet válidas'
        })
        
    except Exception as e:
        logger.error(f"Error validating demo credentials: {str(e)}")
        return Response(
            {'error': 'Erro ao validar credenciais'},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['POST'])
@permission_classes([AllowAny])
def join_waitlist(request):
    """
    Join the Pro plan waitlist.
    
    Users can join the waitlist to be notified when Pro plan
    becomes available with payment integration.
    
    Request body:
    {
        "email": "user@example.com",
        "is_demo_user": true  # Optional, defaults to false
    }
    """
    try:
        data = request.data
        
        if 'email' not in data or not data['email']:
            return Response(
                {'error': 'Email é obrigatório'},
                status=status.HTTP_400_BAD_REQUEST
            )
        
        email = data['email'].strip().lower()
        is_demo_user = data.get('is_demo_user', False)
        client_id = data.get('client_id')
        
        # Check if email already exists in waitlist
        if WaitlistEntry.objects.filter(email=email).exists():
            return Response(
                {'error': 'Este email já está na lista de espera'},
                status=status.HTTP_400_BAD_REQUEST
            )
        
        # Get client if provided
        client = None
        if client_id:
            try:
                client = Client.objects.get(id=client_id)
            except Client.DoesNotExist:
                pass
        
        # Create waitlist entry
        waitlist_entry = WaitlistEntry.objects.create(
            email=email,
            client=client,
            is_demo_user=is_demo_user
        )
        
        logger.info(f"New waitlist entry: {email} (Demo: {is_demo_user})")
        
        return Response({
            'success': True,
            'message': 'Inscrito na lista de espera com sucesso!',
            'email': email,
            'is_demo_user': is_demo_user,
            'created_at': waitlist_entry.created_at
        }, status=status.HTTP_201_CREATED)
        
    except Exception as e:
        logger.error(f"Error joining waitlist: {str(e)}")
        return Response(
            {'error': 'Erro ao se inscrever na lista de espera'},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def get_waitlist_status(request):
    """
    Get the current user's waitlist status.
    
    Returns whether the current user is on the waitlist
    and their position/status.
    """
    try:
        user = request.user
        
        # Check if user is on waitlist by email
        waitlist_entry = WaitlistEntry.objects.filter(email=user.email).first()
        
        if waitlist_entry:
            return Response({
                'on_waitlist': True,
                'email': waitlist_entry.email,
                'is_demo_user': waitlist_entry.is_demo_user,
                'created_at': waitlist_entry.created_at,
                'notified': waitlist_entry.notified_at is not None,
                'notified_at': waitlist_entry.notified_at,
                'message': 'Você está na lista de espera do plano Pro!'
            })
        else:
            return Response({
                'on_waitlist': False,
                'message': 'Você não está na lista de espera'
            })
        
    except Exception as e:
        logger.error(f"Error getting waitlist status: {str(e)}")
        return Response(
            {'error': 'Erro ao verificar status da lista de espera'},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )
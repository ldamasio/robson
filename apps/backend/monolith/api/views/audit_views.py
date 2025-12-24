"""
Audit Views - API endpoints for transaction history and audit trail.

Provides complete transparency for users to see all account activity.
"""

import logging
from decimal import Decimal

from django.utils import timezone
from rest_framework import status
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response

from api.models.audit import AuditTransaction, BalanceSnapshot, TransactionType
from api.models.margin import MarginPosition, MarginTransfer
from api.models import Trade, Order
from api.services.audit_service import AuditService
from clients.models import Client

logger = logging.getLogger(__name__)


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def transaction_history(request):
    """
    Get complete transaction history for the authenticated user.
    
    Query params:
        - type: Filter by transaction type (spot_buy, margin_buy, etc.)
        - symbol: Filter by symbol (BTCUSDC)
        - limit: Number of results (default: 50)
        - offset: Pagination offset
    
    Returns all transactions for complete transparency.
    """
    try:
        # Get client for the user
        client = getattr(request.user, 'client', None)
        if not client:
            # Fallback to client ID 1 for testing
            try:
                client = Client.objects.get(id=1)
            except Client.DoesNotExist:
                return Response({
                    'success': False,
                    'error': 'No client associated with user',
                }, status=status.HTTP_400_BAD_REQUEST)
        
        # Query parameters
        tx_type = request.query_params.get('type')
        symbol = request.query_params.get('symbol')
        limit = int(request.query_params.get('limit', 50))
        offset = int(request.query_params.get('offset', 0))
        
        # Build query
        queryset = AuditTransaction.objects.filter(client=client)
        
        if tx_type:
            queryset = queryset.filter(transaction_type=tx_type.upper())
        
        if symbol:
            queryset = queryset.filter(symbol=symbol.upper())
        
        # Get total count before pagination
        total = queryset.count()
        
        # Apply pagination
        transactions = queryset.order_by('-created_at')[offset:offset + limit]
        
        # Serialize
        data = []
        for tx in transactions:
            data.append({
                'id': tx.id,
                'transaction_id': tx.transaction_id,
                'binance_order_id': tx.binance_order_id,
                'type': tx.transaction_type,
                'type_display': tx.get_transaction_type_display(),
                'status': tx.status,
                'symbol': tx.symbol,
                'asset': tx.asset,
                'side': tx.side,
                'quantity': str(tx.quantity),
                'price': str(tx.price) if tx.price else None,
                'total_value': str(tx.total_value) if tx.total_value else None,
                'fee': str(tx.fee),
                'fee_asset': tx.fee_asset,
                'leverage': tx.leverage,
                'is_margin': tx.is_isolated_margin,
                'stop_price': str(tx.stop_price) if tx.stop_price else None,
                'risk_amount': str(tx.risk_amount) if tx.risk_amount else None,
                'risk_percent': str(tx.risk_percent) if tx.risk_percent else None,
                'description': tx.description,
                'created_at': tx.created_at.isoformat(),
                'executed_at': tx.executed_at.isoformat() if tx.executed_at else None,
                'source': tx.source,
            })
        
        return Response({
            'success': True,
            'transactions': data,
            'total': total,
            'limit': limit,
            'offset': offset,
        })
        
    except Exception as e:
        logger.error(f"Failed to get transaction history: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def all_activity(request):
    """
    Get ALL account activity from all sources.
    
    Combines:
    - AuditTransactions
    - Trades
    - Orders
    - MarginPositions
    - MarginTransfers
    
    This gives a complete view of everything that happened.
    """
    try:
        client = getattr(request.user, 'client', None)
        if not client:
            try:
                client = Client.objects.get(id=1)
            except Client.DoesNotExist:
                return Response({
                    'success': False,
                    'error': 'No client found',
                }, status=status.HTTP_400_BAD_REQUEST)
        
        limit = int(request.query_params.get('limit', 100))
        
        activities = []
        
        # Get audit transactions
        for tx in AuditTransaction.objects.filter(client=client).order_by('-created_at')[:limit]:
            activities.append({
                'type': 'audit_transaction',
                'subtype': tx.transaction_type,
                'description': tx.description,
                'symbol': tx.symbol,
                'side': tx.side,
                'quantity': str(tx.quantity),
                'price': str(tx.price) if tx.price else None,
                'status': tx.status,
                'timestamp': tx.created_at.isoformat(),
                'binance_id': tx.binance_order_id,
            })
        
        # Get trades
        for trade in Trade.objects.all().order_by('-entry_time')[:limit]:
            activities.append({
                'type': 'trade',
                'subtype': f'SPOT_{trade.side}',
                'description': f"Spot {trade.side} {trade.quantity} @ ${trade.entry_price}",
                'symbol': trade.symbol.name if trade.symbol else 'BTCUSDC',
                'side': trade.side,
                'quantity': str(trade.quantity),
                'price': str(trade.entry_price),
                'status': 'FILLED',
                'timestamp': trade.entry_time.isoformat() if trade.entry_time else None,
                'binance_id': None,
            })
        
        # Get orders with Binance IDs
        for order in Order.objects.filter(binance_order_id__isnull=False).order_by('-created_at')[:limit]:
            activities.append({
                'type': 'order',
                'subtype': f'ORDER_{order.side}',
                'description': f"Order {order.side} {order.quantity} @ ${order.avg_fill_price or 'MARKET'}",
                'symbol': order.symbol.name if order.symbol else 'BTCUSDC',
                'side': order.side,
                'quantity': str(order.quantity),
                'price': str(order.avg_fill_price) if order.avg_fill_price else None,
                'status': order.status,
                'timestamp': order.created_at.isoformat() if order.created_at else None,
                'binance_id': order.binance_order_id,
            })
        
        # Get margin positions
        for pos in MarginPosition.objects.filter(client=client).order_by('-created_at')[:limit]:
            activities.append({
                'type': 'margin_position',
                'subtype': f'MARGIN_{pos.side}',
                'description': f"Margin {pos.side} {pos.quantity} BTC @ ${pos.entry_price} ({pos.leverage}x)",
                'symbol': pos.symbol,
                'side': pos.side,
                'quantity': str(pos.quantity),
                'price': str(pos.entry_price),
                'status': pos.status,
                'timestamp': pos.created_at.isoformat(),
                'binance_id': pos.binance_entry_order_id,
                'extra': {
                    'leverage': pos.leverage,
                    'stop_price': str(pos.stop_price),
                    'risk_amount': str(pos.risk_amount),
                    'risk_percent': str(pos.risk_percent),
                    'margin_level': str(pos.margin_level),
                },
            })
        
        # Get margin transfers
        for transfer in MarginTransfer.objects.filter(client=client).order_by('-created_at')[:limit]:
            direction = "Spot → Margin" if transfer.direction == "TO_MARGIN" else "Margin → Spot"
            activities.append({
                'type': 'margin_transfer',
                'subtype': transfer.direction,
                'description': f"Transfer {transfer.amount} {transfer.asset} ({direction})",
                'symbol': transfer.symbol,
                'side': None,
                'quantity': str(transfer.amount),
                'price': None,
                'status': 'COMPLETED' if transfer.success else 'FAILED',
                'timestamp': transfer.created_at.isoformat(),
                'binance_id': transfer.transaction_id,
            })
        
        # Sort by timestamp
        activities.sort(key=lambda x: x['timestamp'] or '', reverse=True)
        
        # Limit results
        activities = activities[:limit]
        
        return Response({
            'success': True,
            'activities': activities,
            'total': len(activities),
        })
        
    except Exception as e:
        logger.error(f"Failed to get all activity: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def balance_history(request):
    """
    Get balance snapshot history.
    
    Shows how the account balance changed over time.
    """
    try:
        client = getattr(request.user, 'client', None)
        if not client:
            try:
                client = Client.objects.get(id=1)
            except Client.DoesNotExist:
                return Response({
                    'success': False,
                    'error': 'No client found',
                }, status=status.HTTP_400_BAD_REQUEST)
        
        limit = int(request.query_params.get('limit', 100))
        
        snapshots = BalanceSnapshot.objects.filter(client=client).order_by('-snapshot_time')[:limit]
        
        data = []
        for snap in snapshots:
            data.append({
                'id': snap.id,
                'timestamp': snap.snapshot_time.isoformat(),
                'spot_usdc': str(snap.spot_usdc),
                'spot_btc': str(snap.spot_btc),
                'margin_btc_free': str(snap.margin_btc_free),
                'margin_btc_borrowed': str(snap.margin_btc_borrowed),
                'margin_usdc_free': str(snap.margin_usdc_free),
                'margin_usdc_borrowed': str(snap.margin_usdc_borrowed),
                'btc_price': str(snap.btc_price),
                'total_equity': str(snap.total_equity),
                'margin_level': str(snap.margin_level) if snap.margin_level else None,
            })
        
        return Response({
            'success': True,
            'snapshots': data,
            'total': len(data),
        })
        
    except Exception as e:
        logger.error(f"Failed to get balance history: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def sync_transactions(request):
    """
    Trigger a sync of transactions from Binance.
    
    This ensures the audit trail is complete.
    """
    try:
        client = getattr(request.user, 'client', None)
        if not client:
            try:
                client = Client.objects.get(id=1)
            except Client.DoesNotExist:
                return Response({
                    'success': False,
                    'error': 'No client found',
                }, status=status.HTTP_400_BAD_REQUEST)
        
        days = int(request.data.get('days', 7))
        take_snapshot = request.data.get('snapshot', True)
        
        audit_service = AuditService(client)
        
        # Sync transactions
        count = audit_service.sync_from_binance(days_back=days)
        
        result = {
            'success': True,
            'synced_count': count,
        }
        
        # Take snapshot if requested
        if take_snapshot:
            snapshot = audit_service.take_balance_snapshot()
            result['snapshot'] = {
                'total_equity': str(snapshot.total_equity),
                'timestamp': snapshot.snapshot_time.isoformat(),
            }
        
        return Response(result)
        
    except Exception as e:
        logger.error(f"Failed to sync transactions: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


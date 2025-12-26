"""
Portfolio BTC API - Endpoints for BTC-denominated portfolio tracking.

This module provides REST API endpoints for tracking portfolio value
and performance in BTC terms (crypto investor's preferred metric).

Endpoints:
- GET /api/portfolio/btc/total/ - Current portfolio value in BTC
- GET /api/portfolio/btc/profit/ - Profit in BTC using user's formula
- GET /api/portfolio/btc/history/ - Historical BTC value over time
- GET /api/portfolio/deposits-withdrawals/ - List of deposits/withdrawals
"""

from decimal import Decimal
from datetime import datetime
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status

from clients.models import Client
from api.services.portfolio_btc_service import PortfolioBTCService


def _get_client(request):
    """Get client from request user."""
    # Try to get client_id from user object
    client_id = getattr(request.user, 'client_id', None)
    if client_id:
        try:
            return Client.objects.get(id=client_id)
        except Client.DoesNotExist:
            pass

    # Fallback to client 1 for development
    return Client.objects.get(id=1)


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def portfolio_btc_total(request):
    """
    Get current portfolio value denominated in BTC.

    Returns total portfolio value in BTC with breakdown by account type
    and individual assets.

    Response:
    {
        "total_btc": "0.5234",
        "spot_btc": "0.5000",
        "margin_btc": "0.0234",
        "margin_debt_btc": "0.0000",
        "breakdown": {
            "BTC": "0.50000000",
            "ETH": "0.02000000",
            "USDC": "0.00340000",
        }
    }
    """
    try:
        client = _get_client(request)
        service = PortfolioBTCService(client)

        portfolio = service.calculate_total_portfolio_btc()

        # Convert decimals to strings for JSON
        response_data = {
            "total_btc": str(portfolio["total_btc"]),
            "spot_btc": str(portfolio["spot_btc"]),
            "margin_btc": str(portfolio["margin_btc"]),
            "margin_debt_btc": str(portfolio["margin_debt_btc"]),
            "breakdown": {
                asset: str(value)
                for asset, value in portfolio["breakdown"].items()
            }
        }

        return Response(response_data)

    except Exception as e:
        return Response(
            {"error": f"Failed to calculate portfolio BTC: {str(e)}"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def portfolio_btc_profit(request):
    """
    Calculate profit in BTC using user's formula.

    Formula: Profit (BTC) = Current Balance (BTC) + Withdrawals (BTC) - Deposits (BTC)

    Query Parameters:
    - since (optional): Start date (YYYY-MM-DD)

    Response:
    {
        "profit_btc": "0.0234",
        "profit_percent": "4.67",
        "current_balance_btc": "0.5234",
        "total_deposits_btc": "0.5000",
        "total_withdrawals_btc": "0.0000",
        "net_inflows_btc": "0.5000",
        "start_date": "2024-01-01T00:00:00Z",
        "calculated_at": "2025-12-26T10:30:00Z"
    }
    """
    try:
        client = _get_client(request)
        service = PortfolioBTCService(client)

        # Parse optional start date
        since_str = request.query_params.get('since')
        since = None
        if since_str:
            try:
                since = datetime.strptime(since_str, '%Y-%m-%d')
            except ValueError:
                return Response(
                    {"error": "Invalid date format. Use YYYY-MM-DD"},
                    status=status.HTTP_400_BAD_REQUEST
                )

        profit_data = service.calculate_profit_btc(since=since)

        # Convert to JSON-serializable format
        response_data = {
            "profit_btc": str(profit_data["profit_btc"]),
            "profit_percent": str(profit_data["profit_percent"]),
            "current_balance_btc": str(profit_data["current_balance_btc"]),
            "total_deposits_btc": str(profit_data["total_deposits_btc"]),
            "total_withdrawals_btc": str(profit_data["total_withdrawals_btc"]),
            "net_inflows_btc": str(profit_data["net_inflows_btc"]),
            "start_date": profit_data["start_date"].isoformat() if profit_data["start_date"] else None,
            "calculated_at": profit_data["calculated_at"].isoformat(),
        }

        return Response(response_data)

    except Exception as e:
        return Response(
            {"error": f"Failed to calculate profit: {str(e)}"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def portfolio_btc_history(request):
    """
    Get historical portfolio value in BTC.

    Query Parameters:
    - start_date (optional): Start date (YYYY-MM-DD)
    - end_date (optional): End date (YYYY-MM-DD)

    Response:
    {
        "history": [
            {
                "snapshot_time": "2025-12-26T10:00:00Z",
                "total_btc": "0.5234",
                "spot_btc": "0.5000",
                "margin_btc": "0.0234",
                "btc_price": "95000.00",
            },
            ...
        ]
    }
    """
    try:
        client = _get_client(request)
        service = PortfolioBTCService(client)

        # Parse dates
        start_date_str = request.query_params.get('start_date')
        end_date_str = request.query_params.get('end_date')

        start_date = None
        end_date = None

        if start_date_str:
            try:
                start_date = datetime.strptime(start_date_str, '%Y-%m-%d')
            except ValueError:
                return Response(
                    {"error": "Invalid start_date format. Use YYYY-MM-DD"},
                    status=status.HTTP_400_BAD_REQUEST
                )

        if end_date_str:
            try:
                end_date = datetime.strptime(end_date_str, '%Y-%m-%d')
            except ValueError:
                return Response(
                    {"error": "Invalid end_date format. Use YYYY-MM-DD"},
                    status=status.HTTP_400_BAD_REQUEST
                )

        snapshots = service.get_btc_history(start_date=start_date, end_date=end_date)

        history = []
        for snapshot in snapshots:
            history.append({
                "snapshot_time": snapshot.snapshot_time.isoformat(),
                "total_btc": str(snapshot.total_equity_btc) if snapshot.total_equity_btc else "0",
                "spot_btc": str(snapshot.spot_btc_value),
                "margin_btc": str(snapshot.margin_btc_value),
                "btc_price": str(snapshot.btc_price),
            })

        return Response({"history": history})

    except Exception as e:
        return Response(
            {"error": f"Failed to get history: {str(e)}"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def deposits_withdrawals(request):
    """
    Get list of all deposits and withdrawals.

    Query Parameters:
    - type (optional): Filter by type (deposit/withdrawal)
    - limit (optional): Max records to return (default: 50)

    Response:
    {
        "transactions": [
            {
                "id": "uuid",
                "type": "DEPOSIT",
                "asset": "BTC",
                "quantity": "0.10000000",
                "executed_at": "2025-12-26T10:00:00Z",
                "btc_value": "0.10000000",
            },
            ...
        ]
    }
    """
    try:
        client = _get_client(request)

        # Build queryset
        from api.models.audit import AuditTransaction, TransactionType

        queryset = AuditTransaction.objects.filter(
            client=client,
            transaction_type__in=[TransactionType.DEPOSIT, TransactionType.WITHDRAWAL],
        )

        # Filter by type
        tx_type = request.query_params.get('type', '').upper()
        if tx_type == 'DEPOSIT':
            queryset = queryset.filter(transaction_type=TransactionType.DEPOSIT)
        elif tx_type == 'WITHDRAWAL':
            queryset = queryset.filter(transaction_type=TransactionType.WITHDRAWAL)

        # Limit
        limit = int(request.query_params.get('limit', 50))
        queryset = queryset.order_by('-executed_at')[:limit]

        # Serialize
        transactions = []
        service = PortfolioBTCService(client)

        for tx in queryset:
            btc_value = service.converter.convert_to_btc(tx.asset, tx.quantity)

            transactions.append({
                "id": tx.transaction_id,
                "type": tx.transaction_type,
                "asset": tx.asset,
                "quantity": str(tx.quantity),
                "executed_at": tx.executed_at.isoformat() if tx.executed_at else None,
                "btc_value": str(btc_value),
                "description": tx.description,
            })

        return Response({"transactions": transactions})

    except Exception as e:
        return Response(
            {"error": f"Failed to get transactions: {str(e)}"},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )

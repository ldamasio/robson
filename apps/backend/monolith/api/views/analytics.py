# api/views/analytics.py
"""
Analytics API endpoints for performance tracking.

Provides insights into strategy performance, trade statistics, and risk metrics.
"""

from decimal import Decimal
from datetime import datetime, timedelta
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status
from django.db.models import Count, Sum, Avg, Max, Min, Q, F
from django.utils import timezone

from api.models import Strategy, Operation, Trade, Position


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def strategy_performance(request):
    """
    Get performance analytics by strategy.

    Query Parameters:
    - strategy_id (optional): Filter by specific strategy
    - start_date (optional): Filter trades from this date (YYYY-MM-DD)
    - end_date (optional): Filter trades until this date (YYYY-MM-DD)
    - top_n (optional): Return only top N strategies by P&L (default: all)

    Response:
    {
        "strategies": [
            {
                "id": 1,
                "name": "Mean Reversion MA99",
                "description": "...",
                "is_active": true,
                "statistics": {
                    "total_trades": 10,
                    "winning_trades": 6,
                    "losing_trades": 4,
                    "win_rate": 60.0,
                    "total_pnl": "125.50",
                    "average_pnl_per_trade": "12.55",
                    "best_trade": "45.00",
                    "worst_trade": "-15.00",
                    "average_win": "35.25",
                    "average_loss": "11.25",
                    "profit_factor": 3.13,
                    "total_operations": 8,
                    "active_operations": 2,
                    "closed_operations": 6
                },
                "monthly_breakdown": [
                    {
                        "month": "2025-12",
                        "trades": 5,
                        "pnl": "75.00",
                        "win_rate": 80.0
                    },
                    ...
                ]
            },
            ...
        ],
        "summary": {
            "total_strategies": 3,
            "total_trades": 25,
            "overall_pnl": "200.00",
            "overall_win_rate": 64.0
        }
    }
    """
    client = request.user.client

    # Parse query parameters
    strategy_id = request.query_params.get('strategy_id')
    start_date_str = request.query_params.get('start_date')
    end_date_str = request.query_params.get('end_date')
    top_n = request.query_params.get('top_n')

    # Build base queryset
    strategies_qs = Strategy.objects.filter(client=client)

    if strategy_id:
        strategies_qs = strategies_qs.filter(id=strategy_id)

    # Parse date filters
    start_date = None
    end_date = None
    if start_date_str:
        try:
            start_date = datetime.strptime(start_date_str, '%Y-%m-%d').date()
        except ValueError:
            return Response(
                {'error': 'Invalid start_date format. Use YYYY-MM-DD'},
                status=status.HTTP_400_BAD_REQUEST
            )

    if end_date_str:
        try:
            end_date = datetime.strptime(end_date_str, '%Y-%m-%d').date()
        except ValueError:
            return Response(
                {'error': 'Invalid end_date format. Use YYYY-MM-DD'},
                status=status.HTTP_400_BAD_REQUEST
            )

    # Build analytics for each strategy
    strategies_data = []
    total_trades_all = 0
    total_pnl_all = Decimal("0")
    total_winning_all = 0

    for strategy in strategies_qs:
        # Get trades for this strategy
        trades_qs = Trade.objects.filter(client=client, strategy=strategy)

        # Apply date filters
        if start_date:
            trades_qs = trades_qs.filter(entry_time__date__gte=start_date)
        if end_date:
            trades_qs = trades_qs.filter(entry_time__date__lte=end_date)

        # Calculate statistics
        total_trades = trades_qs.count()
        if total_trades == 0:
            # Skip strategies with no trades in the filtered period
            continue

        # Get closed trades (with exit_price)
        closed_trades = trades_qs.filter(exit_price__isnull=False)

        winning_trades = closed_trades.filter(
            Q(side='BUY', exit_price__gt=F('entry_price')) |
            Q(side='SELL', exit_price__lt=F('entry_price'))
        ).count()

        losing_trades = closed_trades.filter(
            Q(side='BUY', exit_price__lte=F('entry_price')) |
            Q(side='SELL', exit_price__gte=F('entry_price'))
        ).count()

        # P&L calculations
        total_pnl = closed_trades.aggregate(Sum('pnl'))['pnl__sum'] or Decimal("0")
        avg_pnl = closed_trades.aggregate(Avg('pnl'))['pnl__avg'] or Decimal("0")
        best_trade = closed_trades.aggregate(Max('pnl'))['pnl__max'] or Decimal("0")
        worst_trade = closed_trades.aggregate(Min('pnl'))['pnl__min'] or Decimal("0")

        # Win/Loss averages
        winning_pnl = closed_trades.filter(pnl__gt=0).aggregate(Avg('pnl'))['pnl__avg'] or Decimal("0")
        losing_pnl = closed_trades.filter(pnl__lt=0).aggregate(Avg('pnl'))['pnl__avg'] or Decimal("0")

        # Profit factor (total wins / abs(total losses))
        total_wins = closed_trades.filter(pnl__gt=0).aggregate(Sum('pnl'))['pnl__sum'] or Decimal("0")
        total_losses = abs(closed_trades.filter(pnl__lt=0).aggregate(Sum('pnl'))['pnl__sum'] or Decimal("0"))
        profit_factor = float(total_wins / total_losses) if total_losses > 0 else float('inf')

        # Win rate
        closed_count = closed_trades.count()
        win_rate = float((winning_trades / closed_count) * 100) if closed_count > 0 else 0.0

        # Operations statistics
        operations = Operation.objects.filter(client=client, strategy=strategy)
        total_operations = operations.count()
        active_operations = operations.filter(status='ACTIVE').count()
        closed_operations = operations.filter(status='CLOSED').count()

        # Monthly breakdown
        monthly_data = []
        if start_date and end_date:
            # Group by month
            current_month = start_date.replace(day=1)
            end_month = end_date.replace(day=1)

            while current_month <= end_month:
                next_month = (current_month.replace(day=28) + timedelta(days=4)).replace(day=1)

                month_trades = closed_trades.filter(
                    entry_time__gte=current_month,
                    entry_time__lt=next_month
                )

                month_count = month_trades.count()
                if month_count > 0:
                    month_pnl = month_trades.aggregate(Sum('pnl'))['pnl__sum'] or Decimal("0")
                    month_wins = month_trades.filter(pnl__gt=0).count()
                    month_win_rate = float((month_wins / month_count) * 100)

                    monthly_data.append({
                        'month': current_month.strftime('%Y-%m'),
                        'trades': month_count,
                        'pnl': str(month_pnl),
                        'win_rate': round(month_win_rate, 2)
                    })

                current_month = next_month

        # Build strategy response
        strategies_data.append({
            'id': strategy.id,
            'name': strategy.name,
            'description': strategy.description,
            'is_active': strategy.is_active,
            'statistics': {
                'total_trades': total_trades,
                'winning_trades': winning_trades,
                'losing_trades': losing_trades,
                'win_rate': round(win_rate, 2),
                'total_pnl': str(total_pnl),
                'average_pnl_per_trade': str(avg_pnl.quantize(Decimal('0.01'))),
                'best_trade': str(best_trade),
                'worst_trade': str(worst_trade),
                'average_win': str(winning_pnl.quantize(Decimal('0.01'))),
                'average_loss': str(losing_pnl.quantize(Decimal('0.01'))),
                'profit_factor': round(profit_factor, 2) if profit_factor != float('inf') else 'N/A',
                'total_operations': total_operations,
                'active_operations': active_operations,
                'closed_operations': closed_operations,
            },
            'monthly_breakdown': monthly_data,
            '_sort_pnl': float(total_pnl)  # For sorting
        })

        # Accumulate totals
        total_trades_all += total_trades
        total_pnl_all += total_pnl
        total_winning_all += winning_trades

    # Sort by total P&L (descending)
    strategies_data.sort(key=lambda x: x['_sort_pnl'], reverse=True)

    # Remove sorting key
    for s in strategies_data:
        del s['_sort_pnl']

    # Apply top_n limit
    if top_n:
        try:
            top_n_int = int(top_n)
            strategies_data = strategies_data[:top_n_int]
        except ValueError:
            pass

    # Calculate overall summary
    overall_win_rate = float((total_winning_all / total_trades_all) * 100) if total_trades_all > 0 else 0.0

    summary = {
        'total_strategies': len(strategies_data),
        'total_trades': total_trades_all,
        'overall_pnl': str(total_pnl_all),
        'overall_win_rate': round(overall_win_rate, 2),
    }

    return Response({
        'strategies': strategies_data,
        'summary': summary,
    })


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def risk_metrics(request):
    """
    Get risk metrics and exposure summary.

    Response:
    {
        "current_exposure": {
            "total_position_value": "5000.00",
            "open_positions": 3,
            "largest_position": "2000.00",
            "total_unrealized_pnl": "125.50"
        },
        "risk_limits": {
            "max_risk_per_trade": "1.0",
            "current_month_drawdown": "2.5",
            "max_month_drawdown": "4.0",
            "is_within_limits": true
        },
        "recent_stops": [
            {
                "symbol": "BTCUSDC",
                "side": "BUY",
                "quantity": "0.001",
                "stop_price": "88200.00",
                "pnl": "-10.00",
                "executed_at": "2025-12-22T10:30:00Z"
            }
        ]
    }
    """
    client = request.user.client

    # Current exposure
    open_positions = Position.objects.filter(client=client, status='OPEN')
    total_position_value = sum(p.cost_basis for p in open_positions)
    largest_position = max([p.cost_basis for p in open_positions], default=Decimal("0"))
    total_unrealized_pnl = sum(p.unrealized_pnl or Decimal("0") for p in open_positions)

    current_exposure = {
        'total_position_value': str(total_position_value),
        'open_positions': open_positions.count(),
        'largest_position': str(largest_position),
        'total_unrealized_pnl': str(total_unrealized_pnl),
    }

    # Risk limits (simplified - would integrate with PolicyState in full implementation)
    risk_limits = {
        'max_risk_per_trade': "1.0",
        'current_month_drawdown': "0.0",  # TODO: Calculate from PolicyState
        'max_month_drawdown': "4.0",
        'is_within_limits': True,
    }

    # Recent stop executions (last 10)
    recent_stops = []
    recent_closed = Trade.objects.filter(
        client=client,
        exit_price__isnull=False
    ).order_by('-exit_time')[:10]

    for trade in recent_closed:
        if trade.exit_time:
            recent_stops.append({
                'symbol': trade.symbol.name,
                'side': trade.side,
                'quantity': str(trade.quantity),
                'entry_price': str(trade.entry_price),
                'exit_price': str(trade.exit_price),
                'pnl': str(trade.pnl),
                'executed_at': trade.exit_time.isoformat() if trade.exit_time else None,
            })

    return Response({
        'current_exposure': current_exposure,
        'risk_limits': risk_limits,
        'recent_stops': recent_stops,
    })

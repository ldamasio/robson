# api/views/user_operations.py
"""
REST API endpoints for user-initiated trading operations.

These endpoints implement Robson's core value: Risk Management Assistant.

Key Principle: USER initiates, ROBSON calculates, USER confirms.
"""

from decimal import Decimal
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status
from django.db import transaction
from django.utils import timezone

from api.models import Operation, Order, Strategy, Symbol
from api.services.position_sizing import PositionSizingCalculator
from api.services.binance_service import BinanceService


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def calculate_position_size(request):
    """
    Calculate optimal position size for user's trade intent.

    This endpoint previews the calculation WITHOUT creating anything.
    User can review before committing.

    POST /api/operations/calculate-size/

    Request Body:
    {
        "symbol": "BTCUSDC",
        "side": "BUY",
        "entry_price": "90000.00",
        "stop_price": "88200.00",
        "capital": "1000.00",  # Optional, defaults to user's available capital
        "max_risk_percent": "1.0"  # Optional, defaults to 1%
    }

    Response:
    {
        "quantity": "0.00555556",
        "position_value": "500.00",
        "risk_amount": "10.00",
        "risk_percent": "1.00",
        "stop_distance": "1800.00",
        "stop_distance_percent": "2.00",
        "is_capped": false,
        "validation": {
            "passed": true,
            "warnings": [],
            "errors": []
        }
    }
    """
    # Parse request
    symbol_name = request.data.get('symbol')
    side = request.data.get('side')
    entry_price_str = request.data.get('entry_price')
    stop_price_str = request.data.get('stop_price')
    capital_str = request.data.get('capital')
    max_risk_percent_str = request.data.get('max_risk_percent', '1.0')

    # Validation
    if not all([symbol_name, side, entry_price_str, stop_price_str]):
        return Response(
            {
                'error': 'Missing required fields',
                'required': ['symbol', 'side', 'entry_price', 'stop_price']
            },
            status=status.HTTP_400_BAD_REQUEST
        )

    try:
        entry_price = Decimal(entry_price_str)
        stop_price = Decimal(stop_price_str)
        max_risk_percent = Decimal(max_risk_percent_str)

        # Get capital (use provided or user's available capital)
        if capital_str:
            capital = Decimal(capital_str)
        else:
            # TODO: Get from user's actual portfolio
            capital = Decimal("1000.00")

    except (ValueError, TypeError) as e:
        return Response(
            {'error': f'Invalid decimal values: {e}'},
            status=status.HTTP_400_BAD_REQUEST
        )

    # Calculate position size (Robson's intelligence)
    try:
        calc = PositionSizingCalculator.calculate(
            capital=capital,
            entry_price=entry_price,
            stop_price=stop_price,
            side=side,
            max_risk_percent=max_risk_percent,
        )
    except ValueError as e:
        return Response(
            {'error': str(e)},
            status=status.HTTP_400_BAD_REQUEST
        )

    # Perform additional validations
    warnings = []
    errors = []

    # Check if position is capped
    if calc['is_capped']:
        warnings.append(
            f"Position capped at 50% of capital (${calc['position_value']})"
        )

    # Check risk percent
    if calc['risk_percent'] > max_risk_percent:
        errors.append(
            f"Risk exceeds limit: {calc['risk_percent']}% > {max_risk_percent}%"
        )

    # Check stop distance (warn if too tight)
    if calc['stop_distance_percent'] < Decimal("0.5"):
        warnings.append(
            f"Stop is very tight: {calc['stop_distance_percent']}% (consider widening)"
        )

    # Check stop distance (warn if too wide)
    if calc['stop_distance_percent'] > Decimal("10.0"):
        warnings.append(
            f"Stop is very wide: {calc['stop_distance_percent']}% (high risk)"
        )

    validation_passed = len(errors) == 0

    # Return calculation result
    return Response({
        'quantity': str(calc['quantity']),
        'position_value': str(calc['position_value']),
        'risk_amount': str(calc['risk_amount']),
        'risk_percent': str(calc['risk_percent']),
        'stop_distance': str(calc['stop_distance']),
        'stop_distance_percent': str(calc['stop_distance_percent']),
        'is_capped': calc['is_capped'],
        'validation': {
            'passed': validation_passed,
            'warnings': warnings,
            'errors': errors,
        },
        'capital_used': str(capital),
        'max_risk_percent': str(max_risk_percent),
    })


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def create_user_operation(request):
    """
    Create a user-initiated trading operation.

    This endpoint creates the operation and optionally executes the order.

    POST /api/operations/create/

    Request Body:
    {
        "symbol": "BTCUSDC",
        "side": "BUY",
        "entry_price": "90000.00",
        "stop_price": "88200.00",
        "target_price": "93600.00",  # Optional
        "strategy_name": "Mean Reversion MA99",
        "execute": false,  # If true, places real order
        "capital": "1000.00",  # Optional
        "max_risk_percent": "1.0"  # Optional
    }

    Response (success):
    {
        "operation_id": 123,
        "order_id": 456,
        "status": "ACTIVE",
        "calculated_quantity": "0.00555556",
        "calculated_risk": "10.00",
        "binance_order_id": "7891234",  # If executed
        "fill_price": "90050.00",  # If executed
        "message": "Operation created successfully"
    }

    Response (validation error):
    {
        "error": "Risk validation failed",
        "details": {
            "risk_percent": "2.5",
            "limit_percent": "1.0",
            "reason": "Risk exceeds limit"
        }
    }
    """
    # Parse request
    symbol_name = request.data.get('symbol')
    side = request.data.get('side')
    entry_price_str = request.data.get('entry_price')
    stop_price_str = request.data.get('stop_price')
    target_price_str = request.data.get('target_price')
    strategy_name = request.data.get('strategy_name')
    execute = request.data.get('execute', False)
    capital_str = request.data.get('capital')
    max_risk_percent_str = request.data.get('max_risk_percent', '1.0')

    # Validation
    required_fields = ['symbol', 'side', 'entry_price', 'stop_price', 'strategy_name']
    if not all([symbol_name, side, entry_price_str, stop_price_str, strategy_name]):
        return Response(
            {
                'error': 'Missing required fields',
                'required': required_fields
            },
            status=status.HTTP_400_BAD_REQUEST
        )

    try:
        entry_price = Decimal(entry_price_str)
        stop_price = Decimal(stop_price_str)
        target_price = Decimal(target_price_str) if target_price_str else None
        max_risk_percent = Decimal(max_risk_percent_str)

        if capital_str:
            capital = Decimal(capital_str)
        else:
            # TODO: Get from user's actual portfolio
            capital = Decimal("1000.00")

    except (ValueError, TypeError) as e:
        return Response(
            {'error': f'Invalid decimal values: {e}'},
            status=status.HTTP_400_BAD_REQUEST
        )

    # Get user's client
    client = request.user.client

    # Calculate position size
    try:
        calc = PositionSizingCalculator.calculate(
            capital=capital,
            entry_price=entry_price,
            stop_price=stop_price,
            side=side,
            max_risk_percent=max_risk_percent,
        )
    except ValueError as e:
        return Response(
            {'error': str(e)},
            status=status.HTTP_400_BAD_REQUEST
        )

    # Risk validation
    if calc['risk_percent'] > max_risk_percent:
        return Response(
            {
                'error': 'Risk validation failed',
                'details': {
                    'risk_percent': str(calc['risk_percent']),
                    'limit_percent': str(max_risk_percent),
                    'reason': 'Risk exceeds limit'
                }
            },
            status=status.HTTP_400_BAD_REQUEST
        )

    # Create operation
    try:
        with transaction.atomic():
            # Get or create symbol
            symbol, _ = Symbol.objects.get_or_create(
                client=client,
                name=symbol_name.upper(),
                defaults={
                    'base_asset': symbol_name[:-4] if symbol_name.endswith(('USDC', 'USDT')) else symbol_name[:3],
                    'quote_asset': symbol_name[-4:] if symbol_name.endswith(('USDC', 'USDT')) else symbol_name[3:],
                    'is_active': True,
                }
            )

            # Get or create strategy
            strategy, _ = Strategy.objects.get_or_create(
                client=client,
                name=strategy_name,
                defaults={
                    'description': f'User-selected strategy: {strategy_name}',
                    'is_active': True,
                    'config': {},
                    'risk_config': {
                        'max_risk_per_trade_percent': str(max_risk_percent),
                    },
                }
            )

            # Create Operation with ABSOLUTE stop/target prices
            # ⭐ ADR-0012: Always use absolute prices (FIXED levels)
            operation = Operation.objects.create(
                client=client,
                symbol=symbol,
                strategy=strategy,
                side=side,
                status='PLANNED',
                stop_price=stop_price,  # ⭐ Absolute stop level (from user)
                target_price=target_price,  # ⭐ Absolute target level (optional)
                # Also calculate percentages for reference (DEPRECATED fields)
                stop_loss_percent=calc['stop_distance_percent'],  # Reference only
                stop_gain_percent=(
                    abs((target_price - entry_price) / entry_price * 100)
                    if target_price else None
                ),
            )

            # Create Entry Order
            entry_order = Order.objects.create(
                client=client,
                symbol=symbol,
                strategy=strategy,
                side=side,
                order_type='MARKET',
                quantity=calc['quantity'],
                price=entry_price,
                status='PENDING',
                stop_loss_price=stop_price,
            )

            # Link to operation
            operation.entry_orders.add(entry_order)

            # Execute if requested
            binance_order_id = None
            fill_price = None

            if execute:
                binance = BinanceService()

                result = binance.client.create_order(
                    symbol=symbol_name.upper(),
                    side=side,
                    type='MARKET',
                    quantity=str(calc['quantity'])
                )

                # Update order with results
                entry_order.binance_order_id = result.get('orderId')
                entry_order.status = result.get('status', 'FILLED')
                entry_order.filled_quantity = Decimal(str(result.get('executedQty', str(calc['quantity']))))

                if 'fills' in result:
                    total_cost = sum(
                        Decimal(f['price']) * Decimal(f['qty'])
                        for f in result['fills']
                    )
                    total_filled = sum(Decimal(f['qty']) for f in result['fills'])
                    entry_order.avg_fill_price = total_cost / total_filled if total_filled else entry_price
                else:
                    entry_order.avg_fill_price = entry_price

                entry_order.filled_at = timezone.now()
                entry_order.save()

                # Update operation
                operation.status = 'ACTIVE'
                operation.save()

                binance_order_id = entry_order.binance_order_id
                fill_price = entry_order.avg_fill_price

            # Return success
            return Response({
                'operation_id': operation.id,
                'order_id': entry_order.id,
                'status': operation.status,
                'calculated_quantity': str(calc['quantity']),
                'calculated_risk': str(calc['risk_amount']),
                'calculated_risk_percent': str(calc['risk_percent']),
                'position_value': str(calc['position_value']),
                'binance_order_id': str(binance_order_id) if binance_order_id else None,
                'fill_price': str(fill_price) if fill_price else None,
                'message': 'Operation created successfully' if not execute else 'Operation created and executed',
            }, status=status.HTTP_201_CREATED)

    except Exception as e:
        return Response(
            {'error': f'Failed to create operation: {str(e)}'},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def list_user_strategies(request):
    """
    List user's available strategies.

    GET /api/strategies/

    Response:
    {
        "strategies": [
            {
                "id": 1,
                "name": "Mean Reversion MA99",
                "description": "...",
                "is_active": true,
                "performance": {
                    "total_trades": 10,
                    "win_rate": 60.0,
                    "total_pnl": "50.00"
                }
            }
        ]
    }
    """
    client = request.user.client
    strategies = Strategy.objects.filter(client=client, is_active=True)

    strategies_data = []
    for strategy in strategies:
        strategies_data.append({
            'id': strategy.id,
            'name': strategy.name,
            'description': strategy.description,
            'is_active': strategy.is_active,
            'performance': {
                'total_trades': strategy.total_trades,
                'win_rate': strategy.win_rate,
                'total_pnl': str(strategy.total_pnl),
            },
        })

    return Response({'strategies': strategies_data})

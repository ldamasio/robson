"""
Risk-Managed Trading Views.

These endpoints ENFORCE mandatory risk management rules:
- Every trade MUST have a stop-loss
- Risk per trade MUST NOT exceed 1% of capital
- Monthly drawdown MUST NOT exceed 4%

These are the PRODUCTION endpoints for safe trading.
The legacy buy_btc/sell_btc endpoints will be deprecated.

API:
- POST /api/trade/risk-managed/buy/     - Execute a risk-managed buy
- POST /api/trade/risk-managed/sell/    - Execute a risk-managed sell
- POST /api/trade/risk-managed/validate/ - Validate before execution
"""

import logging
from decimal import Decimal, InvalidOperation
from datetime import datetime, timedelta

from django.conf import settings
from django.db import transaction
from django.db.models import Sum
from django.utils import timezone
from rest_framework import status
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response

from api.models import Trade, Symbol, Order
from api.application.adapters import BinanceExecution, BinanceMarketData
from api.application.execution import ExecutionMode, ExecutionStatus
from api.application.risk_managed_trade import RiskManagedTradeUseCase

logger = logging.getLogger(__name__)


def _get_monthly_pnl(client_id=None) -> Decimal:
    """
    Get the current month's realized P&L.
    
    Returns:
        Total P&L for the current month (negative if losing)
    """
    now = timezone.now()
    start_of_month = now.replace(day=1, hour=0, minute=0, second=0, microsecond=0)
    
    # Get all closed trades this month
    closed_trades = Trade.objects.filter(
        exit_price__isnull=False,
        exit_time__gte=start_of_month,
    )
    
    # Calculate total P&L
    total_pnl = Decimal("0")
    for trade in closed_trades:
        if trade.pnl:
            total_pnl += trade.pnl
    
    return total_pnl


def _get_or_create_symbol(pair: str) -> Symbol:
    """Get or create a Symbol instance for the given trading pair."""
    if pair.endswith('USDC'):
        base = pair[:-4]
        quote = 'USDC'
    elif pair.endswith('USDT'):
        base = pair[:-4]
        quote = 'USDT'
    else:
        base = pair[:3]
        quote = pair[3:]
    
    symbol, created = Symbol.objects.get_or_create(
        name=pair,
        defaults={
            'description': f'{base}/{quote} trading pair',
            'base_asset': base,
            'quote_asset': quote,
        }
    )
    return symbol


class DjangoPnLRepository:
    """Repository for P&L data using Django ORM."""
    
    def get_monthly_pnl(self, client_id=None) -> Decimal:
        """Get current month's realized P&L."""
        return _get_monthly_pnl(client_id)


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def validate_trade(request):
    """
    Validate a trade against risk management rules BEFORE execution.
    
    This endpoint should be called first to check if a trade is safe.
    
    Request body:
        - symbol: Trading pair (e.g., "BTCUSDC") (required)
        - side: "BUY" or "SELL" (required)
        - entry_price: Entry price or current market price (required)
        - stop_price: Stop-loss price (REQUIRED - no trade without stop)
        - quantity: Position size (optional - calculated if not provided)
        - capital: Total available capital (required)
        
    Returns:
        - is_valid: Whether the trade passes all risk checks
        - guards: Results of each risk check
        - safe_quantity: Maximum safe position size
        - message: Summary message
    """
    try:
        # Parse required fields
        symbol = request.data.get('symbol', 'BTCUSDC')
        side = request.data.get('side', 'BUY').upper()
        entry_price = request.data.get('entry_price')
        stop_price = request.data.get('stop_price')
        capital = request.data.get('capital')
        quantity = request.data.get('quantity')
        
        # Validate required fields
        if not entry_price:
            return Response({
                'success': False,
                'error': 'entry_price is required',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        if not stop_price:
            return Response({
                'success': False,
                'error': '❌ stop_price is REQUIRED - no trade without a stop-loss',
                'rule': '1% Risk Rule: Every trade MUST have a defined stop-loss',
                'docs': 'docs/PRODUCTION_TRADING.md#mandatory-risk-management-rules',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        if not capital:
            return Response({
                'success': False,
                'error': 'capital is required to calculate risk percentage',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Convert to Decimal
        try:
            entry_price = Decimal(str(entry_price))
            stop_price = Decimal(str(stop_price))
            capital = Decimal(str(capital))
            quantity = Decimal(str(quantity)) if quantity else None
        except (InvalidOperation, ValueError) as e:
            return Response({
                'success': False,
                'error': f'Invalid number format: {e}',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Create use case
        execution = BinanceExecution()
        pnl_repo = DjangoPnLRepository()
        
        use_case = RiskManagedTradeUseCase(
            execution_adapter=execution,
            pnl_repository=pnl_repo,
        )
        
        # Calculate quantity if not provided
        if quantity is None:
            quantity = use_case.calculate_position_size(capital, entry_price, stop_price)
        
        # Get monthly P&L
        monthly_pnl = _get_monthly_pnl()
        
        # Validate
        result = use_case.validate(
            symbol=symbol,
            side=side,
            quantity=quantity,
            entry_price=entry_price,
            stop_price=stop_price,
            capital=capital,
            monthly_pnl=monthly_pnl,
        )
        
        # Calculate additional metrics
        stop_distance = abs(entry_price - stop_price)
        risk_amount = stop_distance * quantity
        risk_percent = (risk_amount / capital) * Decimal("100")
        position_value = quantity * entry_price
        
        return Response({
            'success': True,
            'validation': result.to_dict(),
            'trade_summary': {
                'symbol': symbol,
                'side': side,
                'quantity': str(quantity),
                'entry_price': str(entry_price),
                'stop_price': str(stop_price),
                'stop_distance': str(stop_distance),
                'stop_distance_percent': str((stop_distance / entry_price) * Decimal("100")),
                'position_value': str(position_value),
                'risk_amount': str(risk_amount),
                'risk_percent': str(risk_percent),
            },
            'monthly_status': {
                'pnl': str(monthly_pnl),
                'is_profitable': monthly_pnl >= 0,
            },
            'timestamp': timezone.now().isoformat(),
        })
        
    except Exception as e:
        logger.error(f"Validation failed: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def risk_managed_buy(request):
    """
    Execute a risk-managed BUY order.
    
    This endpoint ENFORCES all risk management rules:
    - Stop-loss is REQUIRED
    - Risk must be ≤ 1% of capital
    - Monthly drawdown must be ≤ 4%
    
    The order will be BLOCKED if any rule is violated.
    
    Request body:
        - symbol: Trading pair (default: "BTCUSDC")
        - entry_price: Entry price (required, or fetched from market)
        - stop_price: Stop-loss price (REQUIRED)
        - quantity: Position size (optional - calculated for max 1% risk)
        - capital: Total available capital (required)
        - take_profit_price: Optional take-profit price
        - strategy_name: Name of strategy (default: "manual")
        - mode: "dry-run" (default) or "live"
        
    Returns:
        - Execution result with all guards and actions
        - Trade details if successful
    """
    # Safety check
    trading_enabled = getattr(settings, 'TRADING_ENABLED', False)
    if not trading_enabled:
        return Response({
            'success': False,
            'error': 'Trading is disabled. Set TRADING_ENABLED=True to enable.',
        }, status=status.HTTP_403_FORBIDDEN)
    
    try:
        # Parse request
        symbol = request.data.get('symbol', 'BTCUSDC')
        stop_price = request.data.get('stop_price')
        capital = request.data.get('capital')
        entry_price = request.data.get('entry_price')
        quantity = request.data.get('quantity')
        take_profit_price = request.data.get('take_profit_price')
        strategy_name = request.data.get('strategy_name', 'manual')
        mode_str = request.data.get('mode', 'dry-run').lower()
        
        # CRITICAL: Stop-loss is REQUIRED
        if not stop_price:
            return Response({
                'success': False,
                'error': '❌ STOP-LOSS REQUIRED: Cannot execute trade without stop_price',
                'rule': 'Risk Rule: Every trade MUST have a defined stop-loss',
                'recommendation': 'Provide stop_price parameter with your stop-loss level',
                'docs': 'docs/PRODUCTION_TRADING.md#mandatory-risk-management-rules',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        if not capital:
            return Response({
                'success': False,
                'error': 'capital is required to calculate risk percentage',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Determine execution mode
        mode = ExecutionMode.LIVE if mode_str == 'live' else ExecutionMode.DRY_RUN
        
        # Initialize adapters
        execution = BinanceExecution()
        market_data = BinanceMarketData()
        pnl_repo = DjangoPnLRepository()
        
        # Get entry price if not provided
        if not entry_price:
            entry_price = market_data.best_ask(symbol)
        
        # Convert to Decimal
        try:
            entry_price = Decimal(str(entry_price))
            stop_price = Decimal(str(stop_price))
            capital = Decimal(str(capital))
            quantity = Decimal(str(quantity)) if quantity else None
            take_profit_price = Decimal(str(take_profit_price)) if take_profit_price else None
        except (InvalidOperation, ValueError) as e:
            return Response({
                'success': False,
                'error': f'Invalid number format: {e}',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Get monthly P&L
        monthly_pnl = _get_monthly_pnl()
        
        # Create and execute use case
        use_case = RiskManagedTradeUseCase(
            execution_adapter=execution,
            pnl_repository=pnl_repo,
        )
        
        result = use_case.execute_buy(
            symbol=symbol,
            capital=capital,
            entry_price=entry_price,
            stop_price=stop_price,
            quantity=quantity,
            take_profit_price=take_profit_price,
            mode=mode,
            monthly_pnl=monthly_pnl,
            strategy_name=strategy_name,
        )
        
        # Record trade if LIVE and successful
        if mode == ExecutionMode.LIVE and result.is_success():
            with transaction.atomic():
                symbol_obj = _get_or_create_symbol(symbol)
                order_data = result.metadata.get('order', {})
                
                trade = Trade.objects.create(
                    symbol=symbol_obj,
                    side='BUY',
                    quantity=Decimal(order_data.get('quantity', '0')),
                    entry_price=entry_price,
                    entry_time=timezone.now(),
                )
                
                result.metadata['trade_id'] = trade.id
        
        # Format response
        response_data = {
            'success': result.is_success(),
            'execution': result.to_dict(),
            'mode': mode.value,
            'environment': 'production' if not execution.use_testnet else 'testnet',
            'timestamp': timezone.now().isoformat(),
        }
        
        if result.is_blocked():
            response_data['blocked'] = True
            response_data['blocked_by'] = result.error
            return Response(response_data, status=status.HTTP_403_FORBIDDEN)
        
        if result.status == ExecutionStatus.FAILED:
            return Response(response_data, status=status.HTTP_500_INTERNAL_SERVER_ERROR)
        
        return Response(response_data)
        
    except Exception as e:
        logger.error(f"Risk-managed buy failed: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def risk_managed_sell(request):
    """
    Execute a risk-managed SELL order.
    
    Similar to risk_managed_buy but for selling.
    Can be used to close a long position or open a short.
    
    Request body:
        - symbol: Trading pair (default: "BTCUSDC")
        - quantity: Amount to sell (required)
        - stop_price: Stop-loss price (REQUIRED for shorts)
        - capital: Total capital (required)
        - entry_price: Entry price for risk calculation
        - strategy_name: Name of strategy (default: "manual")
        - mode: "dry-run" (default) or "live"
    """
    trading_enabled = getattr(settings, 'TRADING_ENABLED', False)
    if not trading_enabled:
        return Response({
            'success': False,
            'error': 'Trading is disabled. Set TRADING_ENABLED=True to enable.',
        }, status=status.HTTP_403_FORBIDDEN)
    
    try:
        # Parse request
        symbol = request.data.get('symbol', 'BTCUSDC')
        quantity = request.data.get('quantity')
        stop_price = request.data.get('stop_price')
        capital = request.data.get('capital')
        entry_price = request.data.get('entry_price')
        strategy_name = request.data.get('strategy_name', 'manual')
        mode_str = request.data.get('mode', 'dry-run').lower()
        
        # For closing a long position, stop_price is not strictly required
        # But for opening a short, it IS required
        is_closing_long = request.data.get('closing_long', False)
        
        if not is_closing_long and not stop_price:
            return Response({
                'success': False,
                'error': '❌ STOP-LOSS REQUIRED for short positions',
                'hint': 'Set closing_long=true if you are closing an existing long position',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        if not quantity:
            return Response({
                'success': False,
                'error': 'quantity is required for sell orders',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        if not capital:
            return Response({
                'success': False,
                'error': 'capital is required to calculate risk percentage',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Determine execution mode
        mode = ExecutionMode.LIVE if mode_str == 'live' else ExecutionMode.DRY_RUN
        
        # Initialize adapters
        execution = BinanceExecution()
        market_data = BinanceMarketData()
        pnl_repo = DjangoPnLRepository()
        
        # Get entry price if not provided
        if not entry_price:
            entry_price = market_data.best_bid(symbol)
        
        # Convert to Decimal
        try:
            entry_price = Decimal(str(entry_price))
            stop_price = Decimal(str(stop_price)) if stop_price else entry_price  # For closing
            capital = Decimal(str(capital))
            quantity = Decimal(str(quantity))
        except (InvalidOperation, ValueError) as e:
            return Response({
                'success': False,
                'error': f'Invalid number format: {e}',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Get monthly P&L
        monthly_pnl = _get_monthly_pnl()
        
        # Create and execute use case
        use_case = RiskManagedTradeUseCase(
            execution_adapter=execution,
            pnl_repository=pnl_repo,
        )
        
        result = use_case.execute_sell(
            symbol=symbol,
            quantity=quantity,
            capital=capital,
            entry_price=entry_price,
            stop_price=stop_price,
            mode=mode,
            monthly_pnl=monthly_pnl,
            strategy_name=strategy_name,
        )
        
        # Format response
        response_data = {
            'success': result.is_success(),
            'execution': result.to_dict(),
            'mode': mode.value,
            'environment': 'production' if not execution.use_testnet else 'testnet',
            'timestamp': timezone.now().isoformat(),
        }
        
        if result.is_blocked():
            response_data['blocked'] = True
            response_data['blocked_by'] = result.error
            return Response(response_data, status=status.HTTP_403_FORBIDDEN)
        
        if result.status == ExecutionStatus.FAILED:
            return Response(response_data, status=status.HTTP_500_INTERNAL_SERVER_ERROR)
        
        return Response(response_data)
        
    except Exception as e:
        logger.error(f"Risk-managed sell failed: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def risk_status(request):
    """
    Get current risk management status.
    
    Shows:
    - Monthly P&L and drawdown status
    - Whether trading is allowed
    - Risk limits in effect
    """
    try:
        monthly_pnl = _get_monthly_pnl()
        
        # Get capital from request or use a default for display
        capital = Decimal(request.query_params.get('capital', '1000'))
        
        # Calculate drawdown percentage
        if monthly_pnl < 0:
            drawdown_percent = (abs(monthly_pnl) / capital) * Decimal("100")
        else:
            drawdown_percent = Decimal("0")
        
        max_drawdown = Decimal("4.0")
        is_trading_allowed = drawdown_percent < max_drawdown
        
        return Response({
            'success': True,
            'risk_status': {
                'monthly_pnl': str(monthly_pnl),
                'is_profitable': monthly_pnl >= 0,
                'drawdown_percent': str(drawdown_percent),
                'max_drawdown_percent': str(max_drawdown),
                'remaining_drawdown': str(max_drawdown - drawdown_percent),
                'is_trading_allowed': is_trading_allowed,
            },
            'rules': {
                'max_risk_per_trade': '1% of capital',
                'max_monthly_drawdown': '4% of capital',
                'stop_loss': 'REQUIRED for every trade',
            },
            'trading_enabled': getattr(settings, 'TRADING_ENABLED', False),
            'environment': 'production' if not getattr(settings, 'BINANCE_USE_TESTNET', True) else 'testnet',
            'timestamp': timezone.now().isoformat(),
        })
        
    except Exception as e:
        logger.error(f"Risk status check failed: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


"""
Trading views for executing real trades and managing positions.

This module provides endpoints for:
- Executing spot trades (buy/sell)
- Viewing trade history
- P&L tracking and analysis

IMPORTANT: These endpoints can execute REAL trades with REAL money
when BINANCE_USE_TESTNET=False. Use with caution.
"""

import logging
from decimal import Decimal, InvalidOperation
from datetime import datetime, timedelta

from django.conf import settings
from django.db import transaction
from django.utils import timezone
from rest_framework import status
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response

from api.models import Trade, Symbol, Order
from api.application.adapters import BinanceExecution, BinanceMarketData

logger = logging.getLogger(__name__)


def _get_or_create_symbol(pair: str) -> Symbol:
    """Get or create a Symbol instance for the given trading pair."""
    # Parse pair (assumes format like BTCUSDC)
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


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def trading_status(request):
    """
    Get current trading configuration status.
    
    Returns:
        - trading_enabled: Whether trading is enabled in settings
        - environment: 'production' or 'testnet'
        - can_trade: Whether the current setup allows trading
    """
    is_testnet = getattr(settings, 'BINANCE_USE_TESTNET', True)
    trading_enabled = getattr(settings, 'TRADING_ENABLED', False)
    
    # Check if credentials are configured
    if is_testnet:
        has_credentials = bool(settings.BINANCE_API_KEY_TEST and settings.BINANCE_SECRET_KEY_TEST)
    else:
        has_credentials = bool(settings.BINANCE_API_KEY and settings.BINANCE_SECRET_KEY)
    
    return Response({
        'trading_enabled': trading_enabled,
        'environment': 'testnet' if is_testnet else 'production',
        'has_credentials': has_credentials,
        'can_trade': trading_enabled and has_credentials,
        'user': request.user.username,
        'timestamp': timezone.now().isoformat(),
    })


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def account_balance(request):
    """
    Get account balance for all assets or a specific asset.
    
    Query params:
        - asset: Optional specific asset to query (e.g., 'USDC', 'BTC')
    """
    asset = request.query_params.get('asset', None)
    
    try:
        execution = BinanceExecution()
        balance_data = execution.get_account_balance(asset)
        
        return Response({
            'success': True,
            'environment': 'testnet' if execution.use_testnet else 'production',
            'data': balance_data,
            'timestamp': timezone.now().isoformat(),
        })
    except Exception as e:
        logger.error(f"Failed to get account balance: {e}")
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def buy_btc(request):
    """
    Execute a market buy order for BTC using USDC.
    
    This is the historic first production trade endpoint for Robson!
    
    Request body:
        - amount: Amount of USDC to spend (optional, defaults to all available)
        - quantity: BTC quantity to buy (alternative to amount)
    
    Returns:
        - Trade record with execution details
        - P&L tracking data
    """
    # Safety checks
    trading_enabled = getattr(settings, 'TRADING_ENABLED', False)
    if not trading_enabled:
        return Response({
            'success': False,
            'error': 'Trading is disabled. Set TRADING_ENABLED=True to enable.',
        }, status=status.HTTP_403_FORBIDDEN)
    
    try:
        # Parse request
        amount_usdc = request.data.get('amount')
        quantity_btc = request.data.get('quantity')
        
        # Initialize adapters
        execution = BinanceExecution()
        market_data = BinanceMarketData()
        
        env = 'PRODUCTION' if not execution.use_testnet else 'TESTNET'
        logger.info(f"🚀 Executing BTC buy order in {env} mode")
        
        # Get current USDC balance
        usdc_balance = execution.get_account_balance('USDC')
        available_usdc = usdc_balance.get('free', Decimal('0'))
        
        if available_usdc <= 0:
            return Response({
                'success': False,
                'error': 'No USDC available for trading',
                'balance': str(available_usdc),
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Determine quantity to buy
        if quantity_btc:
            try:
                btc_qty = Decimal(str(quantity_btc))
            except (InvalidOperation, ValueError):
                return Response({
                    'success': False,
                    'error': 'Invalid quantity specified',
                }, status=status.HTTP_400_BAD_REQUEST)
        else:
            # Calculate quantity based on amount or use all available
            if amount_usdc:
                try:
                    usdc_to_spend = min(Decimal(str(amount_usdc)), available_usdc)
                except (InvalidOperation, ValueError):
                    return Response({
                        'success': False,
                        'error': 'Invalid amount specified',
                    }, status=status.HTTP_400_BAD_REQUEST)
            else:
                usdc_to_spend = available_usdc
            
            # Get current BTC price
            btc_price = market_data.best_ask('BTCUSDC')
            
            # Calculate BTC quantity (with buffer for fees)
            # Binance spot fee is typically 0.1%, we use 0.2% buffer
            fee_buffer = Decimal('0.998')
            btc_qty = (usdc_to_spend * fee_buffer / btc_price).quantize(Decimal('0.00001'))
        
        if btc_qty <= 0:
            return Response({
                'success': False,
                'error': 'Calculated quantity is too small',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Get price for record keeping
        entry_price = market_data.best_ask('BTCUSDC')
        
        # Execute the trade
        logger.info(f"Executing market buy: {btc_qty} BTC @ ~{entry_price} USDC")
        
        with transaction.atomic():
            # Place market order
            order_response = execution.place_market(
                symbol='BTCUSDC',
                side='BUY',
                quantity=btc_qty,
            )
            
            # Extract execution details
            order_id = order_response.get('orderId')
            executed_qty = Decimal(order_response.get('executedQty', '0'))
            
            # Calculate average fill price from fills
            fills = order_response.get('fills', [])
            if fills:
                total_cost = sum(Decimal(f['price']) * Decimal(f['qty']) for f in fills)
                total_qty = sum(Decimal(f['qty']) for f in fills)
                avg_price = total_cost / total_qty if total_qty > 0 else entry_price
                total_fee = sum(Decimal(f.get('commission', '0')) for f in fills)
            else:
                avg_price = entry_price
                total_fee = Decimal('0')
            
            # Get or create symbol
            symbol = _get_or_create_symbol('BTCUSDC')
            
            # Record the trade
            trade = Trade.objects.create(
                symbol=symbol,
                side='BUY',
                quantity=executed_qty,
                entry_price=avg_price,
                entry_fee=total_fee,
                entry_time=timezone.now(),
            )
            
            # Also record as Order for tracking
            Order.objects.create(
                symbol=symbol,
                side='BUY',
                order_type='MARKET',
                quantity=executed_qty,
                filled_quantity=executed_qty,
                avg_fill_price=avg_price,
                status='FILLED',
                binance_order_id=str(order_id),
            )
            
            logger.info(f"✅ Trade executed successfully: {trade.id}")
            
            return Response({
                'success': True,
                'message': '🎉 Historic first production trade executed!',
                'environment': 'production' if not execution.use_testnet else 'testnet',
                'trade': {
                    'id': trade.id,
                    'symbol': 'BTCUSDC',
                    'side': 'BUY',
                    'quantity': str(executed_qty),
                    'price': str(avg_price),
                    'total_cost': str(executed_qty * avg_price),
                    'fee': str(total_fee),
                    'binance_order_id': str(order_id),
                    'timestamp': trade.entry_time.isoformat(),
                },
                'order_response': order_response,
            })
            
    except Exception as e:
        logger.error(f"Trade execution failed: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
            'environment': 'production' if not getattr(settings, 'BINANCE_USE_TESTNET', True) else 'testnet',
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def sell_btc(request):
    """
    Execute a market sell order for BTC to USDC.
    
    Request body:
        - quantity: BTC quantity to sell (optional, defaults to all available)
    
    Returns:
        - Trade record with execution details
        - P&L calculation if closing a previous buy
    """
    trading_enabled = getattr(settings, 'TRADING_ENABLED', False)
    if not trading_enabled:
        return Response({
            'success': False,
            'error': 'Trading is disabled. Set TRADING_ENABLED=True to enable.',
        }, status=status.HTTP_403_FORBIDDEN)
    
    try:
        quantity_btc = request.data.get('quantity')
        
        execution = BinanceExecution()
        market_data = BinanceMarketData()
        
        env = 'PRODUCTION' if not execution.use_testnet else 'TESTNET'
        logger.info(f"Executing BTC sell order in {env} mode")
        
        # Get current BTC balance
        btc_balance = execution.get_account_balance('BTC')
        available_btc = btc_balance.get('free', Decimal('0'))
        
        if available_btc <= 0:
            return Response({
                'success': False,
                'error': 'No BTC available for selling',
                'balance': str(available_btc),
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Determine quantity to sell
        if quantity_btc:
            try:
                btc_qty = min(Decimal(str(quantity_btc)), available_btc)
            except (InvalidOperation, ValueError):
                return Response({
                    'success': False,
                    'error': 'Invalid quantity specified',
                }, status=status.HTTP_400_BAD_REQUEST)
        else:
            btc_qty = available_btc
        
        # Quantize to valid precision
        btc_qty = btc_qty.quantize(Decimal('0.00001'))
        
        if btc_qty <= 0:
            return Response({
                'success': False,
                'error': 'Quantity is too small',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        exit_price = market_data.best_bid('BTCUSDC')
        
        logger.info(f"Executing market sell: {btc_qty} BTC @ ~{exit_price} USDC")
        
        with transaction.atomic():
            order_response = execution.place_market(
                symbol='BTCUSDC',
                side='SELL',
                quantity=btc_qty,
            )
            
            order_id = order_response.get('orderId')
            executed_qty = Decimal(order_response.get('executedQty', '0'))
            
            fills = order_response.get('fills', [])
            if fills:
                total_proceeds = sum(Decimal(f['price']) * Decimal(f['qty']) for f in fills)
                total_qty = sum(Decimal(f['qty']) for f in fills)
                avg_price = total_proceeds / total_qty if total_qty > 0 else exit_price
                total_fee = sum(Decimal(f.get('commission', '0')) for f in fills)
            else:
                avg_price = exit_price
                total_fee = Decimal('0')
            
            symbol = _get_or_create_symbol('BTCUSDC')
            
            # Try to find and close matching open trade
            open_trade = Trade.objects.filter(
                symbol=symbol,
                side='BUY',
                exit_price__isnull=True
            ).order_by('entry_time').first()
            
            if open_trade:
                # Close existing trade
                open_trade.exit_price = avg_price
                open_trade.exit_fee = total_fee
                open_trade.exit_time = timezone.now()
                open_trade.save()
                
                trade = open_trade
                pnl = trade.pnl
                pnl_percent = trade.pnl_percentage
            else:
                # Create new sell trade
                trade = Trade.objects.create(
                    symbol=symbol,
                    side='SELL',
                    quantity=executed_qty,
                    entry_price=avg_price,
                    entry_fee=total_fee,
                    entry_time=timezone.now(),
                )
                pnl = None
                pnl_percent = None
            
            Order.objects.create(
                symbol=symbol,
                side='SELL',
                order_type='MARKET',
                quantity=executed_qty,
                filled_quantity=executed_qty,
                avg_fill_price=avg_price,
                status='FILLED',
                binance_order_id=str(order_id),
            )
            
            logger.info(f"✅ Sell order executed: {trade.id}, PnL: {pnl}")
            
            return Response({
                'success': True,
                'environment': 'production' if not execution.use_testnet else 'testnet',
                'trade': {
                    'id': trade.id,
                    'symbol': 'BTCUSDC',
                    'side': 'SELL',
                    'quantity': str(executed_qty),
                    'price': str(avg_price),
                    'total_proceeds': str(executed_qty * avg_price),
                    'fee': str(total_fee),
                    'pnl': str(pnl) if pnl else None,
                    'pnl_percentage': str(pnl_percent) if pnl_percent else None,
                    'binance_order_id': str(order_id),
                    'timestamp': timezone.now().isoformat(),
                },
                'order_response': order_response,
            })
            
    except Exception as e:
        logger.error(f"Sell order failed: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def trade_history(request):
    """
    Get trade history with P&L information.
    
    Query params:
        - symbol: Filter by trading pair (e.g., 'BTCUSDC')
        - days: Number of days to look back (default: 30)
        - limit: Maximum number of trades to return (default: 100)
    """
    symbol_filter = request.query_params.get('symbol')
    days = int(request.query_params.get('days', 30))
    limit = int(request.query_params.get('limit', 100))
    
    since = timezone.now() - timedelta(days=days)
    
    queryset = Trade.objects.filter(entry_time__gte=since).order_by('-entry_time')
    
    if symbol_filter:
        queryset = queryset.filter(symbol__name=symbol_filter)
    
    trades = queryset[:limit]
    
    # Calculate summary stats
    closed_trades = [t for t in trades if t.is_closed]
    total_pnl = sum(t.pnl for t in closed_trades if t.pnl)
    winners = len([t for t in closed_trades if t.is_winner])
    losers = len(closed_trades) - winners
    win_rate = (winners / len(closed_trades) * 100) if closed_trades else 0
    
    return Response({
        'trades': [
            {
                'id': t.id,
                'symbol': t.symbol.name,
                'side': t.side,
                'quantity': str(t.quantity),
                'entry_price': str(t.entry_price),
                'exit_price': str(t.exit_price) if t.exit_price else None,
                'pnl': str(t.pnl) if t.pnl else None,
                'pnl_percentage': str(t.pnl_percentage) if t.pnl_percentage else None,
                'is_winner': t.is_winner if t.is_closed else None,
                'is_closed': t.is_closed,
                'duration_hours': t.duration_hours,
                'entry_time': t.entry_time.isoformat(),
                'exit_time': t.exit_time.isoformat() if t.exit_time else None,
                'entry_fee': str(t.entry_fee),
                'exit_fee': str(t.exit_fee) if t.exit_fee else None,
            }
            for t in trades
        ],
        'summary': {
            'total_trades': len(trades),
            'closed_trades': len(closed_trades),
            'open_trades': len(trades) - len(closed_trades),
            'total_pnl': str(total_pnl),
            'winners': winners,
            'losers': losers,
            'win_rate': f'{win_rate:.1f}%',
            'period_days': days,
        },
        'timestamp': timezone.now().isoformat(),
    })


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def pnl_summary(request):
    """
    Get P&L summary by period.
    
    Query params:
        - period: 'daily', 'weekly', 'monthly', 'yearly' (default: monthly)
        - months: Number of months to analyze (default: 12)
    """
    period = request.query_params.get('period', 'monthly')
    months = int(request.query_params.get('months', 12))
    
    since = timezone.now() - timedelta(days=months * 30)
    
    closed_trades = Trade.objects.filter(
        exit_price__isnull=False,
        entry_time__gte=since
    ).order_by('entry_time')
    
    # Group by period
    periods = {}
    for trade in closed_trades:
        if period == 'daily':
            key = trade.entry_time.strftime('%Y-%m-%d')
        elif period == 'weekly':
            key = trade.entry_time.strftime('%Y-W%W')
        elif period == 'yearly':
            key = trade.entry_time.strftime('%Y')
        else:  # monthly
            key = trade.entry_time.strftime('%Y-%m')
        
        if key not in periods:
            periods[key] = {
                'pnl': Decimal('0'),
                'trades': 0,
                'winners': 0,
                'volume': Decimal('0'),
            }
        
        periods[key]['pnl'] += trade.pnl or Decimal('0')
        periods[key]['trades'] += 1
        periods[key]['volume'] += trade.quantity * trade.entry_price
        if trade.is_winner:
            periods[key]['winners'] += 1
    
    # Calculate percentages and format output
    result = []
    cumulative_pnl = Decimal('0')
    
    for key in sorted(periods.keys()):
        p = periods[key]
        cumulative_pnl += p['pnl']
        win_rate = (p['winners'] / p['trades'] * 100) if p['trades'] > 0 else 0
        
        result.append({
            'period': key,
            'pnl': str(p['pnl']),
            'cumulative_pnl': str(cumulative_pnl),
            'trades': p['trades'],
            'winners': p['winners'],
            'losers': p['trades'] - p['winners'],
            'win_rate': f'{win_rate:.1f}%',
            'volume': str(p['volume']),
        })
    
    # Calculate overall stats
    total_pnl = sum(t.pnl for t in closed_trades if t.pnl)
    total_trades = len(closed_trades)
    total_winners = len([t for t in closed_trades if t.is_winner])
    
    return Response({
        'period_type': period,
        'periods': result,
        'overall': {
            'total_pnl': str(total_pnl),
            'total_trades': total_trades,
            'winners': total_winners,
            'losers': total_trades - total_winners,
            'win_rate': f'{(total_winners / total_trades * 100):.1f}%' if total_trades > 0 else '0%',
            'analyzed_months': months,
        },
        'timestamp': timezone.now().isoformat(),
    })


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def calculate_position_size(request):
    """
    Calculate optimal position size based on the 1% risk rule.
    
    The 1% Rule: Never risk more than 1% of capital on a single trade.
    
    Request body:
        - capital: Total available capital in quote currency (required)
        - entry_price: Entry price for the trade (required)
        - stop_loss_percent: Stop loss distance as % (default: 2)
        - take_profit_percent: Take profit distance as % (default: 4)
        - side: "BUY" or "SELL" (default: "BUY")
        - max_risk_percent: Max risk per trade as % (default: 1)
        - max_position_percent: Max position size as % (default: 50)
    
    Returns:
        Position sizing calculation with:
        - quantity: Calculated position size
        - position_value: Total value of position
        - risk_amount: Maximum loss if stopped
        - risk_percent: Risk as % of capital
        - stop_loss_price: Calculated stop loss price
        - take_profit_price: Calculated take profit price
        - risk_reward_ratio: Reward/Risk ratio
    
    Example:
        POST /api/trade/position-size/
        {
            "capital": 1000,
            "entry_price": 90000,
            "stop_loss_percent": 2,
            "take_profit_percent": 4
        }
    """
    from api.application.risk import PositionSizingCalculator
    
    try:
        # Parse request data
        capital = request.data.get('capital')
        entry_price = request.data.get('entry_price')
        
        if not capital or not entry_price:
            return Response({
                'success': False,
                'error': 'capital and entry_price are required',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Convert to Decimal
        try:
            capital = Decimal(str(capital))
            entry_price = Decimal(str(entry_price))
        except (InvalidOperation, ValueError) as e:
            return Response({
                'success': False,
                'error': f'Invalid number format: {e}',
            }, status=status.HTTP_400_BAD_REQUEST)
        
        # Optional parameters
        stop_loss_percent = Decimal(str(request.data.get('stop_loss_percent', 2)))
        take_profit_percent = Decimal(str(request.data.get('take_profit_percent', 4)))
        max_risk_percent = Decimal(str(request.data.get('max_risk_percent', 1)))
        max_position_percent = Decimal(str(request.data.get('max_position_percent', 50)))
        side = request.data.get('side', 'BUY').upper()
        
        # Create calculator with parameters
        calculator = PositionSizingCalculator(
            max_risk_percent=max_risk_percent,
            max_position_percent=max_position_percent,
        )
        
        # Calculate position size
        result = calculator.calculate(
            capital=capital,
            entry_price=entry_price,
            stop_loss_percent=stop_loss_percent,
            take_profit_percent=take_profit_percent,
            side=side,
        )
        
        return Response({
            'success': True,
            'calculation': result.to_dict(),
            'summary': {
                'message': f"Buy {result.quantity} at {result.entry_price}",
                'risk': f"${result.risk_amount} ({result.risk_percent}% of capital)",
                'stop_loss': f"${result.stop_loss_price} (-{result.stop_distance_percent}%)",
                'take_profit': f"${result.take_profit_price} (+{result.target_distance_percent}%)" if result.take_profit_price else None,
                'risk_reward': f"1:{result.risk_reward_ratio}" if result.risk_reward_ratio else None,
                'is_capped': result.is_capped,
            },
            'timestamp': timezone.now().isoformat(),
        })
        
    except ValueError as e:
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_400_BAD_REQUEST)
    except Exception as e:
        logger.error(f"Position sizing calculation failed: {e}", exc_info=True)
        return Response({
            'success': False,
            'error': str(e),
        }, status=status.HTTP_500_INTERNAL_SERVER_ERROR)

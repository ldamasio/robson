"""
Audit Service - Records and syncs all transactions.

This service ensures complete transparency by:
1. Recording every Robson operation to the audit trail
2. Syncing with Binance to capture external transactions
3. Providing a complete view of all account activity
"""

import logging
import uuid
from decimal import Decimal
from typing import Optional
from datetime import datetime, timedelta

from django.db import transaction
from django.utils import timezone

from api.models.audit import AuditTransaction, BalanceSnapshot, TransactionType, TransactionStatus
from api.application.adapters import BinanceExecution
from clients.models import Client

logger = logging.getLogger(__name__)


class AuditService:
    """
    Service for recording and managing audit transactions.
    
    Every operation goes through this service to ensure complete auditability.
    """
    
    def __init__(self, client: Client, execution: Optional[BinanceExecution] = None):
        self.client = client
        self.execution = execution or BinanceExecution()
    
    def record_spot_buy(
        self,
        symbol: str,
        quantity: Decimal,
        price: Decimal,
        binance_order_id: str,
        fee: Decimal = Decimal("0"),
        fee_asset: str = "USDC",
        stop_price: Optional[Decimal] = None,
        risk_amount: Optional[Decimal] = None,
        risk_percent: Optional[Decimal] = None,
        raw_response: Optional[dict] = None,
    ) -> AuditTransaction:
        """Record a spot buy order."""
        return self._create_transaction(
            transaction_type=TransactionType.SPOT_BUY,
            symbol=symbol,
            asset=symbol[:-4],  # e.g., BTC from BTCUSDC
            quantity=quantity,
            price=price,
            side="BUY",
            binance_order_id=binance_order_id,
            fee=fee,
            fee_asset=fee_asset,
            stop_price=stop_price,
            risk_amount=risk_amount,
            risk_percent=risk_percent,
            raw_response=raw_response,
            description=f"Spot buy {quantity} {symbol[:-4]} @ ${price}",
        )
    
    def record_spot_sell(
        self,
        symbol: str,
        quantity: Decimal,
        price: Decimal,
        binance_order_id: str,
        fee: Decimal = Decimal("0"),
        fee_asset: str = "USDC",
        raw_response: Optional[dict] = None,
    ) -> AuditTransaction:
        """Record a spot sell order."""
        return self._create_transaction(
            transaction_type=TransactionType.SPOT_SELL,
            symbol=symbol,
            asset=symbol[:-4],
            quantity=quantity,
            price=price,
            side="SELL",
            binance_order_id=binance_order_id,
            fee=fee,
            fee_asset=fee_asset,
            raw_response=raw_response,
            description=f"Spot sell {quantity} {symbol[:-4]} @ ${price}",
        )
    
    def record_margin_buy(
        self,
        symbol: str,
        quantity: Decimal,
        price: Decimal,
        binance_order_id: str,
        leverage: int = 1,
        stop_price: Optional[Decimal] = None,
        risk_amount: Optional[Decimal] = None,
        risk_percent: Optional[Decimal] = None,
        position=None,
        raw_response: Optional[dict] = None,
    ) -> AuditTransaction:
        """Record a margin buy order."""
        return self._create_transaction(
            transaction_type=TransactionType.MARGIN_BUY,
            symbol=symbol,
            asset=symbol[:-4],
            quantity=quantity,
            price=price,
            side="BUY",
            binance_order_id=binance_order_id,
            leverage=leverage,
            is_isolated_margin=True,
            stop_price=stop_price,
            risk_amount=risk_amount,
            risk_percent=risk_percent,
            related_position=position,
            raw_response=raw_response,
            description=f"Margin buy {quantity} {symbol[:-4]} @ ${price} ({leverage}x leverage)",
        )
    
    def record_margin_borrow(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
        binance_transaction_id: str,
        raw_response: Optional[dict] = None,
    ) -> AuditTransaction:
        """Record a margin borrow."""
        return self._create_transaction(
            transaction_type=TransactionType.MARGIN_BORROW,
            symbol=symbol,
            asset=asset,
            quantity=amount,
            price=None,
            binance_order_id=binance_transaction_id,
            is_isolated_margin=True,
            raw_response=raw_response,
            description=f"Borrowed {amount} {asset} for isolated margin",
        )
    
    def record_transfer_to_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
        binance_transaction_id: str,
        raw_response: Optional[dict] = None,
    ) -> AuditTransaction:
        """Record a transfer from spot to margin."""
        return self._create_transaction(
            transaction_type=TransactionType.TRANSFER_TO_MARGIN,
            symbol=symbol,
            asset=asset,
            quantity=amount,
            price=None,
            binance_order_id=binance_transaction_id,
            is_isolated_margin=True,
            raw_response=raw_response,
            description=f"Transfer {amount} {asset} from Spot to Isolated Margin ({symbol})",
        )
    
    def record_transfer_from_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
        binance_transaction_id: str,
        raw_response: Optional[dict] = None,
    ) -> AuditTransaction:
        """Record a transfer from margin to spot."""
        return self._create_transaction(
            transaction_type=TransactionType.TRANSFER_FROM_MARGIN,
            symbol=symbol,
            asset=asset,
            quantity=amount,
            price=None,
            binance_order_id=binance_transaction_id,
            is_isolated_margin=True,
            raw_response=raw_response,
            description=f"Transfer {amount} {asset} from Isolated Margin ({symbol}) to Spot",
        )
    
    def record_stop_loss_placed(
        self,
        symbol: str,
        quantity: Decimal,
        stop_price: Decimal,
        binance_order_id: str,
        is_margin: bool = False,
        position=None,
        raw_response: Optional[dict] = None,
    ) -> AuditTransaction:
        """Record a stop-loss order placement."""
        return self._create_transaction(
            transaction_type=TransactionType.STOP_LOSS_PLACED,
            symbol=symbol,
            asset=symbol[:-4],
            quantity=quantity,
            price=stop_price,
            stop_price=stop_price,
            side="SELL",
            binance_order_id=binance_order_id,
            is_isolated_margin=is_margin,
            related_position=position,
            raw_response=raw_response,
            description=f"Stop-loss placed: Sell {quantity} {symbol[:-4]} if price drops to ${stop_price}",
        )
    
    def _create_transaction(
        self,
        transaction_type: str,
        symbol: str,
        asset: str,
        quantity: Decimal,
        description: str,
        price: Optional[Decimal] = None,
        side: Optional[str] = None,
        binance_order_id: Optional[str] = None,
        fee: Decimal = Decimal("0"),
        fee_asset: Optional[str] = None,
        leverage: Optional[int] = None,
        is_isolated_margin: bool = False,
        stop_price: Optional[Decimal] = None,
        risk_amount: Optional[Decimal] = None,
        risk_percent: Optional[Decimal] = None,
        related_position=None,
        raw_response: Optional[dict] = None,
    ) -> AuditTransaction:
        """Create an audit transaction."""
        
        total_value = None
        if price and quantity:
            total_value = price * quantity
        
        with transaction.atomic():
            audit_tx = AuditTransaction.objects.create(
                transaction_id=str(uuid.uuid4()),
                binance_order_id=binance_order_id,
                client=self.client,
                transaction_type=transaction_type,
                status=TransactionStatus.FILLED,
                symbol=symbol,
                asset=asset,
                quantity=quantity,
                price=price,
                total_value=total_value,
                fee=fee,
                fee_asset=fee_asset,
                side=side,
                leverage=leverage,
                is_isolated_margin=is_isolated_margin,
                stop_price=stop_price,
                risk_amount=risk_amount,
                risk_percent=risk_percent,
                related_position=related_position,
                description=description,
                raw_response=raw_response,
                executed_at=timezone.now(),
                source="robson",
            )
            
            logger.info(f"Recorded audit transaction: {audit_tx}")
            return audit_tx
    
    def sync_from_binance(self, days_back: int = 7) -> int:
        """
        Sync transactions from Binance that might have been missed.
        
        Returns the number of new transactions synced.
        """
        count = 0
        
        # Sync spot trades
        count += self._sync_spot_trades(days_back)
        
        # Sync margin trades
        count += self._sync_margin_trades(days_back)
        
        # Sync transfers
        count += self._sync_transfers(days_back)
        
        logger.info(f"Synced {count} transactions from Binance")
        return count
    
    def _sync_spot_trades(self, days_back: int) -> int:
        """Sync spot trades from Binance."""
        count = 0
        
        try:
            # Get recent trades from Binance
            trades = self.execution.client.get_my_trades(
                symbol="BTCUSDC",
                limit=100,
            )
            
            for trade in trades:
                order_id = str(trade.get('orderId', ''))
                
                # Skip if already recorded
                if AuditTransaction.objects.filter(binance_order_id=order_id).exists():
                    continue
                
                # Record the trade
                qty = Decimal(str(trade.get('qty', '0')))
                price = Decimal(str(trade.get('price', '0')))
                is_buyer = trade.get('isBuyer', False)
                commission = Decimal(str(trade.get('commission', '0')))
                commission_asset = trade.get('commissionAsset', 'USDC')
                
                tx_type = TransactionType.SPOT_BUY if is_buyer else TransactionType.SPOT_SELL
                
                AuditTransaction.objects.create(
                    transaction_id=str(uuid.uuid4()),
                    binance_order_id=order_id,
                    client=self.client,
                    transaction_type=tx_type,
                    status=TransactionStatus.FILLED,
                    symbol="BTCUSDC",
                    asset="BTC",
                    quantity=qty,
                    price=price,
                    total_value=qty * price,
                    fee=commission,
                    fee_asset=commission_asset,
                    side="BUY" if is_buyer else "SELL",
                    description=f"Synced from Binance: {'Buy' if is_buyer else 'Sell'} {qty} BTC @ ${price}",
                    raw_response=trade,
                    executed_at=timezone.now(),
                    source="binance_sync",
                )
                count += 1
                
        except Exception as e:
            logger.error(f"Failed to sync spot trades: {e}")
        
        return count
    
    def _sync_margin_trades(self, days_back: int) -> int:
        """Sync margin trades from Binance."""
        count = 0
        
        try:
            # Get margin trades
            trades = self.execution.client.get_margin_trades(
                symbol="BTCUSDC",
                isIsolated="TRUE",
            )
            
            for trade in trades:
                order_id = str(trade.get('orderId', ''))
                
                if AuditTransaction.objects.filter(binance_order_id=order_id).exists():
                    continue
                
                qty = Decimal(str(trade.get('qty', '0')))
                price = Decimal(str(trade.get('price', '0')))
                is_buyer = trade.get('isBuyer', False)
                commission = Decimal(str(trade.get('commission', '0')))
                
                tx_type = TransactionType.MARGIN_BUY if is_buyer else TransactionType.MARGIN_SELL
                
                AuditTransaction.objects.create(
                    transaction_id=str(uuid.uuid4()),
                    binance_order_id=order_id,
                    client=self.client,
                    transaction_type=tx_type,
                    status=TransactionStatus.FILLED,
                    symbol="BTCUSDC",
                    asset="BTC",
                    quantity=qty,
                    price=price,
                    total_value=qty * price,
                    fee=commission,
                    side="BUY" if is_buyer else "SELL",
                    is_isolated_margin=True,
                    description=f"Synced from Binance: Margin {'Buy' if is_buyer else 'Sell'} {qty} BTC @ ${price}",
                    raw_response=trade,
                    executed_at=timezone.now(),
                    source="binance_sync",
                )
                count += 1
                
        except Exception as e:
            logger.error(f"Failed to sync margin trades: {e}")
        
        return count
    
    def _sync_transfers(self, days_back: int) -> int:
        """Sync transfers from Binance."""
        count = 0
        
        try:
            # Get transfer history
            # Note: Binance API for transfer history is limited
            # We may need to use different endpoints depending on API version
            pass
        except Exception as e:
            logger.error(f"Failed to sync transfers: {e}")
        
        return count
    
    def take_balance_snapshot(self) -> BalanceSnapshot:
        """Take a snapshot of current balances."""
        
        # Get spot balances
        spot_usdc = self.execution.get_account_balance('USDC')
        spot_btc = self.execution.get_account_balance('BTC')
        
        spot_usdc_free = Decimal(spot_usdc.get('free', '0'))
        spot_btc_free = Decimal(spot_btc.get('free', '0'))
        
        # Get margin balances
        margin_info = self.execution.client.get_isolated_margin_account(symbols='BTCUSDC')
        assets = margin_info.get('assets', [])
        
        margin_btc_free = Decimal('0')
        margin_btc_borrowed = Decimal('0')
        margin_usdc_free = Decimal('0')
        margin_usdc_borrowed = Decimal('0')
        margin_level = None
        btc_price = Decimal('0')
        
        if assets:
            base = assets[0].get('baseAsset', {})
            quote = assets[0].get('quoteAsset', {})
            margin_btc_free = Decimal(base.get('free', '0'))
            margin_btc_borrowed = Decimal(base.get('borrowed', '0'))
            margin_usdc_free = Decimal(quote.get('free', '0'))
            margin_usdc_borrowed = Decimal(quote.get('borrowed', '0'))
            margin_level = Decimal(assets[0].get('marginLevel', '0'))
            btc_price = Decimal(assets[0].get('indexPrice', '0'))
        
        # Calculate total equity
        total_btc = spot_btc_free + margin_btc_free
        total_btc_value = total_btc * btc_price
        total_equity = spot_usdc_free + margin_usdc_free + total_btc_value - margin_usdc_borrowed
        
        snapshot = BalanceSnapshot.objects.create(
            client=self.client,
            snapshot_time=timezone.now(),
            spot_usdc=spot_usdc_free,
            spot_btc=spot_btc_free,
            margin_btc_free=margin_btc_free,
            margin_btc_borrowed=margin_btc_borrowed,
            margin_usdc_free=margin_usdc_free,
            margin_usdc_borrowed=margin_usdc_borrowed,
            btc_price=btc_price,
            total_equity=total_equity,
            margin_level=margin_level,
        )
        
        logger.info(f"Balance snapshot taken: ${total_equity}")
        return snapshot


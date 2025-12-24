"""
Binance Isolated Margin Adapter

Implements MarginExecutionPort for Binance exchange.
Handles all margin-specific operations including transfers, orders, and monitoring.

WARNING: When BINANCE_USE_TESTNET=False, this operates with REAL money!
"""

from __future__ import annotations
from decimal import Decimal
from typing import Optional
import logging

from django.conf import settings
from binance.client import Client
from binance.exceptions import BinanceAPIException
from dataclasses import dataclass
from typing import Protocol

logger = logging.getLogger(__name__)


# ============================================================================
# Local Port Definitions (to avoid container import path issues)
# ============================================================================

@dataclass(frozen=True)
class MarginTransferResult:
    """Result of a margin transfer operation."""
    success: bool
    transaction_id: Optional[str]
    asset: str
    amount: Decimal
    from_account: str
    to_account: str
    error_message: Optional[str] = None


@dataclass(frozen=True)
class MarginAccountSnapshot:
    """Snapshot of Isolated Margin account for a symbol."""
    symbol: str
    base_asset: str
    base_free: Decimal
    base_locked: Decimal
    base_borrowed: Decimal
    quote_asset: str
    quote_free: Decimal
    quote_locked: Decimal
    quote_borrowed: Decimal
    margin_level: Decimal
    liquidation_price: Decimal
    is_margin_trade_enabled: bool


@dataclass(frozen=True)
class MarginOrderExecutionResult:
    """Result of margin order execution."""
    success: bool
    order_id: Optional[str]
    binance_order_id: Optional[str]
    symbol: str
    side: str
    order_type: str
    quantity: Decimal
    price: Optional[Decimal]
    filled_quantity: Decimal
    avg_fill_price: Optional[Decimal]
    status: str
    error_message: Optional[str] = None


class MarginExecutionPort(Protocol):
    """Port for Isolated Margin trading operations."""
    
    def transfer_to_margin(self, symbol: str, asset: str, amount: Decimal) -> MarginTransferResult:
        ...
    
    def transfer_from_margin(self, symbol: str, asset: str, amount: Decimal) -> MarginTransferResult:
        ...
    
    def get_margin_account(self, symbol: str) -> MarginAccountSnapshot:
        ...
    
    def place_margin_order(self, symbol: str, side: str, order_type: str, quantity: Decimal, 
                          price: Optional[Decimal] = None, stop_price: Optional[Decimal] = None) -> MarginOrderExecutionResult:
        ...


def _get_binance_client(use_testnet: bool = None) -> Client:
    """
    Create a Binance client with appropriate credentials.
    
    Args:
        use_testnet: Override testnet setting. If None, uses settings.BINANCE_USE_TESTNET
        
    Returns:
        Configured Binance Client instance
    """
    if use_testnet is None:
        use_testnet = getattr(settings, 'BINANCE_USE_TESTNET', True)
    
    if use_testnet:
        api_key = settings.BINANCE_API_KEY_TEST
        secret_key = settings.BINANCE_SECRET_KEY_TEST
    else:
        api_key = settings.BINANCE_API_KEY
        secret_key = settings.BINANCE_SECRET_KEY
    
    if not api_key or not secret_key:
        mode = "testnet" if use_testnet else "production"
        raise RuntimeError(f'Binance API credentials not configured for {mode} mode')
    
    mode_str = "TESTNET" if use_testnet else "PRODUCTION"
    logger.info(f"Creating Binance client in {mode_str} mode for margin trading")
    
    return Client(api_key, secret_key, testnet=use_testnet)


class BinanceMarginAdapter(MarginExecutionPort):
    """
    Binance Isolated Margin implementation.
    
    This adapter handles:
    - Transfers between Spot and Isolated Margin wallets
    - Placing margin orders (MARKET, LIMIT, STOP_LOSS_LIMIT)
    - Monitoring margin levels and positions
    - Order management (cancel, query)
    
    WARNING: When use_testnet=False, this operates with REAL MONEY!
    
    Key Binance Isolated Margin Concepts:
    - Each trading pair has its own isolated margin account
    - Margin is borrowed per-pair, not shared across pairs
    - Liquidation affects only the specific pair, not entire account
    """
    
    def __init__(self, client: Client | None = None, use_testnet: bool = None):
        """
        Initialize adapter with Binance client.
        
        Args:
            client: Optional pre-configured Binance client
            use_testnet: Override testnet setting. If None, uses settings.BINANCE_USE_TESTNET
        """
        if use_testnet is None:
            use_testnet = getattr(settings, 'BINANCE_USE_TESTNET', True)
        
        self.use_testnet = use_testnet
        self.client = client or _get_binance_client(use_testnet)
        
        mode = "TESTNET" if use_testnet else "PRODUCTION"
        logger.info(f"BinanceMarginAdapter initialized in {mode} mode")
        
        if not use_testnet:
            logger.warning("⚠️ PRODUCTION mode - Real money margin operations!")
    
    def transfer_to_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> MarginTransferResult:
        """
        Transfer from Spot wallet to Isolated Margin account.
        
        This moves funds from your regular Spot wallet to the isolated margin
        account for a specific trading pair.
        
        Example:
            Transfer 100 USDC to BTCUSDC isolated margin account
            >>> adapter.transfer_to_margin("BTCUSDC", "USDC", Decimal("100"))
        """
        try:
            mode = "TESTNET" if self.use_testnet else "PRODUCTION"
            logger.info(f"[{mode}] Transferring {amount} {asset} to margin for {symbol}")
            
            response = self.client.transfer_spot_to_isolated_margin(
                asset=asset,
                symbol=symbol,
                amount=str(amount),
            )
            
            transaction_id = str(response.get("tranId", ""))
            logger.info(f"Transfer successful: txId={transaction_id}")
            
            return MarginTransferResult(
                success=True,
                transaction_id=transaction_id,
                asset=asset,
                amount=amount,
                from_account="SPOT",
                to_account=f"ISOLATED_MARGIN:{symbol}",
            )
            
        except BinanceAPIException as e:
            logger.error(f"Transfer to margin failed: {e.message} (code: {e.code})")
            return MarginTransferResult(
                success=False,
                transaction_id=None,
                asset=asset,
                amount=amount,
                from_account="SPOT",
                to_account=f"ISOLATED_MARGIN:{symbol}",
                error_message=f"{e.message} (code: {e.code})",
            )
        except Exception as e:
            logger.exception(f"Unexpected error in transfer_to_margin: {e}")
            return MarginTransferResult(
                success=False,
                transaction_id=None,
                asset=asset,
                amount=amount,
                from_account="SPOT",
                to_account=f"ISOLATED_MARGIN:{symbol}",
                error_message=str(e),
            )
    
    def transfer_from_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> MarginTransferResult:
        """
        Transfer from Isolated Margin account back to Spot wallet.
        
        This moves available (non-locked) funds from the isolated margin
        account back to your regular Spot wallet.
        
        Note: Will fail if:
            - Amount exceeds available (free) balance
            - Transfer would cause margin level to drop below safe threshold
        """
        try:
            mode = "TESTNET" if self.use_testnet else "PRODUCTION"
            logger.info(f"[{mode}] Transferring {amount} {asset} from margin for {symbol}")
            
            response = self.client.transfer_isolated_margin_to_spot(
                asset=asset,
                symbol=symbol,
                amount=str(amount),
            )
            
            transaction_id = str(response.get("tranId", ""))
            logger.info(f"Transfer successful: txId={transaction_id}")
            
            return MarginTransferResult(
                success=True,
                transaction_id=transaction_id,
                asset=asset,
                amount=amount,
                from_account=f"ISOLATED_MARGIN:{symbol}",
                to_account="SPOT",
            )
            
        except BinanceAPIException as e:
            logger.error(f"Transfer from margin failed: {e.message} (code: {e.code})")
            return MarginTransferResult(
                success=False,
                transaction_id=None,
                asset=asset,
                amount=amount,
                from_account=f"ISOLATED_MARGIN:{symbol}",
                to_account="SPOT",
                error_message=f"{e.message} (code: {e.code})",
            )
        except Exception as e:
            logger.exception(f"Unexpected error in transfer_from_margin: {e}")
            return MarginTransferResult(
                success=False,
                transaction_id=None,
                asset=asset,
                amount=amount,
                from_account=f"ISOLATED_MARGIN:{symbol}",
                to_account="SPOT",
                error_message=str(e),
            )
    
    def get_margin_account(self, symbol: str) -> MarginAccountSnapshot:
        """
        Get Isolated Margin account info for a specific symbol.
        
        Returns detailed information about the margin account including:
        - Base and quote asset balances (free, locked, borrowed)
        - Current margin level
        - Liquidation price
        - Whether margin trading is enabled
        """
        try:
            response = self.client.get_isolated_margin_account()
            
            # Find the specific symbol in the response
            for asset_info in response.get("assets", []):
                if asset_info.get("symbol") == symbol:
                    base_asset = asset_info.get("baseAsset", {})
                    quote_asset = asset_info.get("quoteAsset", {})
                    
                    return MarginAccountSnapshot(
                        symbol=symbol,
                        base_asset=base_asset.get("asset", ""),
                        base_free=Decimal(base_asset.get("free", "0")),
                        base_locked=Decimal(base_asset.get("locked", "0")),
                        base_borrowed=Decimal(base_asset.get("borrowed", "0")),
                        quote_asset=quote_asset.get("asset", ""),
                        quote_free=Decimal(quote_asset.get("free", "0")),
                        quote_locked=Decimal(quote_asset.get("locked", "0")),
                        quote_borrowed=Decimal(quote_asset.get("borrowed", "0")),
                        margin_level=Decimal(asset_info.get("marginLevel", "999")),
                        liquidation_price=Decimal(asset_info.get("liquidatePrice", "0")),
                        is_margin_trade_enabled=asset_info.get("marginRatio", "") != "",
                    )
            
            raise ValueError(f"Symbol {symbol} not found in Isolated Margin account")
            
        except BinanceAPIException as e:
            logger.error(f"Get margin account failed: {e.message} (code: {e.code})")
            raise
    
    def place_margin_order(
        self,
        symbol: str,
        side: str,
        order_type: str,
        quantity: Decimal,
        price: Optional[Decimal] = None,
        stop_price: Optional[Decimal] = None,
        side_effect_type: Optional[str] = None,
    ) -> MarginOrderExecutionResult:
        """
        Place an order on Isolated Margin account.
        
        Supported order types:
        - MARKET: Immediate execution at best available price
        - LIMIT: Execute only at specified price or better
        - STOP_LOSS_LIMIT: Triggered when stop_price reached, then becomes LIMIT
        - TAKE_PROFIT_LIMIT: Triggered when stop_price reached (for profit)
        
        Side effect types:
        - MARGIN_BUY: Auto-borrow needed amount when buying
        - AUTO_REPAY: Auto-repay borrowed amount when selling
        - None: No auto borrow/repay
        
        Example (Long entry with auto-borrow):
            >>> adapter.place_margin_order(
            ...     symbol="BTCUSDC",
            ...     side="BUY",
            ...     order_type="MARKET",
            ...     quantity=Decimal("0.001"),
            ...     side_effect_type="MARGIN_BUY"
            ... )
        """
        try:
            # Build order parameters
            params = {
                "symbol": symbol,
                "side": side,
                "type": order_type,
                "quantity": str(quantity),
                "isIsolated": "TRUE",
            }
            
            # Add price for LIMIT orders
            if price is not None:
                params["price"] = str(price)
            
            # Add stop price for STOP_LOSS_LIMIT orders
            if stop_price is not None:
                params["stopPrice"] = str(stop_price)
            
            # Add time in force for limit-type orders
            if order_type in ("LIMIT", "STOP_LOSS_LIMIT", "TAKE_PROFIT_LIMIT"):
                params["timeInForce"] = "GTC"  # Good Till Cancelled
            
            # Add side effect type for auto borrow/repay
            if side_effect_type is not None:
                params["sideEffectType"] = side_effect_type
            
            mode = "TESTNET" if self.use_testnet else "PRODUCTION"
            logger.info(f"[{mode}] Placing margin order: {params}")
            
            response = self.client.create_margin_order(**params)
            
            order_id = str(response.get("clientOrderId", ""))
            binance_order_id = str(response.get("orderId", ""))
            
            logger.info(f"Margin order placed: orderId={binance_order_id}, status={response.get('status')}")
            
            return MarginOrderExecutionResult(
                success=True,
                order_id=order_id,
                binance_order_id=binance_order_id,
                symbol=symbol,
                side=side,
                order_type=order_type,
                quantity=quantity,
                price=price,
                filled_quantity=Decimal(response.get("executedQty", "0")),
                avg_fill_price=Decimal(response.get("price", "0")) if response.get("price") else None,
                status=response.get("status", "NEW"),
            )
            
        except BinanceAPIException as e:
            logger.error(f"Margin order failed: {e.message} (code: {e.code})")
            return MarginOrderExecutionResult(
                success=False,
                order_id=None,
                binance_order_id=None,
                symbol=symbol,
                side=side,
                order_type=order_type,
                quantity=quantity,
                price=price,
                filled_quantity=Decimal("0"),
                avg_fill_price=None,
                status="REJECTED",
                error_message=f"{e.message} (code: {e.code})",
            )
        except Exception as e:
            logger.exception(f"Unexpected error in place_margin_order: {e}")
            return MarginOrderExecutionResult(
                success=False,
                order_id=None,
                binance_order_id=None,
                symbol=symbol,
                side=side,
                order_type=order_type,
                quantity=quantity,
                price=price,
                filled_quantity=Decimal("0"),
                avg_fill_price=None,
                status="ERROR",
                error_message=str(e),
            )
    
    def cancel_margin_order(
        self,
        symbol: str,
        order_id: str,
    ) -> bool:
        """
        Cancel an open Isolated Margin order.
        
        Args:
            symbol: Trading pair
            order_id: Binance order ID to cancel
            
        Returns:
            True if cancelled successfully, False otherwise
        """
        try:
            mode = "TESTNET" if self.use_testnet else "PRODUCTION"
            logger.info(f"[{mode}] Cancelling margin order {order_id} for {symbol}")
            
            self.client.cancel_margin_order(
                symbol=symbol,
                orderId=order_id,
                isIsolated="TRUE",
            )
            
            logger.info(f"Margin order {order_id} cancelled successfully")
            return True
            
        except BinanceAPIException as e:
            logger.error(f"Cancel margin order failed: {e.message} (code: {e.code})")
            return False
        except Exception as e:
            logger.exception(f"Unexpected error in cancel_margin_order: {e}")
            return False
    
    def get_margin_level(self, symbol: str) -> Decimal:
        """
        Get current margin level for symbol.
        
        Margin Level = Total Asset Value / (Total Borrowed + Total Interest)
        
        Interpretation:
        - >= 2.0: SAFE - can open new positions
        - >= 1.5: CAUTION - be careful
        - >= 1.3: WARNING - consider reducing position
        - >= 1.1: CRITICAL - close to liquidation
        - < 1.1: DANGER - imminent liquidation
        """
        account = self.get_margin_account(symbol)
        return account.margin_level
    
    def get_open_margin_orders(self, symbol: str) -> list[dict]:
        """
        Get all open margin orders for a symbol.
        
        Returns list of orders with details including:
        - orderId, symbol, side, type, quantity, price, status
        """
        try:
            orders = self.client.get_open_margin_orders(
                symbol=symbol,
                isIsolated="TRUE",
            )
            
            return [
                {
                    "order_id": str(order.get("orderId")),
                    "client_order_id": order.get("clientOrderId"),
                    "symbol": order.get("symbol"),
                    "side": order.get("side"),
                    "type": order.get("type"),
                    "quantity": Decimal(order.get("origQty", "0")),
                    "price": Decimal(order.get("price", "0")),
                    "stop_price": Decimal(order.get("stopPrice", "0")) if order.get("stopPrice") else None,
                    "status": order.get("status"),
                    "time": order.get("time"),
                }
                for order in orders
            ]
            
        except BinanceAPIException as e:
            logger.error(f"Get open margin orders failed: {e.message} (code: {e.code})")
            return []


class MockMarginAdapter(MarginExecutionPort):
    """
    Mock Margin adapter for testing and paper trading.
    
    Simulates margin operations without connecting to Binance.
    Useful for unit tests and development.
    """
    
    def __init__(self):
        """Initialize mock adapter with simulated state."""
        self.transfers: list[MarginTransferResult] = []
        self.orders: list[MarginOrderExecutionResult] = []
        self.balances: dict[str, dict] = {}
        self._order_counter = 0
        
        logger.info("MockMarginAdapter initialized (paper trading mode)")
    
    def transfer_to_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> MarginTransferResult:
        """Simulate transfer to margin."""
        result = MarginTransferResult(
            success=True,
            transaction_id=f"mock-tx-{len(self.transfers) + 1}",
            asset=asset,
            amount=amount,
            from_account="SPOT",
            to_account=f"ISOLATED_MARGIN:{symbol}",
        )
        self.transfers.append(result)
        
        # Update simulated balance
        if symbol not in self.balances:
            self.balances[symbol] = {"base_free": Decimal("0"), "quote_free": Decimal("0")}
        
        if asset == symbol[-4:]:  # Quote asset (e.g., USDC)
            self.balances[symbol]["quote_free"] += amount
        else:
            self.balances[symbol]["base_free"] += amount
        
        return result
    
    def transfer_from_margin(
        self,
        symbol: str,
        asset: str,
        amount: Decimal,
    ) -> MarginTransferResult:
        """Simulate transfer from margin."""
        result = MarginTransferResult(
            success=True,
            transaction_id=f"mock-tx-{len(self.transfers) + 1}",
            asset=asset,
            amount=amount,
            from_account=f"ISOLATED_MARGIN:{symbol}",
            to_account="SPOT",
        )
        self.transfers.append(result)
        return result
    
    def get_margin_account(self, symbol: str) -> MarginAccountSnapshot:
        """Return simulated margin account."""
        balance = self.balances.get(symbol, {"base_free": Decimal("0"), "quote_free": Decimal("0")})
        
        # Parse symbol (e.g., BTCUSDC -> BTC, USDC)
        quote = symbol[-4:]  # Assumes 4-char quote (USDC, USDT)
        base = symbol[:-4]
        
        return MarginAccountSnapshot(
            symbol=symbol,
            base_asset=base,
            base_free=balance.get("base_free", Decimal("0")),
            base_locked=Decimal("0"),
            base_borrowed=Decimal("0"),
            quote_asset=quote,
            quote_free=balance.get("quote_free", Decimal("0")),
            quote_locked=Decimal("0"),
            quote_borrowed=Decimal("0"),
            margin_level=Decimal("999"),  # Safe level
            liquidation_price=Decimal("0"),
            is_margin_trade_enabled=True,
        )
    
    def place_margin_order(
        self,
        symbol: str,
        side: str,
        order_type: str,
        quantity: Decimal,
        price: Optional[Decimal] = None,
        stop_price: Optional[Decimal] = None,
        side_effect_type: Optional[str] = None,
    ) -> MarginOrderExecutionResult:
        """Simulate margin order placement."""
        self._order_counter += 1
        
        result = MarginOrderExecutionResult(
            success=True,
            order_id=f"mock-client-{self._order_counter}",
            binance_order_id=f"mock-{self._order_counter}",
            symbol=symbol,
            side=side,
            order_type=order_type,
            quantity=quantity,
            price=price,
            filled_quantity=quantity if order_type == "MARKET" else Decimal("0"),
            avg_fill_price=price or Decimal("100000"),  # Mock price
            status="FILLED" if order_type == "MARKET" else "NEW",
        )
        self.orders.append(result)
        return result
    
    def cancel_margin_order(
        self,
        symbol: str,
        order_id: str,
    ) -> bool:
        """Simulate order cancellation."""
        return True
    
    def get_margin_level(self, symbol: str) -> Decimal:
        """Return safe margin level for mock."""
        return Decimal("999")
    
    def get_open_margin_orders(self, symbol: str) -> list[dict]:
        """Return mock open orders."""
        return [
            {
                "order_id": order.binance_order_id,
                "client_order_id": order.order_id,
                "symbol": order.symbol,
                "side": order.side,
                "type": order.order_type,
                "quantity": order.quantity,
                "price": order.price,
                "stop_price": None,
                "status": order.status,
                "time": None,
            }
            for order in self.orders
            if order.symbol == symbol and order.status in ("NEW", "PARTIALLY_FILLED")
        ]


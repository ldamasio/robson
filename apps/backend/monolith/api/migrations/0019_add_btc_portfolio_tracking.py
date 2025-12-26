# Migration 0019: Add BTC Portfolio Tracking
# ADR-0012: Zero-downtime migration (metadata-only, no table rewrite)

from django.db import migrations, models


class Migration(migrations.Migration):
    """
    Add BTC-denominated portfolio tracking functionality.

    Changes:
    1. Add DEPOSIT and WITHDRAWAL transaction types (external flows)
    2. Add EXTERNAL movement category
    3. Add BTC valuation fields to BalanceSnapshot

    Lock Analysis:
    - ACCESS EXCLUSIVE lock duration: <1 second (metadata-only)
    - No table rewrite (NULL columns and default=0 for new fields)
    - Safe for production deployment
    """

    dependencies = [
        ('api', '0018_create_stop_indexes_concurrent'),
    ]

    operations = [
        # Add EXTERNAL category to MovementCategory
        migrations.AlterField(
            model_name='audittransaction',
            name='category',
            field=models.CharField(
                max_length=20,
                choices=[
                    ('TRADING', 'Trading (Buy/Sell)'),
                    ('TRANSFER', 'Transfer (Between Accounts)'),
                    ('CREDIT', 'Credit (Borrow/Repay)'),
                    ('ORDER', 'Order Lifecycle'),
                    ('FEE', 'Fees & Interest'),
                    ('RISK', 'Risk Events'),
                    ('EXTERNAL', 'External Flows (Deposit/Withdrawal)'),  # NEW
                ],
                default='TRADING',
                db_index=True,
                help_text='High-level category of this movement',
            ),
        ),

        # Add DEPOSIT and WITHDRAWAL transaction types
        migrations.AlterField(
            model_name='audittransaction',
            name='transaction_type',
            field=models.CharField(
                max_length=30,
                choices=[
                    # CATEGORY A: TRADING
                    ('SPOT_BUY', 'Spot Buy'),
                    ('SPOT_SELL', 'Spot Sell'),
                    ('MARGIN_BUY', 'Margin Buy'),
                    ('MARGIN_SELL', 'Margin Sell'),
                    # CATEGORY B: TRANSFERS
                    ('TRANSFER_SPOT_TO_ISOLATED', 'Transfer Spot → Isolated Margin'),
                    ('TRANSFER_ISOLATED_TO_SPOT', 'Transfer Isolated Margin → Spot'),
                    ('TRANSFER_TO_MARGIN', 'Transfer to Margin'),
                    ('TRANSFER_FROM_MARGIN', 'Transfer from Margin'),
                    # CATEGORY C: CREDIT
                    ('MARGIN_BORROW', 'Margin Borrow'),
                    ('MARGIN_REPAY', 'Margin Repay'),
                    ('INTEREST_CHARGED', 'Interest Charged'),
                    # CATEGORY D: ORDER LIFECYCLE
                    ('STOP_LOSS_PLACED', 'Stop-Loss Placed'),
                    ('STOP_LOSS_TRIGGERED', 'Stop-Loss Triggered'),
                    ('STOP_LOSS_CANCELLED', 'Stop-Loss Cancelled'),
                    ('TAKE_PROFIT_PLACED', 'Take-Profit Placed'),
                    ('TAKE_PROFIT_TRIGGERED', 'Take-Profit Triggered'),
                    ('LIMIT_ORDER_PLACED', 'Limit Order Placed'),
                    ('LIMIT_ORDER_FILLED', 'Limit Order Filled'),
                    ('LIMIT_ORDER_CANCELLED', 'Limit Order Cancelled'),
                    # CATEGORY E: FEES
                    ('TRADING_FEE', 'Trading Fee'),
                    ('FEE_PAID', 'Fee Paid'),
                    # CATEGORY F: RISK EVENTS
                    ('LIQUIDATION', 'Liquidation'),
                    ('MARGIN_CALL', 'Margin Call Warning'),
                    # CATEGORY G: EXTERNAL FLOWS (NEW)
                    ('DEPOSIT', 'Deposit (External → Exchange)'),
                    ('WITHDRAWAL', 'Withdrawal (Exchange → External)'),
                ],
                db_index=True,
                help_text='Type of transaction',
            ),
        ),

        # Add BTC fields to BalanceSnapshot
        migrations.AddField(
            model_name='balancesnapshot',
            name='total_equity_btc',
            field=models.DecimalField(
                max_digits=20,
                decimal_places=8,
                null=True,  # No DEFAULT (avoid table rewrite)
                blank=True,
                help_text='Total equity denominated in BTC',
            ),
        ),

        migrations.AddField(
            model_name='balancesnapshot',
            name='spot_btc_value',
            field=models.DecimalField(
                max_digits=20,
                decimal_places=8,
                default=0,  # Safe default (no table rewrite)
                help_text='Total spot balances converted to BTC',
            ),
        ),

        migrations.AddField(
            model_name='balancesnapshot',
            name='margin_btc_value',
            field=models.DecimalField(
                max_digits=20,
                decimal_places=8,
                default=0,  # Safe default (no table rewrite)
                help_text='Total margin positions converted to BTC (net of debt)',
            ),
        ),
    ]

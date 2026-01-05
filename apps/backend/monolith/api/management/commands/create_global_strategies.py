"""
Create Global Strategies Management Command

Creates global system strategy templates (client=null) that are
available to all users. These templates define trading approaches,
risk parameters, and configuration.

Global strategies are NOT auto-trading algorithms - they are templates
that users select when creating operations.

Run: python manage.py create_global_strategies
"""

from django.core.management.base import BaseCommand

from api.models import Strategy


class Command(BaseCommand):
    help = "Create global system strategy templates (available to all users)"

    # Pre-defined strategy templates offered by Robson
    DEFAULT_STRATEGIES = [
        {
            "name": "Iron Exit Protocol",
            "description": (
                "Isolated margin short with technical stop on 15m (level 2) "
                "and 1% max risk. Stop executed by Robson monitor (market)."
            ),
            "market_bias": "BEARISH",
            "config": {
                "account_type": "isolated_margin",
                "capital_mode": "balance",
                "capital_balance_percent": "100",
                "technical_stop": {
                    "timeframe": "15m",
                    "level": 2,
                    "side": "SELL",
                },
                "stop_execution": "robson_market",
                "risk_percent": 1.0,
            },
            "risk_config": {
                "max_risk_per_trade": 1.0,
                "use_technical_stop": True,
                "stop_execution": "robson_market",
            },
        },
        {
            "name": "All In",
            "description": "Go all-in with technical stop precision. Buy maximum position size with stop at second technical support (15m chart).",
            "market_bias": "BULLISH",
            "config": {
                "timeframe": "15m",
                "indicators": ["Support/Resistance", "Technical Stop"],
                "entry_type": "manual",
                "risk_percent": 1.0,
                "use_technical_stop": True,
                "leverage": 3,
                "account_type": "isolated_margin",
            },
            "risk_config": {
                "max_risk_per_trade": 1.0,
                "use_technical_stop": True,
                "stop_placement": "second_support_15m",
            },
        },
        {
            "name": "Rescue Forces",
            "description": "Automatic rescue on bullish momentum. Enters when MA4 crosses above MA9 with short-term uptrend confirmed.",
            "market_bias": "BULLISH",
            "config": {
                "timeframe": "15m",
                "indicators": ["MA4", "MA9", "Trend"],
                "entry_type": "auto",
                "entry_conditions": {
                    "ma_cross": "MA4 > MA9",
                    "trend": "short_term_bullish",
                    "confirmation": "volume_spike",
                },
                "risk_percent": 1.0,
                "leverage": 3,
                "account_type": "isolated_margin",
            },
            "risk_config": {
                "max_risk_per_trade": 1.0,
                "use_technical_stop": True,
                "stop_placement": "below_ma9",
            },
        },
        {
            "name": "Smooth Sailing",
            "description": "Ride the calm waves of trending markets with moving average crossovers.",
            "market_bias": "BULLISH",
            "config": {
                "timeframe": "1h",
                "indicators": ["MA50", "MA200"],
                "entry_type": "trend",
                "risk_percent": 0.5,
                "account_type": "spot",
            },
        },
        {
            "name": "Bounce Back",
            "description": "Catch the bounce when price returns to mean in range-bound markets.",
            "market_bias": "BULLISH",
            "config": {
                "timeframe": "30m",
                "indicators": ["Bollinger Bands", "RSI"],
                "entry_type": "reversion",
                "risk_percent": 0.5,
                "account_type": "spot",
            },
        },
    ]

    def handle(self, *args, **options):
        """Create or update global strategy templates."""

        self.stdout.write(self.style.HTTP_INFO("Creating global strategy templates..."))

        created_count = 0
        updated_count = 0

        for strategy_data in self.DEFAULT_STRATEGIES:
            strategy, created = Strategy.objects.get_or_create(
                name=strategy_data["name"],
                client=None,  # NULL = global (available to all users)
                defaults={
                    "description": strategy_data["description"],
                    "config": strategy_data.get("config", {}),
                    "risk_config": strategy_data.get("risk_config", {}),
                    "market_bias": strategy_data.get("market_bias", "BULLISH"),
                    "is_active": True,
                },
            )

            if created:
                created_count += 1
                self.stdout.write(self.style.SUCCESS(f"  ✓ Created: {strategy.name}"))
            else:
                # Update existing global strategy with latest config
                strategy.description = strategy_data["description"]
                strategy.config = strategy_data.get("config", {})
                strategy.risk_config = strategy_data.get("risk_config", {})
                strategy.market_bias = strategy_data.get("market_bias", strategy.market_bias)
                strategy.is_active = True
                strategy.save()
                updated_count += 1
                self.stdout.write(self.style.HTTP_INFO(f"  ~ Updated: {strategy.name}"))

        # Show summary
        self.stdout.write(
            "\n"
            + self.style.SUCCESS(
                f"Successfully processed {created_count + updated_count} global strategies:\n"
                f"  • {created_count} created\n"
                f"  • {updated_count} updated\n\n"
                f"Global strategies are now available to all users via GET /api/strategies/"
            )
        )

        # Show verification query
        self.stdout.write(
            self.style.HTTP_INFO(
                "\nVerify with:\n"
                '  python manage.py shell -c "from api.models import Strategy; '
                "print(list(Strategy.objects.filter(client__isnull=True).values_list('name', flat=True)))\""
            )
        )

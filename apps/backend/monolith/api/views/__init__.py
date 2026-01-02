"""
API Views Package.

This package contains modularized view files.
To maintain backward compatibility with main_urls.py imports,
we re-export necessary modules here.
"""

# Re-export modular views for backward compatibility
from . import portfolio_btc
from . import auth
from . import trading
from . import strategy_views
from . import market_views
from . import margin_views
from . import emotional_guard
from . import risk_managed_trading
from . import audit_views
from . import pattern_views
from . import operation_views
from . import trading_intent_views
from . import user_operations

# Import legacy functions from parent views.py module
# We need to use absolute import to avoid circular dependency
try:
    import importlib.util
    import os

    # Load views.py as a separate module
    views_py_path = os.path.join(os.path.dirname(__file__), '..', 'views.py')
    spec = importlib.util.spec_from_file_location("api.legacy_views", views_py_path)
    if spec and spec.loader:
        legacy_views = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(legacy_views)

        # Export ALL callable functions from legacy views automatically
        # This prevents AttributeError for any function referenced in main_urls.py
        import types
        for name in dir(legacy_views):
            obj = getattr(legacy_views, name)
            # Export all functions and classes (but skip private/magic methods and modules)
            if not name.startswith('_') and isinstance(obj, (types.FunctionType, type)):
                globals()[name] = obj
except Exception as e:
    # Silently fail if legacy views can't be imported
    import logging
    logging.warning(f"Could not import legacy views.py: {e}")

__all__ = [
    'portfolio_btc',
    'auth',
    'trading',
    'strategy_views',
    'market_views',
    'margin_views',
    'emotional_guard',
    'risk_managed_trading',
    'audit_views',
    'pattern_views',
    'operation_views',
    'trading_intent_views',
    'user_operations',
]

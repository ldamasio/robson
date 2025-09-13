# # api/models/__init__.py

# """
# Importa todos os models para manter compatibilidade
# """
# from .base import *
# from .trading import *
# from .analysis import *
# from .patterns import *
# from .indicators import *
# from .risk import *
# from .reports import *



# api/models/__init__.py
"""
Import all models to maintain compatibility with existing imports.
Centralizes imports and facilitates model usage.
"""

# Import base classes and mixins
from .base import (
    # Mixins
    TimestampMixin,
    TenantMixin,
    MarketTypeMixin,
    DescriptionMixin,
    ExperienceMixin,
    StatusMixin,
    
    # Base classes
    BaseModel,
    BaseTechnicalModel,
    BaseConfigModel,
    BaseFinancialModel,
    
    # Managers
    ActiveManager,
    TenantManager,
    
    # Utilities
    ModelChoices,
)

# Import trading models
from .trading import (
    Symbol,
    Strategy,
    Order,
    Operation,
    Position,
    Trade,
)

# Import technical analysis models (refactored)
from .analysis import (
    TechnicalAnalysisInterpretation,
    TechnicalEvent,
    Argument,
    Reason,
)

# Chart patterns
from .patterns import (
    Rectangle,
    Triangle,
)

# Indicators
from .indicators import (
    MovingAverage,
    RSIIndicator,
    MACDIndicator,
)

# Facts (refactored)
from .facts import (
    Resistance,
    Support,
    Line,
    TrendLine,
    Channel,
    Accumulation,
    Sideways,
    Breakout,
    Uptrend,
    Downtrend,
)

# ==========================================
# MAINTAIN COMPATIBILITY WITH OLD MODELS
# ==========================================

# Old models that haven't been migrated yet
# Temporarily import from original models.py to avoid breaking anything

# TODO: Migrate these models gradually
try:
    # Import models that haven't been refactored yet
    # Comment out as each one gets migrated
    
    # Principles (legacy) — compatibility layer
    from ..models import OddsYourFavor
    from ..models import LimitLosses
    
    # Technical Analysis
    # Technical analysis models migrated to api/models/analysis.py
    
    # Facts migrated to api/models/facts.py
    
    # Chart Patterns
    # Chart Patterns migrated to api/models/patterns.py
    # Triangle, Hammer, etc. (incomplete classes in original)
    
    # Reversal Patterns
    # Engulfing, ShootingStar, etc. (incomplete classes in original)
    
    # Statistical Indicators
    # MovingAverage, RSI, etc. (incomplete classes in original)
    
    # Attributes (legacy) — compatibility layer
    from ..models import Attribute

    # Rules/Config/Reports (legacy) — compatibility layer
    from ..models import OnePercentOfCapital
    from ..models import JustBet4percent
    from ..models import OnlyTradeReversal
    from ..models import MaxTradePerDay
    from ..models import AlocatedCapitalPercent

except ImportError as e:
    # If can't import from original models.py, no problem
    # Means we already migrated everything or models.py doesn't exist anymore
    pass

# ==========================================
# LIST OF ALL AVAILABLE MODELS
# ==========================================

__all__ = [
    # Base classes
    'TimestampMixin',
    'TenantMixin', 
    'MarketTypeMixin',
    'DescriptionMixin',
    'ExperienceMixin',
    'StatusMixin',
    'BaseModel',
    'BaseTechnicalModel',
    'BaseConfigModel',
    'BaseFinancialModel',
    'ActiveManager',
    'TenantManager',
    'ModelChoices',
    
    # Trading models (refactored)
    'Symbol',
    'Strategy',
    'Order',
    'Operation',
    'Position',
    'Trade',
    
    # Refactored technical analysis
    'TechnicalAnalysisInterpretation',
    'TechnicalEvent',
    'Argument',
    'Reason',

    # Refactored patterns and indicators
    'Rectangle',
    'Triangle',
    'MovingAverage',
    'RSIIndicator',
    'MACDIndicator',

    # Old models (kept for compatibility)
    'OddsYourFavor',
    'LimitLosses',
    'Attribute',
    'Resistance',
    'Support',
    'Line',
    'TrendLine',
    'Channel',
    'Accumulation',
    'Sideways',
    'Breakout',
    'Uptrend',
    'Downtrend',
    'OnePercentOfCapital',
    'JustBet4percent',
    'OnlyTradeReversal',
    'MaxTradePerDay',
    'AlocatedCapitalPercent',
]

# ==========================================
# MIGRATION INFORMATION
# ==========================================

MIGRATION_STATUS = {
    'completed': [
        'Symbol',      # ✅ Migrated with improvements
        'Strategy',    # ✅ Migrated with JSON config
        'Order',       # ✅ Completely new (fixed typo symbol_orderd)
        'Operation',   # ✅ Significantly improved
        'Position',    # ✅ New model for positions
        'Trade',       # ✅ New model for history
        # ✅ Technical analysis
        'TechnicalAnalysisInterpretation',
        'TechnicalEvent',
        'Argument',
        'Reason',
        # ✅ Patterns
        'Rectangle',
        'Triangle',
        # ✅ Indicators
        'MovingAverage',
        'RSIIndicator',
        'MACDIndicator',
        # ✅ Facts
        'Resistance',
        'Support',
        'Line',
        'TrendLine',
        'Channel',
        'Accumulation',
        'Sideways',
        'Breakout',
        'Uptrend',
        'Downtrend',
    ],
    'in_progress': [
        # Next to be migrated
    ],
    'planned': [
        # Rules and configs
    ],
    'deprecated': [
        # Models to be removed
    ]
}

def get_migration_status():
    """Return migration status of models"""
    return MIGRATION_STATUS

def list_available_models():
    """List all available models"""
    return __all__

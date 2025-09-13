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

# ==========================================
# MAINTAIN COMPATIBILITY WITH OLD MODELS
# ==========================================

# Old models that haven't been migrated yet
# Temporarily import from original models.py to avoid breaking anything

# TODO: Migrate these models gradually
try:
    # Import models that haven't been refactored yet
    # Comment out as each one gets migrated
    
    # Principles
    from ..models import OddsYourFavor
    from ..models import LimitLosses
    
    # Attributes  
    from ..models import Attribute
    
    # Technical Analysis
    from ..models import TechnicalAnalysisInterpretation
    from ..models import TechnicalEvent
    from ..models import Argument
    from ..models import Reason
    
    # Facts
    from ..models import Resistance
    from ..models import Support
    from ..models import Line
    from ..models import TrendLine
    from ..models import Channel
    from ..models import Accumulation
    from ..models import Sideways
    from ..models import Breakout
    from ..models import Uptrend
    from ..models import Downtrend
    
    # Chart Patterns
    from ..models import Rectangle
    # Triangle, Hammer, etc. (incomplete classes in original)
    
    # Reversal Patterns
    # Engulfing, ShootingStar, etc. (incomplete classes in original)
    
    # Statistical Indicators
    # MovingAverage, RSI, etc. (incomplete classes in original)
    
    # Rules
    from ..models import OnePercentOfCapital
    from ..models import JustBet4percent
    
    # Config
    from ..models import OnlyTradeReversal
    from ..models import MaxTradePerDay
    
    # Reports
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
    
    # Old models (not migrated yet)
    # TODO: Remove as each gets migrated
    'OddsYourFavor',
    'LimitLosses',
    'Attribute',
    'TechnicalAnalysisInterpretation',
    'TechnicalEvent',
    'Argument',
    'Reason',
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
    'Rectangle',
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
    ],
    'in_progress': [
        # Next to be migrated
    ],
    'planned': [
        'TechnicalAnalysisInterpretation',
        'TechnicalEvent', 
        'Argument',
        'Reason',
        'Resistance',
        'Support',
        'Line',
        'TrendLine',
        'Channel',
        'Rectangle',
        # Chart patterns
        # Indicators
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

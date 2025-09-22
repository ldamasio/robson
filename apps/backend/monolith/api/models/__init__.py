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
    Hammer,
    InvertedHammer,
    HangingMan,
    Piercing,
    Engulfing,
    ShootingStar,
    MorningStar,
    EveningStar,
)

# Indicators
from .indicators import (
    MovingAverage,
    RSIIndicator,
    MACDIndicator,
    RelativeStrengthIndex,
    MovingAverageConvergenceDivergence,
    BollingerBands,
    StochasticOscillator,
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

# Principles and qualitative data
from .principles import (
    OddsYourFavor,
    LimitLosses,
    Attribute,
)

# Risk & configuration
from .risk import (
    OnePercentOfCapital,
    JustBet4percent,
)

from .config import (
    OnlyTradeReversal,
    MaxTradePerDay,
)

# Reports
from .reports import (
    AlocatedCapitalPercent,
)

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
    'Hammer',
    'InvertedHammer',
    'HangingMan',
    'Piercing',
    'Engulfing',
    'ShootingStar',
    'MorningStar',
    'EveningStar',
    'MovingAverage',
    'RSIIndicator',
    'MACDIndicator',
    'RelativeStrengthIndex',
    'MovingAverageConvergenceDivergence',
    'BollingerBands',
    'StochasticOscillator',

    # Legacy models now fully migrated
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
        'Hammer',
        'InvertedHammer',
        'HangingMan',
        'Piercing',
        'Engulfing',
        'ShootingStar',
        'MorningStar',
        'EveningStar',
        # ✅ Indicators
        'MovingAverage',
        'RSIIndicator',
        'MACDIndicator',
        'RelativeStrengthIndex',
        'MovingAverageConvergenceDivergence',
        'BollingerBands',
        'StochasticOscillator',
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
        # ✅ Principles and risk/config
        'OddsYourFavor',
        'LimitLosses',
        'Attribute',
        'OnePercentOfCapital',
        'JustBet4percent',
        'OnlyTradeReversal',
        'MaxTradePerDay',
        'AlocatedCapitalPercent',
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

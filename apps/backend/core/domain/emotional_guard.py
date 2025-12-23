"""
Emotional Trading Guard Domain Entities.

Core intelligence for detecting and responding to emotional trading patterns.
This module protects traders from making impulsive decisions based on fear,
greed, or other psychological biases.

Key Concepts:
- SignalType: Categories of detected warning signs
- RiskLevel: Severity classification
- EmotionalSignal: Single detected warning
- IntentAnalysis: Complete analysis of a trading message
"""

from dataclasses import dataclass, field
from decimal import Decimal
from enum import Enum
from typing import Optional
from datetime import datetime


class SignalType(Enum):
    """
    Types of emotional signals detected in trading messages.
    
    Each signal type corresponds to a specific psychological pattern
    that can lead to poor trading decisions.
    """
    
    # Urgency signals
    URGENCY = "urgency"
    NOW_OR_NEVER = "now_or_never"
    FOMO = "fomo"
    
    # Overconfidence signals
    ABSOLUTE_CERTAINTY = "absolute_certainty"
    GUARANTEED_WIN = "guaranteed_win"
    INSIDER_INFO = "insider_info"
    
    # Risk blindness signals
    NO_STOP_LOSS = "no_stop_loss"
    ALL_IN = "all_in"
    EXCESSIVE_LEVERAGE = "excessive_leverage"
    IGNORE_RISK = "ignore_risk"
    
    # Revenge trading signals
    REVENGE_TRADING = "revenge_trading"
    RECOVER_LOSSES = "recover_losses"
    DOUBLE_DOWN = "double_down"
    
    # External pressure signals
    SOCIAL_PRESSURE = "social_pressure"
    INFLUENCER_TIP = "influencer_tip"
    GROUP_CONSENSUS = "group_consensus"
    
    # Greed signals
    MOON_THINKING = "moon_thinking"
    UNREALISTIC_TARGET = "unrealistic_target"
    GET_RICH_QUICK = "get_rich_quick"
    
    # Positive signals (good habits)
    HAS_STOP_LOSS = "has_stop_loss"
    HAS_ENTRY_PLAN = "has_entry_plan"
    HAS_TARGET = "has_target"
    RISK_DEFINED = "risk_defined"
    PATIENT_APPROACH = "patient_approach"


class RiskLevel(Enum):
    """
    Risk level classification for trading intentions.
    
    Determines the type of response the system should provide.
    """
    
    LOW = "low"           # Good habits detected, proceed normally
    MEDIUM = "medium"     # Some concerns, provide educational response
    HIGH = "high"         # Clear warning signs, require confirmation
    CRITICAL = "critical"  # Multiple red flags, strong recommendation to stop


@dataclass(frozen=True)
class EmotionalSignal:
    """
    A single detected emotional signal in a trading message.
    
    Represents one piece of evidence that the trader may be making
    an emotional rather than rational decision.
    
    Attributes:
        signal_type: Category of the signal
        confidence: How confident the detection is (0.0 to 1.0)
        matched_phrase: The text that triggered the detection
        weight: Impact weight for risk calculation (higher = more concerning)
        is_positive: Whether this is a good habit (reduces risk score)
        explanation: Human-readable explanation of why this is a concern
    """
    
    signal_type: SignalType
    confidence: float
    matched_phrase: str
    weight: float = 1.0
    is_positive: bool = False
    explanation: str = ""
    
    def __post_init__(self):
        """Validate confidence and weight ranges."""
        if not 0.0 <= self.confidence <= 1.0:
            raise ValueError(f"Confidence must be between 0 and 1, got {self.confidence}")
        if self.weight < 0:
            raise ValueError(f"Weight must be non-negative, got {self.weight}")


@dataclass(frozen=True)
class ExtractedParameters:
    """
    Trading parameters extracted from the user's message.
    
    Used to understand what trade the user wants to execute
    and whether risk parameters are defined.
    
    Attributes:
        symbol: Trading pair (e.g., BTCUSDC)
        side: BUY or SELL
        entry_price: Intended entry price
        stop_price: Stop-loss price
        target_price: Take-profit price
        leverage: Leverage multiplier
        capital_percent: Percentage of capital to risk
    """
    
    symbol: Optional[str] = None
    side: Optional[str] = None
    entry_price: Optional[Decimal] = None
    stop_price: Optional[Decimal] = None
    target_price: Optional[Decimal] = None
    leverage: Optional[int] = None
    capital_percent: Optional[Decimal] = None
    
    @property
    def has_risk_parameters(self) -> bool:
        """Check if minimum risk parameters are defined."""
        return self.stop_price is not None
    
    @property
    def is_complete(self) -> bool:
        """Check if all key parameters are defined."""
        return all([
            self.symbol,
            self.side,
            self.entry_price,
            self.stop_price,
        ])


@dataclass
class IntentAnalysis:
    """
    Complete analysis of a trading intent message.
    
    Contains all detected signals, extracted parameters, risk assessment,
    and the recommended response.
    
    This is the main output of the Emotional Trading Guard system.
    
    Attributes:
        original_message: The user's original input
        signals: List of detected emotional signals
        parameters: Extracted trading parameters
        risk_level: Overall risk classification
        risk_score: Numerical risk score (0-100)
        response_message: Human-readable response for the user
        proceed_allowed: Whether the system recommends proceeding
        requires_confirmation: Whether additional confirmation is needed
        educational_content: Optional educational content to display
        analyzed_at: Timestamp of analysis
    """
    
    original_message: str
    signals: list[EmotionalSignal] = field(default_factory=list)
    parameters: ExtractedParameters = field(default_factory=ExtractedParameters)
    risk_level: RiskLevel = RiskLevel.LOW
    risk_score: float = 0.0
    response_message: str = ""
    proceed_allowed: bool = True
    requires_confirmation: bool = False
    educational_content: Optional[str] = None
    analyzed_at: datetime = field(default_factory=datetime.now)
    
    @property
    def warning_signals(self) -> list[EmotionalSignal]:
        """Get only negative (warning) signals."""
        return [s for s in self.signals if not s.is_positive]
    
    @property
    def positive_signals(self) -> list[EmotionalSignal]:
        """Get only positive signals."""
        return [s for s in self.signals if s.is_positive]
    
    @property
    def has_warnings(self) -> bool:
        """Check if any warning signals were detected."""
        return len(self.warning_signals) > 0
    
    @property
    def signal_summary(self) -> dict[str, int]:
        """Count signals by type category."""
        summary = {}
        for signal in self.signals:
            category = signal.signal_type.value
            summary[category] = summary.get(category, 0) + 1
        return summary


# ============================================================================
# Signal Detection Patterns
# ============================================================================

# Patterns that indicate urgency/FOMO
URGENCY_PATTERNS = [
    ("agora", "now", 2.0, SignalType.URGENCY),
    ("imediatamente", "immediately", 2.5, SignalType.URGENCY),
    ("rápido", "fast", 1.5, SignalType.URGENCY),
    ("já", "right now", 2.0, SignalType.URGENCY),
    ("não posso perder", "can't miss", 2.5, SignalType.FOMO),
    ("vai explodir", "will explode", 2.5, SignalType.FOMO),
    ("vai subir muito", "will go up a lot", 2.0, SignalType.FOMO),
    ("última chance", "last chance", 3.0, SignalType.NOW_OR_NEVER),
    ("oportunidade única", "unique opportunity", 2.5, SignalType.NOW_OR_NEVER),
    ("vai decolar", "will take off", 2.0, SignalType.FOMO),
    ("pump", "pump", 2.0, SignalType.FOMO),
]

# Patterns that indicate overconfidence
OVERCONFIDENCE_PATTERNS = [
    ("certeza absoluta", "absolute certainty", 3.0, SignalType.ABSOLUTE_CERTAINTY),
    ("100% certo", "100% sure", 3.5, SignalType.ABSOLUTE_CERTAINTY),
    ("com certeza", "for sure", 2.0, SignalType.ABSOLUTE_CERTAINTY),
    ("não tem como dar errado", "can't go wrong", 3.5, SignalType.GUARANTEED_WIN),
    ("lucro garantido", "guaranteed profit", 4.0, SignalType.GUARANTEED_WIN),
    ("impossível perder", "impossible to lose", 4.0, SignalType.GUARANTEED_WIN),
    ("informação privilegiada", "insider info", 4.0, SignalType.INSIDER_INFO),
    ("fonte segura", "reliable source", 2.5, SignalType.INSIDER_INFO),
    ("dica quente", "hot tip", 2.5, SignalType.INSIDER_INFO),
]

# Patterns that indicate risk blindness
RISK_BLINDNESS_PATTERNS = [
    ("sem stop", "no stop", 3.5, SignalType.NO_STOP_LOSS),
    ("não preciso de stop", "don't need stop", 4.0, SignalType.NO_STOP_LOSS),
    ("todo meu capital", "all my capital", 4.0, SignalType.ALL_IN),
    ("tudo que tenho", "everything I have", 4.0, SignalType.ALL_IN),
    ("all in", "all in", 4.0, SignalType.ALL_IN),
    ("alavancagem máxima", "max leverage", 3.5, SignalType.EXCESSIVE_LEVERAGE),
    ("10x", "10x leverage", 3.0, SignalType.EXCESSIVE_LEVERAGE),
    ("20x", "20x leverage", 4.0, SignalType.EXCESSIVE_LEVERAGE),
    ("50x", "50x leverage", 5.0, SignalType.EXCESSIVE_LEVERAGE),
    ("100x", "100x leverage", 5.0, SignalType.EXCESSIVE_LEVERAGE),
    ("não importa o risco", "risk doesn't matter", 4.0, SignalType.IGNORE_RISK),
]

# Patterns that indicate revenge trading
REVENGE_PATTERNS = [
    ("recuperar", "recover", 2.5, SignalType.REVENGE_TRADING),
    ("recuperar perda", "recover loss", 3.5, SignalType.REVENGE_TRADING),
    ("compensar", "compensate", 2.5, SignalType.REVENGE_TRADING),
    ("vingar", "revenge", 4.0, SignalType.REVENGE_TRADING),
    ("dobrar aposta", "double bet", 3.5, SignalType.DOUBLE_DOWN),
    ("dobrar posição", "double position", 3.0, SignalType.DOUBLE_DOWN),
    ("martingale", "martingale", 4.0, SignalType.DOUBLE_DOWN),
]

# Patterns that indicate greed
GREED_PATTERNS = [
    ("to the moon", "to the moon", 2.5, SignalType.MOON_THINKING),
    ("vai pra lua", "going to the moon", 2.5, SignalType.MOON_THINKING),
    ("enriquecer", "get rich", 2.5, SignalType.GET_RICH_QUICK),
    ("ficar rico", "become rich", 2.5, SignalType.GET_RICH_QUICK),
    ("1000%", "1000%", 3.5, SignalType.UNREALISTIC_TARGET),
    ("10000%", "10000%", 4.0, SignalType.UNREALISTIC_TARGET),
    ("lambo", "lambo", 2.5, SignalType.GET_RICH_QUICK),
]

# Patterns that indicate external pressure
PRESSURE_PATTERNS = [
    ("todo mundo", "everyone", 2.0, SignalType.SOCIAL_PRESSURE),
    ("influencer", "influencer", 2.5, SignalType.INFLUENCER_TIP),
    ("youtuber", "youtuber", 2.5, SignalType.INFLUENCER_TIP),
    ("grupo", "group", 1.5, SignalType.GROUP_CONSENSUS),
    ("telegram", "telegram", 1.5, SignalType.GROUP_CONSENSUS),
    ("discord", "discord", 1.5, SignalType.GROUP_CONSENSUS),
]

# Positive patterns (good trading habits)
POSITIVE_PATTERNS = [
    ("stop loss", "stop loss", -2.0, SignalType.HAS_STOP_LOSS),
    ("stop em", "stop at", -2.0, SignalType.HAS_STOP_LOSS),
    ("meu stop", "my stop", -2.0, SignalType.HAS_STOP_LOSS),
    ("entrada em", "entry at", -1.5, SignalType.HAS_ENTRY_PLAN),
    ("ponto de entrada", "entry point", -1.5, SignalType.HAS_ENTRY_PLAN),
    ("target", "target", -1.5, SignalType.HAS_TARGET),
    ("alvo", "target", -1.5, SignalType.HAS_TARGET),
    ("take profit", "take profit", -1.5, SignalType.HAS_TARGET),
    ("1%", "1% risk", -1.0, SignalType.RISK_DEFINED),
    ("risco definido", "risk defined", -1.5, SignalType.RISK_DEFINED),
    ("paciência", "patience", -1.0, SignalType.PATIENT_APPROACH),
    ("aguardar", "wait", -0.5, SignalType.PATIENT_APPROACH),
    ("confirmação", "confirmation", -1.0, SignalType.PATIENT_APPROACH),
]


# ============================================================================
# Risk Level Thresholds
# ============================================================================

RISK_THRESHOLDS = {
    RiskLevel.LOW: 0,
    RiskLevel.MEDIUM: 15,
    RiskLevel.HIGH: 35,
    RiskLevel.CRITICAL: 60,
}


def calculate_risk_level(score: float) -> RiskLevel:
    """
    Determine risk level based on score.
    
    Args:
        score: Numerical risk score
        
    Returns:
        RiskLevel enum value
    """
    if score >= RISK_THRESHOLDS[RiskLevel.CRITICAL]:
        return RiskLevel.CRITICAL
    elif score >= RISK_THRESHOLDS[RiskLevel.HIGH]:
        return RiskLevel.HIGH
    elif score >= RISK_THRESHOLDS[RiskLevel.MEDIUM]:
        return RiskLevel.MEDIUM
    else:
        return RiskLevel.LOW


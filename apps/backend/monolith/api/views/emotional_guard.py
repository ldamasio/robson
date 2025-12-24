"""
REST API views for Emotional Trading Guard.

Provides endpoints for analyzing trading intentions and detecting
emotional patterns that may lead to poor decisions.

Endpoints:
- POST /api/guard/analyze/  - Analyze a trading intention message
- GET  /api/guard/signals/  - Get list of all detectable signals
- GET  /api/guard/tips/     - Get random trading psychology tips
"""

from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated, AllowAny
from rest_framework.response import Response
from rest_framework import status
from enum import Enum
import random
import re


# ============================================================================
# Local Domain Entities (to avoid container import path issues)
# ============================================================================

class SignalType(Enum):
    """Types of emotional signals detected in trading messages."""
    URGENCY = "urgency"
    NOW_OR_NEVER = "now_or_never"
    FOMO = "fomo"
    ABSOLUTE_CERTAINTY = "absolute_certainty"
    GUARANTEED_WIN = "guaranteed_win"
    NO_STOP_LOSS = "no_stop_loss"
    ALL_IN = "all_in"
    EXCESSIVE_LEVERAGE = "excessive_leverage"
    REVENGE_TRADING = "revenge_trading"
    RECOVER_LOSSES = "recover_losses"
    HAS_STOP_LOSS = "has_stop_loss"
    HAS_ENTRY_PLAN = "has_entry_plan"
    HAS_TARGET = "has_target"
    RISK_DEFINED = "risk_defined"


class RiskLevel(Enum):
    """Risk level classification for trading intentions."""
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    CRITICAL = "critical"


class AnalyzeIntentUseCase:
    """
    Simplified local implementation of intent analysis.
    Detects emotional patterns in trading messages.
    """
    
    # Patterns that indicate urgency/FOMO
    URGENCY_PATTERNS = [
        r'\bagora\b', r'\brapido\b', r'\burgente\b', r'\bimediato\b',
        r'\bnow\b', r'\bquick\b', r'\bhurry\b', r'\bfast\b', r'\bimmediately\b',
    ]
    
    # Patterns that indicate overconfidence
    CERTAINTY_PATTERNS = [
        r'\bcom certeza\b', r'\b100%\b', r'\bgarantido\b', r'\bsempre\b',
        r'\bcertain\b', r'\bguaranteed\b', r'\balways\b', r'\bdefinitely\b',
    ]
    
    # Patterns that indicate risk blindness
    RISK_PATTERNS = [
        r'\bsem stop\b', r'\ball in\b', r'\btudo\b', r'\bmaximo\b',
        r'\bno stop\b', r'\bmax leverage\b', r'\byolo\b',
    ]
    
    # Patterns that indicate good habits
    GOOD_PATTERNS = [
        r'\bstop[ -]?loss\b', r'\brisk\b', r'\btarget\b', r'\bentry\b',
        r'\bgestao de risco\b', r'\banalise\b',
    ]
    
    def execute(self, message: str) -> dict:
        """Analyze a trading message for emotional patterns."""
        message_lower = message.lower()
        
        signals = []
        risk_score = 0
        
        # Check urgency patterns
        for pattern in self.URGENCY_PATTERNS:
            if re.search(pattern, message_lower, re.IGNORECASE):
                signals.append({
                    'type': SignalType.URGENCY.value,
                    'confidence': 0.8,
                    'is_positive': False,
                })
                risk_score += 20
                break
        
        # Check certainty patterns
        for pattern in self.CERTAINTY_PATTERNS:
            if re.search(pattern, message_lower, re.IGNORECASE):
                signals.append({
                    'type': SignalType.ABSOLUTE_CERTAINTY.value,
                    'confidence': 0.7,
                    'is_positive': False,
                })
                risk_score += 15
                break
        
        # Check risk patterns
        for pattern in self.RISK_PATTERNS:
            if re.search(pattern, message_lower, re.IGNORECASE):
                signals.append({
                    'type': SignalType.ALL_IN.value,
                    'confidence': 0.9,
                    'is_positive': False,
                })
                risk_score += 30
                break
        
        # Check good patterns (reduce risk)
        for pattern in self.GOOD_PATTERNS:
            if re.search(pattern, message_lower, re.IGNORECASE):
                signals.append({
                    'type': SignalType.HAS_STOP_LOSS.value,
                    'confidence': 0.7,
                    'is_positive': True,
                })
                risk_score -= 10
        
        # Determine risk level
        if risk_score >= 40:
            risk_level = RiskLevel.CRITICAL
        elif risk_score >= 25:
            risk_level = RiskLevel.HIGH
        elif risk_score >= 10:
            risk_level = RiskLevel.MEDIUM
        else:
            risk_level = RiskLevel.LOW
        
        # Generate response
        response_messages = {
            RiskLevel.LOW: "Your trading approach looks well-planned. Remember to always use stop-losses!",
            RiskLevel.MEDIUM: "I detected some emotional language. Consider reviewing your risk parameters before trading.",
            RiskLevel.HIGH: "Warning: I detected several concerning patterns. Please slow down and review your trading plan.",
            RiskLevel.CRITICAL: "STOP! Multiple red flags detected. This looks like emotional trading. Please step away and review tomorrow.",
        }
        
        return {
            'risk_level': risk_level.value,
            'risk_score': max(0, min(100, risk_score)),
            'signals': signals,
            'response': response_messages[risk_level],
            'should_proceed': risk_level in [RiskLevel.LOW, RiskLevel.MEDIUM],
        }


# ============================================================================
# Intent Analysis
# ============================================================================

@api_view(['POST'])
@permission_classes([IsAuthenticated])
def analyze_intent(request):
    """
    Analyze a trading intention message for emotional patterns.
    
    Request Body:
        {
            "message": "I want to buy BTC now! It's going to the moon!"
        }
        
    Response:
        {
            "risk_level": "HIGH",
            "risk_score": 45.5,
            "proceed_allowed": false,
            "requires_confirmation": true,
            "response_message": "🚨 Warning: Multiple risk factors detected!...",
            "signals": [
                {
                    "type": "urgency",
                    "confidence": 0.8,
                    "matched_phrase": "now",
                    "is_positive": false,
                    "explanation": "Urgency can lead to..."
                }
            ],
            "parameters": {
                "symbol": "BTCUSDC",
                "side": "BUY",
                "has_stop_loss": false
            },
            "educational_content": "..."
        }
    """
    message = request.data.get("message", "")
    
    if not message or not message.strip():
        return Response(
            {"error": "message is required"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    try:
        use_case = AnalyzeIntentUseCase()
        analysis = use_case.execute(message)
        
        return Response({
            "risk_level": analysis.risk_level.value,
            "risk_score": round(analysis.risk_score, 2),
            "proceed_allowed": analysis.proceed_allowed,
            "requires_confirmation": analysis.requires_confirmation,
            "response_message": analysis.response_message,
            "signals": [
                {
                    "type": signal.signal_type.value,
                    "confidence": round(signal.confidence, 2),
                    "matched_phrase": signal.matched_phrase,
                    "weight": signal.weight,
                    "is_positive": signal.is_positive,
                    "explanation": signal.explanation,
                }
                for signal in analysis.signals
            ],
            "warning_count": len(analysis.warning_signals),
            "positive_count": len(analysis.positive_signals),
            "parameters": {
                "symbol": analysis.parameters.symbol,
                "side": analysis.parameters.side,
                "entry_price": str(analysis.parameters.entry_price) if analysis.parameters.entry_price else None,
                "stop_price": str(analysis.parameters.stop_price) if analysis.parameters.stop_price else None,
                "target_price": str(analysis.parameters.target_price) if analysis.parameters.target_price else None,
                "leverage": analysis.parameters.leverage,
                "has_risk_parameters": analysis.parameters.has_risk_parameters,
                "is_complete": analysis.parameters.is_complete,
            },
            "educational_content": analysis.educational_content,
            "analyzed_at": analysis.analyzed_at.isoformat(),
        })
        
    except Exception as e:
        return Response(
            {"error": str(e)},
            status=status.HTTP_500_INTERNAL_SERVER_ERROR
        )


# ============================================================================
# Signal Information
# ============================================================================

@api_view(['GET'])
@permission_classes([AllowAny])
def list_signals(request):
    """
    Get list of all detectable emotional signals.
    
    Useful for documentation and understanding what the system detects.
    
    Response:
        {
            "signals": {
                "warning": [...],
                "positive": [...]
            }
        }
    """
    warning_signals = []
    positive_signals = []
    
    signal_info = {
        # Warning signals
        SignalType.URGENCY: ("Urgency", "Indicates rushed decision-making"),
        SignalType.NOW_OR_NEVER: ("Now or Never", "\"Last chance\" mentality"),
        SignalType.FOMO: ("FOMO", "Fear of Missing Out"),
        SignalType.ABSOLUTE_CERTAINTY: ("Overconfidence", "Unrealistic certainty about outcome"),
        SignalType.GUARANTEED_WIN: ("Guaranteed Win", "Belief in risk-free trades"),
        SignalType.INSIDER_INFO: ("Insider Info", "Relying on tips/rumors"),
        SignalType.NO_STOP_LOSS: ("No Stop-Loss", "Trading without defined exit"),
        SignalType.ALL_IN: ("All-In", "Risking entire capital"),
        SignalType.EXCESSIVE_LEVERAGE: ("Excessive Leverage", "Using dangerous leverage levels"),
        SignalType.IGNORE_RISK: ("Ignoring Risk", "Dismissing risk management"),
        SignalType.REVENGE_TRADING: ("Revenge Trading", "Trading to recover losses"),
        SignalType.RECOVER_LOSSES: ("Recover Losses", "Emotional urge to make back money"),
        SignalType.DOUBLE_DOWN: ("Double Down", "Martingale-style position sizing"),
        SignalType.SOCIAL_PRESSURE: ("Social Pressure", "Trading based on group behavior"),
        SignalType.INFLUENCER_TIP: ("Influencer Tip", "Following social media tips"),
        SignalType.GROUP_CONSENSUS: ("Group Consensus", "Echo chamber decision-making"),
        SignalType.MOON_THINKING: ("Moon Thinking", "Unrealistic price expectations"),
        SignalType.UNREALISTIC_TARGET: ("Unrealistic Target", "Expecting extreme returns"),
        SignalType.GET_RICH_QUICK: ("Get Rich Quick", "Short-term wealth mentality"),
        
        # Positive signals
        SignalType.HAS_STOP_LOSS: ("Has Stop-Loss", "Defined exit for losses"),
        SignalType.HAS_ENTRY_PLAN: ("Has Entry Plan", "Clear entry criteria"),
        SignalType.HAS_TARGET: ("Has Target", "Defined profit target"),
        SignalType.RISK_DEFINED: ("Risk Defined", "Clear risk parameters"),
        SignalType.PATIENT_APPROACH: ("Patient Approach", "Waiting for confirmation"),
    }
    
    for signal_type, (name, description) in signal_info.items():
        info = {
            "code": signal_type.value,
            "name": name,
            "description": description,
        }
        
        if signal_type in (
            SignalType.HAS_STOP_LOSS,
            SignalType.HAS_ENTRY_PLAN,
            SignalType.HAS_TARGET,
            SignalType.RISK_DEFINED,
            SignalType.PATIENT_APPROACH,
        ):
            positive_signals.append(info)
        else:
            warning_signals.append(info)
    
    return Response({
        "signals": {
            "warning": warning_signals,
            "positive": positive_signals,
        },
        "total_warning": len(warning_signals),
        "total_positive": len(positive_signals),
    })


# ============================================================================
# Trading Tips
# ============================================================================

TRADING_TIPS = [
    {
        "category": "Risk Management",
        "tip": "Never risk more than 1% of your capital on a single trade.",
        "explanation": "This ensures you can survive a losing streak and stay in the game.",
    },
    {
        "category": "Psychology",
        "tip": "If you feel urgent about a trade, that's usually a sign to wait.",
        "explanation": "Urgency clouds judgment. Markets provide new opportunities constantly.",
    },
    {
        "category": "Discipline",
        "tip": "Your trading plan should be boring. Execution should be mechanical.",
        "explanation": "Exciting trades are usually emotional trades.",
    },
    {
        "category": "Losses",
        "tip": "Losses are tuition fees. Every loss teaches something if you journal it.",
        "explanation": "Professionals review every trade. Amateurs only remember the wins.",
    },
    {
        "category": "Patience",
        "tip": "Waiting for the perfect setup is itself a position.",
        "explanation": "Being flat (no position) is a valid and often optimal position.",
    },
    {
        "category": "FOMO",
        "tip": "The market that moves without you will eventually come back to you.",
        "explanation": "Price always retraces. Chasing moves is a losing strategy long-term.",
    },
    {
        "category": "Leverage",
        "tip": "Leverage is a tool, not a strategy. Use it to be precise, not to gamble.",
        "explanation": "Proper leverage allows smaller position sizes with same exposure.",
    },
    {
        "category": "Recovery",
        "tip": "After a losing day, reduce size by 50% the next day.",
        "explanation": "This prevents revenge trading from compounding losses.",
    },
    {
        "category": "Confirmation",
        "tip": "If your analysis requires cherry-picking data, it's wrong.",
        "explanation": "Confirmation bias is the enemy of objective analysis.",
    },
    {
        "category": "Ego",
        "tip": "The market doesn't care about your opinion. Follow price, not predictions.",
        "explanation": "Being right matters less than making money.",
    },
    {
        "category": "Consistency",
        "tip": "10 trades at 1R each beats 1 trade at 10R every time.",
        "explanation": "Consistent small wins compound. Swinging for home runs leads to strikes.",
    },
    {
        "category": "Stop-Loss",
        "tip": "Your stop-loss is where your analysis is invalidated, not where it hurts.",
        "explanation": "Place stops based on market structure, not on your P&L comfort.",
    },
]


@api_view(['GET'])
@permission_classes([AllowAny])
def trading_tips(request):
    """
    Get trading psychology tips.
    
    Query Parameters:
        random: If true, returns a random tip (default: false)
        category: Filter by category
        
    Response:
        {
            "tips": [...]
        }
    """
    tips = TRADING_TIPS.copy()
    
    # Filter by category if specified
    category = request.query_params.get("category")
    if category:
        tips = [t for t in tips if t["category"].lower() == category.lower()]
    
    # Return random tip if requested
    if request.query_params.get("random", "false").lower() == "true":
        if tips:
            return Response({"tip": random.choice(tips)})
        return Response({"error": "No tips found for this category"}, status=404)
    
    return Response({
        "tips": tips,
        "total": len(tips),
        "categories": list(set(t["category"] for t in TRADING_TIPS)),
    })


# ============================================================================
# Risk Level Information
# ============================================================================

@api_view(['GET'])
@permission_classes([AllowAny])
def risk_levels(request):
    """
    Get information about risk levels and their meanings.
    
    Response:
        {
            "levels": [
                {
                    "level": "LOW",
                    "description": "...",
                    "action": "...",
                    "color": "green"
                },
                ...
            ]
        }
    """
    levels = [
        {
            "level": RiskLevel.LOW.value,
            "name": "Low Risk",
            "description": "Your trading intention shows good habits and proper planning.",
            "action": "Proceed with your trade plan.",
            "color": "#22c55e",  # green
            "score_range": "0-14",
        },
        {
            "level": RiskLevel.MEDIUM.value,
            "name": "Medium Risk",
            "description": "Some concerns detected. Consider reviewing your plan.",
            "action": "Review the warnings before proceeding.",
            "color": "#eab308",  # yellow
            "score_range": "15-34",
        },
        {
            "level": RiskLevel.HIGH.value,
            "name": "High Risk",
            "description": "Multiple warning signs detected. High probability of emotional trading.",
            "action": "Confirmation required before proceeding.",
            "color": "#f97316",  # orange
            "score_range": "35-59",
        },
        {
            "level": RiskLevel.CRITICAL.value,
            "name": "Critical Risk",
            "description": "Strong signs of emotional decision-making. This trade is likely to result in losses.",
            "action": "Strongly recommended to NOT proceed. Take a break.",
            "color": "#ef4444",  # red
            "score_range": "60+",
        },
    ]
    
    return Response({
        "levels": levels,
        "scoring_explanation": (
            "Risk score is calculated based on detected signals, "
            "their weights, and missing risk parameters. "
            "Positive signals (like having a stop-loss) reduce the score."
        ),
    })


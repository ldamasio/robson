"""
Emotional Trading Guard Use Case.

Application layer for analyzing trading intentions and detecting
emotional patterns that may lead to poor trading decisions.

This module implements the core logic that:
1. Receives a user's trading message
2. Detects emotional warning signals
3. Extracts trading parameters
4. Calculates risk level
5. Generates an appropriate response
"""

import re
from decimal import Decimal, InvalidOperation
from typing import Optional

from apps.backend.core.domain.emotional_guard import (
    SignalType,
    RiskLevel,
    EmotionalSignal,
    ExtractedParameters,
    IntentAnalysis,
    URGENCY_PATTERNS,
    OVERCONFIDENCE_PATTERNS,
    RISK_BLINDNESS_PATTERNS,
    REVENGE_PATTERNS,
    GREED_PATTERNS,
    PRESSURE_PATTERNS,
    POSITIVE_PATTERNS,
    calculate_risk_level,
)


class SignalDetector:
    """
    Detects emotional signals in trading messages.
    
    Uses pattern matching to identify warning signs and positive
    trading habits in user messages.
    """
    
    def __init__(self):
        """Initialize with all pattern categories."""
        self.all_patterns = (
            URGENCY_PATTERNS +
            OVERCONFIDENCE_PATTERNS +
            RISK_BLINDNESS_PATTERNS +
            REVENGE_PATTERNS +
            GREED_PATTERNS +
            PRESSURE_PATTERNS +
            POSITIVE_PATTERNS
        )
    
    def detect(self, message: str) -> list[EmotionalSignal]:
        """
        Detect all emotional signals in a message.
        
        Args:
            message: User's trading intention message
            
        Returns:
            List of detected EmotionalSignal objects
        """
        signals = []
        message_lower = message.lower()
        
        for pattern in self.all_patterns:
            pt_term, en_term, weight, signal_type = pattern
            
            # Check Portuguese term
            if pt_term.lower() in message_lower:
                confidence = self._calculate_confidence(pt_term, message_lower)
                is_positive = weight < 0
                explanation = self._get_explanation(signal_type, is_positive)
                
                signals.append(EmotionalSignal(
                    signal_type=signal_type,
                    confidence=confidence,
                    matched_phrase=pt_term,
                    weight=abs(weight),
                    is_positive=is_positive,
                    explanation=explanation,
                ))
            
            # Check English term (different from Portuguese)
            elif en_term.lower() != pt_term.lower() and en_term.lower() in message_lower:
                confidence = self._calculate_confidence(en_term, message_lower)
                is_positive = weight < 0
                explanation = self._get_explanation(signal_type, is_positive)
                
                signals.append(EmotionalSignal(
                    signal_type=signal_type,
                    confidence=confidence,
                    matched_phrase=en_term,
                    weight=abs(weight),
                    is_positive=is_positive,
                    explanation=explanation,
                ))
        
        return signals
    
    def _calculate_confidence(self, term: str, message: str) -> float:
        """
        Calculate confidence based on context.
        
        Higher confidence if the term is emphasized or repeated.
        """
        base_confidence = 0.7
        
        # Increase confidence for uppercase
        if term.upper() in message.upper() and not term.islower():
            base_confidence += 0.1
        
        # Increase for exclamation marks nearby
        if "!" in message:
            base_confidence += 0.1
        
        # Increase for repetition
        count = message.count(term.lower())
        if count > 1:
            base_confidence += min(0.1 * (count - 1), 0.2)
        
        return min(base_confidence, 1.0)
    
    def _get_explanation(self, signal_type: SignalType, is_positive: bool) -> str:
        """Get human-readable explanation for a signal type."""
        explanations = {
            # Warning signals
            SignalType.URGENCY: "Urgency can lead to impulsive decisions without proper analysis.",
            SignalType.NOW_OR_NEVER: "\"Now or never\" thinking ignores that markets always provide new opportunities.",
            SignalType.FOMO: "Fear of Missing Out (FOMO) is one of the most dangerous trading emotions.",
            SignalType.ABSOLUTE_CERTAINTY: "No trade is 100% certain. Overconfidence leads to excessive risk.",
            SignalType.GUARANTEED_WIN: "There are no guaranteed wins in trading. This mindset leads to disaster.",
            SignalType.INSIDER_INFO: "Relying on \"insider info\" is both risky and potentially illegal.",
            SignalType.NO_STOP_LOSS: "Trading without a stop-loss exposes you to unlimited losses.",
            SignalType.ALL_IN: "Risking all your capital on one trade is the fastest path to ruin.",
            SignalType.EXCESSIVE_LEVERAGE: "High leverage amplifies both gains and losses exponentially.",
            SignalType.IGNORE_RISK: "Ignoring risk management is the #1 cause of blown accounts.",
            SignalType.REVENGE_TRADING: "Trying to recover losses emotionally often leads to bigger losses.",
            SignalType.RECOVER_LOSSES: "The urge to \"make it back\" clouds judgment and increases risk.",
            SignalType.DOUBLE_DOWN: "Doubling down after a loss (martingale) is mathematically flawed.",
            SignalType.SOCIAL_PRESSURE: "Trading based on what others are doing ignores your own analysis.",
            SignalType.INFLUENCER_TIP: "Influencers often have different risk tolerance and hidden agendas.",
            SignalType.GROUP_CONSENSUS: "Group consensus can create dangerous echo chambers.",
            SignalType.MOON_THINKING: "\"To the moon\" thinking ignores realistic price targets.",
            SignalType.UNREALISTIC_TARGET: "Unrealistic targets lead to holding losing positions too long.",
            SignalType.GET_RICH_QUICK: "Get-rich-quick mentality leads to excessive risk and disappointment.",
            
            # Positive signals
            SignalType.HAS_STOP_LOSS: "Excellent! You have a defined stop-loss to limit potential losses.",
            SignalType.HAS_ENTRY_PLAN: "Good! You have a specific entry point planned.",
            SignalType.HAS_TARGET: "Great! You have a profit target defined.",
            SignalType.RISK_DEFINED: "Smart! You're thinking about risk management.",
            SignalType.PATIENT_APPROACH: "Wise! Patience is a key trait of successful traders.",
        }
        return explanations.get(signal_type, "")


class ParameterExtractor:
    """
    Extracts trading parameters from user messages.
    
    Uses regex patterns to identify symbols, prices, leverage, etc.
    """
    
    # Common trading pair patterns
    SYMBOL_PATTERNS = [
        r'\b(BTC|ETH|SOL|XRP|ADA|DOGE|AVAX|DOT|LINK|MATIC)(USDT|USDC|BUSD)?\b',
        r'\b(bitcoin|ethereum|solana)\b',
    ]
    
    # Price patterns
    PRICE_PATTERN = r'\$?\s*(\d{1,3}(?:[,.]?\d{3})*(?:[.,]\d+)?)\s*(?:USD|USDT|USDC)?'
    
    # Leverage patterns
    LEVERAGE_PATTERN = r'(\d+)[xX]\s*(?:leverage|alavancagem)?'
    
    # Percentage patterns
    PERCENT_PATTERN = r'(\d+(?:\.\d+)?)\s*%'
    
    def extract(self, message: str) -> ExtractedParameters:
        """
        Extract trading parameters from a message.
        
        Args:
            message: User's trading intention message
            
        Returns:
            ExtractedParameters with extracted values
        """
        message_upper = message.upper()
        
        # Extract symbol
        symbol = self._extract_symbol(message_upper)
        
        # Extract side (BUY/SELL)
        side = self._extract_side(message)
        
        # Extract prices
        prices = self._extract_prices(message)
        entry_price = prices.get('entry')
        stop_price = prices.get('stop')
        target_price = prices.get('target')
        
        # Extract leverage
        leverage = self._extract_leverage(message)
        
        # Extract capital percentage
        capital_percent = self._extract_capital_percent(message)
        
        return ExtractedParameters(
            symbol=symbol,
            side=side,
            entry_price=entry_price,
            stop_price=stop_price,
            target_price=target_price,
            leverage=leverage,
            capital_percent=capital_percent,
        )
    
    def _extract_symbol(self, message: str) -> Optional[str]:
        """Extract trading pair symbol."""
        for pattern in self.SYMBOL_PATTERNS:
            match = re.search(pattern, message, re.IGNORECASE)
            if match:
                symbol = match.group(0).upper()
                # Normalize common symbols
                if symbol in ("BITCOIN", "BTC"):
                    return "BTCUSDC"
                elif symbol in ("ETHEREUM", "ETH"):
                    return "ETHUSDC"
                elif symbol in ("SOLANA", "SOL"):
                    return "SOLUSDC"
                elif "USDT" in symbol or "USDC" in symbol or "BUSD" in symbol:
                    return symbol
                else:
                    return f"{symbol}USDC"
        return None
    
    def _extract_side(self, message: str) -> Optional[str]:
        """Extract trade side (BUY/SELL)."""
        message_lower = message.lower()
        
        buy_terms = ["buy", "comprar", "long", "compra", "entrada"]
        sell_terms = ["sell", "vender", "short", "venda", "saída"]
        
        for term in buy_terms:
            if term in message_lower:
                return "BUY"
        
        for term in sell_terms:
            if term in message_lower:
                return "SELL"
        
        return None
    
    def _extract_prices(self, message: str) -> dict[str, Optional[Decimal]]:
        """Extract entry, stop, and target prices."""
        result = {'entry': None, 'stop': None, 'target': None}
        message_lower = message.lower()
        
        # Find all numbers that look like prices
        price_matches = re.findall(self.PRICE_PATTERN, message, re.IGNORECASE)
        prices = []
        for match in price_matches:
            try:
                # Clean the number (remove commas, handle decimals)
                clean = match.replace(',', '').replace('.', '', match.count('.') - 1)
                if '.' not in clean and len(match) > 3:
                    clean = clean[:-2] + '.' + clean[-2:]
                prices.append(Decimal(clean.replace(',', '')))
            except (InvalidOperation, ValueError):
                continue
        
        # Try to identify which price is which based on context
        lines = message_lower.split('\n')
        for line in lines:
            for i, price in enumerate(prices):
                price_str = str(price)
                if price_str in line or f"${price_str}" in line:
                    if any(term in line for term in ['stop', 'sl', 'perda']):
                        result['stop'] = price
                    elif any(term in line for term in ['target', 'tp', 'alvo', 'lucro']):
                        result['target'] = price
                    elif any(term in line for term in ['entry', 'entrada', 'comprar', 'buy']):
                        result['entry'] = price
        
        # If we have prices but couldn't categorize, use heuristics
        if not result['entry'] and prices:
            result['entry'] = prices[0]
        
        return result
    
    def _extract_leverage(self, message: str) -> Optional[int]:
        """Extract leverage multiplier."""
        match = re.search(self.LEVERAGE_PATTERN, message, re.IGNORECASE)
        if match:
            try:
                return int(match.group(1))
            except ValueError:
                pass
        return None
    
    def _extract_capital_percent(self, message: str) -> Optional[Decimal]:
        """Extract capital percentage to risk."""
        message_lower = message.lower()
        
        # Look for percentage near risk-related terms
        for term in ['capital', 'risco', 'risk', 'portfólio', 'portfolio']:
            if term in message_lower:
                match = re.search(self.PERCENT_PATTERN, message)
                if match:
                    try:
                        return Decimal(match.group(1))
                    except (InvalidOperation, ValueError):
                        pass
        return None


class ResponseGenerator:
    """
    Generates appropriate responses based on analysis results.
    
    Crafts messages that are educational, not judgmental,
    and help traders make better decisions.
    """
    
    def generate(self, analysis: IntentAnalysis) -> str:
        """
        Generate a response message based on the analysis.
        
        Args:
            analysis: Completed intent analysis
            
        Returns:
            Human-readable response string
        """
        if analysis.risk_level == RiskLevel.LOW:
            return self._generate_low_risk_response(analysis)
        elif analysis.risk_level == RiskLevel.MEDIUM:
            return self._generate_medium_risk_response(analysis)
        elif analysis.risk_level == RiskLevel.HIGH:
            return self._generate_high_risk_response(analysis)
        else:  # CRITICAL
            return self._generate_critical_risk_response(analysis)
    
    def _generate_low_risk_response(self, analysis: IntentAnalysis) -> str:
        """Response for low-risk (good) trading intentions."""
        response = "✅ **Your trading intention looks well-planned!**\n\n"
        
        if analysis.positive_signals:
            response += "**Good practices detected:**\n"
            for signal in analysis.positive_signals[:3]:
                response += f"• {signal.explanation}\n"
            response += "\n"
        
        if analysis.parameters.is_complete:
            response += "**Your trade parameters:**\n"
            p = analysis.parameters
            response += f"• Symbol: {p.symbol}\n"
            response += f"• Side: {p.side}\n"
            response += f"• Entry: ${p.entry_price}\n"
            response += f"• Stop: ${p.stop_price}\n"
            if p.target_price:
                response += f"• Target: ${p.target_price}\n"
        
        response += "\n*Remember: Even well-planned trades can lose. Never risk more than 1% of your capital.*"
        
        return response
    
    def _generate_medium_risk_response(self, analysis: IntentAnalysis) -> str:
        """Response for medium-risk trading intentions."""
        response = "⚠️ **Some concerns detected in your trading intention**\n\n"
        
        if analysis.warning_signals:
            response += "**Points to consider:**\n"
            for signal in analysis.warning_signals[:3]:
                response += f"• {signal.explanation}\n"
            response += "\n"
        
        if not analysis.parameters.has_risk_parameters:
            response += "**Missing risk parameters:**\n"
            response += "• No stop-loss defined. Consider where you would exit if wrong.\n\n"
        
        response += "**Recommendation:** Take a few minutes to review your plan before executing.\n\n"
        response += "*Would you like me to help you calculate proper position sizing?*"
        
        return response
    
    def _generate_high_risk_response(self, analysis: IntentAnalysis) -> str:
        """Response for high-risk trading intentions."""
        response = "🚨 **Warning: Multiple risk factors detected!**\n\n"
        
        response += "**Concerning patterns:**\n"
        for signal in analysis.warning_signals[:5]:
            response += f"• **{signal.signal_type.value}**: {signal.explanation}\n"
        response += "\n"
        
        response += "**Before proceeding, please:**\n"
        response += "1. Define a clear stop-loss level\n"
        response += "2. Limit your risk to 1% of capital maximum\n"
        response += "3. Wait 10 minutes and ask yourself: \"Am I reacting emotionally?\"\n\n"
        
        response += "**⚠️ Robson requires confirmation before allowing this trade.**\n\n"
        response += "*Type \"I understand the risks and want to proceed\" to continue, "
        response += "or let me help you plan a safer approach.*"
        
        return response
    
    def _generate_critical_risk_response(self, analysis: IntentAnalysis) -> str:
        """Response for critical-risk trading intentions."""
        response = "🛑 **STOP: This trade shows signs of emotional decision-making**\n\n"
        
        response += "Robson has detected multiple red flags:\n\n"
        
        for signal in analysis.warning_signals[:5]:
            response += f"❌ **{signal.signal_type.value.replace('_', ' ').title()}**\n"
            response += f"   {signal.explanation}\n\n"
        
        response += "---\n\n"
        response += "**The statistics are clear:**\n"
        response += "• 90% of traders who trade emotionally lose money\n"
        response += "• Revenge trading after a loss typically leads to bigger losses\n"
        response += "• FOMO trades have a much lower win rate\n\n"
        
        response += "**Robson's recommendation: DO NOT execute this trade.**\n\n"
        
        response += "Instead:\n"
        response += "1. 🧘 Close the charts and take a 30-minute break\n"
        response += "2. 📝 Journal what you're feeling right now\n"
        response += "3. 💡 Come back with a written trade plan\n\n"
        
        response += "*I'm here to help you succeed, and sometimes that means protecting you from yourself.*"
        
        return response
    
    def get_educational_content(self, signals: list[EmotionalSignal]) -> Optional[str]:
        """Generate educational content based on detected signals."""
        if not signals:
            return None
        
        # Get the most significant signal
        warning_signals = [s for s in signals if not s.is_positive]
        if not warning_signals:
            return None
        
        top_signal = max(warning_signals, key=lambda s: s.weight)
        
        content_map = {
            SignalType.FOMO: """
## Understanding FOMO in Trading

FOMO (Fear of Missing Out) is one of the most destructive emotions in trading.
It causes traders to:
- Enter positions without proper analysis
- Chase prices that have already moved
- Ignore their trading rules

**Reality check:** Markets are open 24/7. There will ALWAYS be another opportunity.
The best traders are patient and wait for their setups.
""",
            SignalType.REVENGE_TRADING: """
## The Danger of Revenge Trading

After a loss, the emotional urge to "make it back" is powerful but dangerous.
Revenge trading typically:
- Increases position sizes (more risk)
- Ignores entry criteria (lower probability trades)
- Compounds losses

**Solution:** After any loss, take a mandatory break. Your next trade should be
smaller, not larger. Accept the loss and move forward with discipline.
""",
            SignalType.NO_STOP_LOSS: """
## Why Stop-Losses Are Non-Negotiable

Trading without a stop-loss is like driving without brakes.
It only takes ONE trade to blow an account.

**The math:**
- Losing 50% requires a 100% gain to recover
- Losing 90% requires a 900% gain to recover

**Rule:** Always define your exit BEFORE you enter. If you can't define
where you're wrong, you don't have a valid trade setup.
""",
            SignalType.ALL_IN: """
## The Fatal Flaw of "All-In" Trading

Risking all your capital on one trade is the fastest path to ruin.

**Professional risk management:**
- Risk 1-2% of capital per trade maximum
- This allows you to survive 50+ consecutive losses
- Consistency beats home runs

**Remember:** The goal is to stay in the game. You can't make money
if you've blown your account.
""",
        }
        
        return content_map.get(top_signal.signal_type)


class AnalyzeIntentUseCase:
    """
    Main use case for analyzing trading intentions.
    
    Orchestrates the signal detector, parameter extractor,
    and response generator to produce a complete analysis.
    """
    
    def __init__(
        self,
        signal_detector: Optional[SignalDetector] = None,
        parameter_extractor: Optional[ParameterExtractor] = None,
        response_generator: Optional[ResponseGenerator] = None,
    ):
        """
        Initialize with optional custom implementations.
        
        Args:
            signal_detector: Custom signal detector (or use default)
            parameter_extractor: Custom parameter extractor (or use default)
            response_generator: Custom response generator (or use default)
        """
        self.signal_detector = signal_detector or SignalDetector()
        self.parameter_extractor = parameter_extractor or ParameterExtractor()
        self.response_generator = response_generator or ResponseGenerator()
    
    def execute(self, message: str) -> IntentAnalysis:
        """
        Analyze a trading intention message.
        
        Args:
            message: User's trading intention in natural language
            
        Returns:
            Complete IntentAnalysis with signals, parameters, and response
        """
        if not message or not message.strip():
            return IntentAnalysis(
                original_message=message,
                risk_level=RiskLevel.LOW,
                response_message="Please describe your trading intention.",
            )
        
        # Detect signals
        signals = self.signal_detector.detect(message)
        
        # Extract parameters
        parameters = self.parameter_extractor.extract(message)
        
        # Calculate risk score
        risk_score = self._calculate_risk_score(signals, parameters)
        
        # Determine risk level
        risk_level = calculate_risk_level(risk_score)
        
        # Build analysis
        analysis = IntentAnalysis(
            original_message=message,
            signals=signals,
            parameters=parameters,
            risk_level=risk_level,
            risk_score=risk_score,
            proceed_allowed=risk_level in (RiskLevel.LOW, RiskLevel.MEDIUM),
            requires_confirmation=risk_level == RiskLevel.HIGH,
        )
        
        # Generate response
        analysis.response_message = self.response_generator.generate(analysis)
        
        # Add educational content for concerning signals
        analysis.educational_content = self.response_generator.get_educational_content(signals)
        
        return analysis
    
    def _calculate_risk_score(
        self,
        signals: list[EmotionalSignal],
        parameters: ExtractedParameters,
    ) -> float:
        """
        Calculate a numerical risk score.
        
        Higher score = more dangerous trading intention.
        """
        score = 0.0
        
        # Add signal contributions
        for signal in signals:
            contribution = signal.weight * signal.confidence
            if signal.is_positive:
                score -= contribution
            else:
                score += contribution
        
        # Penalties for missing risk parameters
        if not parameters.has_risk_parameters:
            score += 10  # Significant penalty for no stop-loss
        
        # Penalty for high leverage
        if parameters.leverage and parameters.leverage > 5:
            score += (parameters.leverage - 5) * 3
        
        # Ensure score doesn't go negative
        return max(0, score)


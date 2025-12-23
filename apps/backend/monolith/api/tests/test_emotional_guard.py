"""
Tests for Emotional Trading Guard.

Tests the signal detection, parameter extraction, and response generation.
"""

import pytest
from decimal import Decimal

from apps.backend.core.domain.emotional_guard import (
    SignalType,
    RiskLevel,
    EmotionalSignal,
    ExtractedParameters,
    IntentAnalysis,
    calculate_risk_level,
)
from apps.backend.core.application.emotional_guard_use_case import (
    SignalDetector,
    ParameterExtractor,
    ResponseGenerator,
    AnalyzeIntentUseCase,
)


class TestEmotionalSignal:
    """Tests for EmotionalSignal domain entity."""
    
    def test_create_valid_signal(self):
        """Test creating a valid emotional signal."""
        signal = EmotionalSignal(
            signal_type=SignalType.FOMO,
            confidence=0.8,
            matched_phrase="vai explodir",
            weight=2.5,
            is_positive=False,
            explanation="Fear of Missing Out detected",
        )
        
        assert signal.signal_type == SignalType.FOMO
        assert signal.confidence == 0.8
        assert signal.weight == 2.5
        assert not signal.is_positive
    
    def test_confidence_validation(self):
        """Test that confidence must be between 0 and 1."""
        with pytest.raises(ValueError):
            EmotionalSignal(
                signal_type=SignalType.URGENCY,
                confidence=1.5,  # Invalid
                matched_phrase="now",
            )
    
    def test_weight_validation(self):
        """Test that weight must be non-negative."""
        with pytest.raises(ValueError):
            EmotionalSignal(
                signal_type=SignalType.URGENCY,
                confidence=0.5,
                matched_phrase="now",
                weight=-1.0,  # Invalid
            )


class TestExtractedParameters:
    """Tests for ExtractedParameters domain entity."""
    
    def test_has_risk_parameters_true(self):
        """Test has_risk_parameters when stop is defined."""
        params = ExtractedParameters(
            symbol="BTCUSDC",
            side="BUY",
            entry_price=Decimal("100000"),
            stop_price=Decimal("98000"),
        )
        
        assert params.has_risk_parameters is True
    
    def test_has_risk_parameters_false(self):
        """Test has_risk_parameters when stop is missing."""
        params = ExtractedParameters(
            symbol="BTCUSDC",
            side="BUY",
            entry_price=Decimal("100000"),
        )
        
        assert params.has_risk_parameters is False
    
    def test_is_complete(self):
        """Test is_complete when all required fields are present."""
        complete = ExtractedParameters(
            symbol="BTCUSDC",
            side="BUY",
            entry_price=Decimal("100000"),
            stop_price=Decimal("98000"),
        )
        
        incomplete = ExtractedParameters(
            symbol="BTCUSDC",
            side="BUY",
        )
        
        assert complete.is_complete is True
        assert incomplete.is_complete is False


class TestIntentAnalysis:
    """Tests for IntentAnalysis domain entity."""
    
    def test_warning_signals_filter(self):
        """Test filtering warning signals."""
        analysis = IntentAnalysis(
            original_message="test",
            signals=[
                EmotionalSignal(SignalType.FOMO, 0.8, "fomo", is_positive=False),
                EmotionalSignal(SignalType.HAS_STOP_LOSS, 0.9, "stop", is_positive=True),
                EmotionalSignal(SignalType.URGENCY, 0.7, "now", is_positive=False),
            ],
        )
        
        assert len(analysis.warning_signals) == 2
        assert len(analysis.positive_signals) == 1
    
    def test_has_warnings(self):
        """Test has_warnings property."""
        with_warnings = IntentAnalysis(
            original_message="test",
            signals=[
                EmotionalSignal(SignalType.FOMO, 0.8, "fomo", is_positive=False),
            ],
        )
        
        without_warnings = IntentAnalysis(
            original_message="test",
            signals=[
                EmotionalSignal(SignalType.HAS_STOP_LOSS, 0.9, "stop", is_positive=True),
            ],
        )
        
        assert with_warnings.has_warnings is True
        assert without_warnings.has_warnings is False


class TestCalculateRiskLevel:
    """Tests for risk level calculation."""
    
    def test_low_risk(self):
        """Test low risk level for low scores."""
        assert calculate_risk_level(0) == RiskLevel.LOW
        assert calculate_risk_level(14) == RiskLevel.LOW
    
    def test_medium_risk(self):
        """Test medium risk level."""
        assert calculate_risk_level(15) == RiskLevel.MEDIUM
        assert calculate_risk_level(34) == RiskLevel.MEDIUM
    
    def test_high_risk(self):
        """Test high risk level."""
        assert calculate_risk_level(35) == RiskLevel.HIGH
        assert calculate_risk_level(59) == RiskLevel.HIGH
    
    def test_critical_risk(self):
        """Test critical risk level."""
        assert calculate_risk_level(60) == RiskLevel.CRITICAL
        assert calculate_risk_level(100) == RiskLevel.CRITICAL


class TestSignalDetector:
    """Tests for SignalDetector."""
    
    @pytest.fixture
    def detector(self):
        return SignalDetector()
    
    def test_detect_urgency(self, detector):
        """Test detecting urgency signals."""
        signals = detector.detect("Preciso comprar AGORA!")
        
        urgency_signals = [s for s in signals if s.signal_type == SignalType.URGENCY]
        assert len(urgency_signals) >= 1
    
    def test_detect_fomo(self, detector):
        """Test detecting FOMO signals."""
        signals = detector.detect("BTC vai explodir, não posso perder!")
        
        fomo_signals = [s for s in signals if s.signal_type == SignalType.FOMO]
        assert len(fomo_signals) >= 1
    
    def test_detect_stop_loss(self, detector):
        """Test detecting positive stop-loss signal."""
        signals = detector.detect("Vou comprar BTC com stop loss em 95k")
        
        stop_signals = [s for s in signals if s.signal_type == SignalType.HAS_STOP_LOSS]
        assert len(stop_signals) >= 1
        assert stop_signals[0].is_positive is True
    
    def test_detect_leverage(self, detector):
        """Test detecting excessive leverage."""
        signals = detector.detect("Quero operar com 20x de alavancagem")
        
        leverage_signals = [s for s in signals if s.signal_type == SignalType.EXCESSIVE_LEVERAGE]
        assert len(leverage_signals) >= 1
    
    def test_detect_revenge_trading(self, detector):
        """Test detecting revenge trading patterns."""
        signals = detector.detect("Preciso recuperar a perda de ontem")
        
        revenge_signals = [s for s in signals if s.signal_type in (
            SignalType.REVENGE_TRADING, SignalType.RECOVER_LOSSES
        )]
        assert len(revenge_signals) >= 1
    
    def test_no_signals_for_neutral_message(self, detector):
        """Test that neutral messages produce few/no signals."""
        signals = detector.detect("O mercado está lateral hoje.")
        
        # Should have no strong warning signals
        warning_signals = [s for s in signals if not s.is_positive]
        assert len(warning_signals) == 0


class TestParameterExtractor:
    """Tests for ParameterExtractor."""
    
    @pytest.fixture
    def extractor(self):
        return ParameterExtractor()
    
    def test_extract_symbol(self, extractor):
        """Test extracting trading symbol."""
        params = extractor.extract("Quero comprar BTCUSDC")
        assert params.symbol == "BTCUSDC"
        
        params = extractor.extract("Buy Bitcoin")
        assert params.symbol == "BTCUSDC"
    
    def test_extract_side_buy(self, extractor):
        """Test extracting BUY side."""
        params = extractor.extract("I want to buy BTC")
        assert params.side == "BUY"
        
        params = extractor.extract("Quero comprar ETH")
        assert params.side == "BUY"
        
        params = extractor.extract("Long BTCUSDC")
        assert params.side == "BUY"
    
    def test_extract_side_sell(self, extractor):
        """Test extracting SELL side."""
        params = extractor.extract("I want to sell BTC")
        assert params.side == "SELL"
        
        params = extractor.extract("Quero vender ETH")
        assert params.side == "SELL"
        
        params = extractor.extract("Short BTCUSDC")
        assert params.side == "SELL"
    
    def test_extract_leverage(self, extractor):
        """Test extracting leverage."""
        params = extractor.extract("Operar com 5x de alavancagem")
        assert params.leverage == 5
        
        params = extractor.extract("10X leverage on BTC")
        assert params.leverage == 10


class TestResponseGenerator:
    """Tests for ResponseGenerator."""
    
    @pytest.fixture
    def generator(self):
        return ResponseGenerator()
    
    def test_low_risk_response(self, generator):
        """Test response for low risk."""
        analysis = IntentAnalysis(
            original_message="Buy BTC with stop at 95k",
            risk_level=RiskLevel.LOW,
            signals=[
                EmotionalSignal(SignalType.HAS_STOP_LOSS, 0.9, "stop", is_positive=True),
            ],
        )
        
        response = generator.generate(analysis)
        
        assert "✅" in response
        assert "well-planned" in response.lower() or "good" in response.lower()
    
    def test_critical_risk_response(self, generator):
        """Test response for critical risk."""
        analysis = IntentAnalysis(
            original_message="All in BTC NOW!",
            risk_level=RiskLevel.CRITICAL,
            signals=[
                EmotionalSignal(SignalType.URGENCY, 0.9, "now", is_positive=False),
                EmotionalSignal(SignalType.ALL_IN, 0.9, "all in", is_positive=False),
            ],
        )
        
        response = generator.generate(analysis)
        
        assert "🛑" in response or "STOP" in response
        assert "DO NOT" in response or "not execute" in response.lower()


class TestAnalyzeIntentUseCase:
    """Tests for AnalyzeIntentUseCase."""
    
    @pytest.fixture
    def use_case(self):
        return AnalyzeIntentUseCase()
    
    def test_analyze_good_intent(self, use_case):
        """Test analyzing a well-planned trade."""
        analysis = use_case.execute(
            "Vou comprar BTC com entrada em 100k, stop em 98k, alvo em 110k. Risco de 1%."
        )
        
        assert analysis.risk_level in (RiskLevel.LOW, RiskLevel.MEDIUM)
        assert analysis.proceed_allowed is True
        assert len(analysis.positive_signals) > 0
    
    def test_analyze_dangerous_intent(self, use_case):
        """Test analyzing a dangerous trade."""
        analysis = use_case.execute(
            "AGORA! BTC vai explodir! Preciso colocar todo meu capital com 20x! "
            "Não preciso de stop, tenho certeza absoluta!"
        )
        
        assert analysis.risk_level in (RiskLevel.HIGH, RiskLevel.CRITICAL)
        assert analysis.proceed_allowed is False or analysis.requires_confirmation is True
        assert len(analysis.warning_signals) > 3
    
    def test_analyze_fomo_intent(self, use_case):
        """Test detecting FOMO-driven trade."""
        analysis = use_case.execute(
            "Todo mundo está comprando! Não posso perder essa oportunidade única! "
            "BTC vai para a lua!"
        )
        
        assert analysis.risk_level in (RiskLevel.MEDIUM, RiskLevel.HIGH, RiskLevel.CRITICAL)
        
        fomo_detected = any(
            s.signal_type in (SignalType.FOMO, SignalType.NOW_OR_NEVER, SignalType.MOON_THINKING)
            for s in analysis.signals
        )
        assert fomo_detected
    
    def test_analyze_empty_message(self, use_case):
        """Test handling empty message."""
        analysis = use_case.execute("")
        
        assert analysis.risk_level == RiskLevel.LOW
        assert "describe" in analysis.response_message.lower()
    
    def test_educational_content_for_fomo(self, use_case):
        """Test that educational content is provided for FOMO."""
        analysis = use_case.execute(
            "Não posso perder! BTC vai explodir! Última chance!"
        )
        
        if analysis.educational_content:
            assert "FOMO" in analysis.educational_content or "missing" in analysis.educational_content.lower()
    
    def test_risk_score_calculation(self, use_case):
        """Test that risk score increases with more warning signals."""
        safe_analysis = use_case.execute(
            "Comprar BTC com stop loss definido, entrada planejada."
        )
        
        dangerous_analysis = use_case.execute(
            "AGORA! All in! 50x! Sem stop! Certeza absoluta!"
        )
        
        assert dangerous_analysis.risk_score > safe_analysis.risk_score


# Execution Plan: Emotional Trading Guard

**EP-ID**: EP-001  
**Title**: Emotional Trading Guard - AI-Powered Risk Sentiment Analysis  
**Status**: Draft  
**Created**: 2024-12-23  
**Priority**: High (P0)  
**Estimated Effort**: 5 days  

---

## 1. Executive Summary

Implement a prompt-based interface on Robson's main dashboard where users can communicate their trading intentions in natural language. The system will analyze the message for **emotional trading signals** (urgency, overconfidence, missing risk parameters) and provide protective feedback before allowing trade execution.

### Core Philosophy

> "The best trade is sometimes no trade. Robson protects users from themselves."

This feature embodies Robson's identity as a **Risk Management Assistant**, not just an order execution system.

---

## 2. Problem Statement

### Current State
- Users can execute trades through forms/API without emotional state assessment
- No protection against impulsive, emotionally-driven trading decisions
- Risk management is passive (position sizing) rather than active (intent analysis)

### Desired State
- Users communicate trading intent through natural language
- System detects emotional/risky patterns before execution
- Protective guardrails prevent impulsive trades
- Educational feedback helps users develop better trading discipline

### Business Value
- Reduced user losses from emotional trading
- Increased user trust in the platform
- Differentiation from "dumb" execution-only platforms
- Alignment with Robson's mission as Risk Management Assistant

---

## 3. Requirements

### 3.1 Functional Requirements

#### REQ-GUARD-001: Prompt Input Interface
**Description**: Main dashboard must include a textarea for users to describe their trading intentions.

**Acceptance Criteria**:
- [ ] Textarea visible on main dashboard for logged-in users
- [ ] Placeholder text guides user (e.g., "What trade are you considering?")
- [ ] Submit button with clear label (e.g., "Analyze Intent")
- [ ] Loading state while processing
- [ ] Response displayed below input

#### REQ-GUARD-002: Emotional Signal Detection
**Description**: Backend must analyze user messages for emotional trading signals.

**Signals to Detect**:

| Signal | Examples | Risk Level |
|--------|----------|------------|
| **Urgency** | "agora", "urgente", "rápido", "now", "immediately" | HIGH |
| **Overconfidence** | "certeza", "garantido", "vai subir", "certain", "definitely" | HIGH |
| **FOMO** | "não posso perder", "última chance", "missing out" | HIGH |
| **Revenge Trading** | "recuperar", "compensar perda", "get back" | CRITICAL |
| **Leverage without params** | "alavancado" + missing stop/entry/target | CRITICAL |
| **Missing Risk Params** | No stop-loss, entry, or target mentioned | MEDIUM |
| **Emotional Language** | "odeio", "amo", "hate", "love", exclamations | MEDIUM |
| **Round Numbers** | "all in", "100%", "tudo" | HIGH |

**Acceptance Criteria**:
- [ ] Detect at least 8 signal categories
- [ ] Assign risk level to each signal (LOW/MEDIUM/HIGH/CRITICAL)
- [ ] Return confidence score for each detected signal
- [ ] Support Portuguese and English

#### REQ-GUARD-003: Protective Response System
**Description**: System must provide appropriate response based on detected signals.

**Response Types**:

| Risk Level | Action |
|------------|--------|
| NONE | Proceed with position sizing calculation |
| LOW | Show gentle reminder about risk management |
| MEDIUM | Show warning with educational content |
| HIGH | Show strong warning, require confirmation |
| CRITICAL | Block trade, require cooling-off period |

**Acceptance Criteria**:
- [ ] Response explains WHY trading is being questioned
- [ ] Response provides educational content
- [ ] High/Critical responses require explicit acknowledgment
- [ ] Critical responses enforce 24-hour cooling-off period

#### REQ-GUARD-004: Trade Intent Extraction
**Description**: Extract structured trading parameters from natural language.

**Parameters to Extract**:
- Symbol (e.g., BTC, ETH)
- Side (BUY/SELL, LONG/SHORT)
- Entry price (if mentioned)
- Stop-loss price (if mentioned)
- Target price (if mentioned)
- Leverage (if mentioned)
- Position size / amount (if mentioned)

**Acceptance Criteria**:
- [ ] Extract at least 5 parameter types
- [ ] Handle various input formats (price, percentage, etc.)
- [ ] Flag missing critical parameters (stop-loss)

#### REQ-GUARD-005: Audit Trail
**Description**: Log all user intents and system responses for analysis.

**Acceptance Criteria**:
- [ ] Log user message (sanitized)
- [ ] Log detected signals and confidence scores
- [ ] Log system response and user action
- [ ] Enable analytics on emotional trading patterns

### 3.2 Non-Functional Requirements

#### REQ-GUARD-NFR-001: Response Time
- Analysis must complete within 2 seconds
- UI must show loading state

#### REQ-GUARD-NFR-002: Privacy
- User messages processed but not stored long-term
- No sharing with third parties
- Anonymized analytics only

#### REQ-GUARD-NFR-003: Internationalization
- Support Portuguese (BR) and English
- Detect language automatically

---

## 4. Technical Design

### 4.1 Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                         FRONTEND (React)                            │
│  ┌─────────────────────────────────────────────────────────────────┐│
│  │                    TradingIntentInput                           ││
│  │  ┌─────────────────────────────────────────────────────────────┐││
│  │  │  [Textarea: "What trade are you considering?"]              │││
│  │  │  [Submit Button: "Analyze Intent"]                          │││
│  │  └─────────────────────────────────────────────────────────────┘││
│  │                              │                                  ││
│  │                              ▼                                  ││
│  │  ┌─────────────────────────────────────────────────────────────┐││
│  │  │              IntentAnalysisResult                           │││
│  │  │  - Risk Level Badge                                         │││
│  │  │  - Detected Signals List                                    │││
│  │  │  - Educational Message                                      │││
│  │  │  - [Proceed] / [Reconsider] Buttons                         │││
│  │  └─────────────────────────────────────────────────────────────┘││
│  └─────────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼ POST /api/intent/analyze/
┌─────────────────────────────────────────────────────────────────────┐
│                         BACKEND (Django)                            │
│  ┌─────────────────────────────────────────────────────────────────┐│
│  │                    IntentAnalysisView                           ││
│  │                          │                                      ││
│  │                          ▼                                      ││
│  │  ┌─────────────────────────────────────────────────────────────┐││
│  │  │              AnalyzeIntentUseCase                           │││
│  │  │                     │                                       │││
│  │  │     ┌───────────────┼───────────────┐                       │││
│  │  │     ▼               ▼               ▼                       │││
│  │  │ SignalDetector  ParamExtractor  ResponseGenerator           │││
│  │  │     │               │               │                       │││
│  │  │     ▼               ▼               ▼                       │││
│  │  │ [Urgency]      [Symbol]      [Risk Level]                   │││
│  │  │ [FOMO]         [Side]        [Message]                      │││
│  │  │ [Overconf]     [Stop]        [Education]                    │││
│  │  └─────────────────────────────────────────────────────────────┘││
│  └─────────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────────┘
```

### 4.2 Domain Model

```python
# apps/backend/core/domain/intent_guard.py

from dataclasses import dataclass, field
from decimal import Decimal
from datetime import datetime
from typing import Optional
from enum import Enum


class RiskLevel(str, Enum):
    """Risk level of detected signals."""
    NONE = "NONE"
    LOW = "LOW"
    MEDIUM = "MEDIUM"
    HIGH = "HIGH"
    CRITICAL = "CRITICAL"


class SignalType(str, Enum):
    """Types of emotional trading signals."""
    URGENCY = "URGENCY"
    OVERCONFIDENCE = "OVERCONFIDENCE"
    FOMO = "FOMO"
    REVENGE_TRADING = "REVENGE_TRADING"
    LEVERAGE_NO_STOP = "LEVERAGE_NO_STOP"
    MISSING_STOP_LOSS = "MISSING_STOP_LOSS"
    MISSING_ENTRY = "MISSING_ENTRY"
    EMOTIONAL_LANGUAGE = "EMOTIONAL_LANGUAGE"
    ALL_IN = "ALL_IN"
    GREED = "GREED"


@dataclass(frozen=True)
class DetectedSignal:
    """A detected emotional trading signal."""
    signal_type: SignalType
    risk_level: RiskLevel
    confidence: float  # 0.0 to 1.0
    matched_text: str
    explanation: str


@dataclass(frozen=True)
class ExtractedParams:
    """Trading parameters extracted from intent."""
    symbol: Optional[str] = None
    side: Optional[str] = None  # BUY, SELL, LONG, SHORT
    entry_price: Optional[Decimal] = None
    stop_price: Optional[Decimal] = None
    target_price: Optional[Decimal] = None
    leverage: Optional[int] = None
    amount: Optional[Decimal] = None
    amount_percent: Optional[Decimal] = None  # % of capital


@dataclass
class IntentAnalysisResult:
    """Result of analyzing a trading intent."""
    # Input
    original_message: str
    detected_language: str
    
    # Signals
    signals: list[DetectedSignal] = field(default_factory=list)
    overall_risk_level: RiskLevel = RiskLevel.NONE
    
    # Extracted params
    params: Optional[ExtractedParams] = None
    missing_params: list[str] = field(default_factory=list)
    
    # Response
    response_message: str = ""
    educational_content: str = ""
    requires_acknowledgment: bool = False
    cooling_off_until: Optional[datetime] = None
    
    # Actions
    can_proceed: bool = True
    recommended_action: str = ""
    
    @property
    def has_warnings(self) -> bool:
        return len(self.signals) > 0
    
    @property
    def is_blocked(self) -> bool:
        return self.overall_risk_level == RiskLevel.CRITICAL
```

### 4.3 Signal Detection Engine

```python
# apps/backend/core/application/signal_detector.py

import re
from typing import List
from apps.backend.core.domain.intent_guard import (
    DetectedSignal, SignalType, RiskLevel
)


class EmotionalSignalDetector:
    """
    Detects emotional trading signals in user messages.
    
    Uses pattern matching and heuristics to identify:
    - Urgency signals (time pressure)
    - Overconfidence (certainty language)
    - FOMO (fear of missing out)
    - Revenge trading (recovering losses)
    - Missing risk parameters
    """
    
    # Signal patterns (Portuguese + English)
    PATTERNS = {
        SignalType.URGENCY: {
            "patterns": [
                r"\b(agora|urgente|já|imediato|rápido|depressa)\b",
                r"\b(now|urgent|immediately|quick|hurry|asap)\b",
                r"!{2,}",  # Multiple exclamation marks
            ],
            "risk_level": RiskLevel.HIGH,
            "explanation_pt": "Urgência detectada. Decisões apressadas geralmente resultam em perdas.",
            "explanation_en": "Urgency detected. Rushed decisions usually lead to losses.",
        },
        SignalType.OVERCONFIDENCE: {
            "patterns": [
                r"\b(certeza|garantido|impossível perder|vai subir|vai cair)\b",
                r"\b(certain|guaranteed|can't lose|will go up|will go down|definitely)\b",
                r"\b(100%|obvio|óbvio|claro que)\b",
            ],
            "risk_level": RiskLevel.HIGH,
            "explanation_pt": "Overconfidence detectada. Ninguém pode prever o mercado com certeza.",
            "explanation_en": "Overconfidence detected. No one can predict the market with certainty.",
        },
        SignalType.FOMO: {
            "patterns": [
                r"\b(não posso perder|última chance|todo mundo|está subindo)\b",
                r"\b(missing out|last chance|everyone is|moon|to the moon)\b",
                r"\b(fomo|foguete|rocket)\b",
            ],
            "risk_level": RiskLevel.HIGH,
            "explanation_pt": "FOMO detectado. Medo de ficar de fora leva a entradas ruins.",
            "explanation_en": "FOMO detected. Fear of missing out leads to poor entries.",
        },
        SignalType.REVENGE_TRADING: {
            "patterns": [
                r"\b(recuperar|compensar|perdi|prejuízo|voltar)\b",
                r"\b(recover|get back|lost|revenge|make up for)\b",
            ],
            "risk_level": RiskLevel.CRITICAL,
            "explanation_pt": "⚠️ Revenge trading detectado. Tentar recuperar perdas é a causa #1 de ruína.",
            "explanation_en": "⚠️ Revenge trading detected. Trying to recover losses is the #1 cause of ruin.",
        },
        SignalType.ALL_IN: {
            "patterns": [
                r"\b(all in|tudo|todo|100%|inteiro)\b",
                r"\b(yolo|maximum|max leverage|alavancagem máxima)\b",
            ],
            "risk_level": RiskLevel.CRITICAL,
            "explanation_pt": "⚠️ All-in detectado. Nunca arrisque mais do que pode perder.",
            "explanation_en": "⚠️ All-in detected. Never risk more than you can afford to lose.",
        },
        SignalType.EMOTIONAL_LANGUAGE: {
            "patterns": [
                r"\b(odeio|amo|raiva|frustrado|ansioso)\b",
                r"\b(hate|love|angry|frustrated|anxious|excited)\b",
                r"!{3,}",  # Many exclamation marks
                r"\b[A-Z]{4,}\b",  # ALL CAPS words
            ],
            "risk_level": RiskLevel.MEDIUM,
            "explanation_pt": "Linguagem emocional detectada. Trading deve ser racional, não emocional.",
            "explanation_en": "Emotional language detected. Trading should be rational, not emotional.",
        },
    }
    
    # Risk parameter keywords
    RISK_PARAMS = {
        "stop": [r"\bstop\b", r"\bstop.?loss\b", r"\bsl\b", r"\bparar\b"],
        "entry": [r"\bentry\b", r"\bentrada\b", r"\bpreço de entrada\b", r"\b\d+\.?\d*\s*(usd|usdc|btc)\b"],
        "target": [r"\btarget\b", r"\balvo\b", r"\btp\b", r"\btake.?profit\b"],
    }
    
    def detect(self, message: str) -> List[DetectedSignal]:
        """Detect emotional signals in message."""
        signals = []
        message_lower = message.lower()
        
        # Check pattern-based signals
        for signal_type, config in self.PATTERNS.items():
            for pattern in config["patterns"]:
                matches = re.findall(pattern, message_lower, re.IGNORECASE)
                if matches:
                    # Detect language
                    is_portuguese = self._is_portuguese(message)
                    explanation = config["explanation_pt"] if is_portuguese else config["explanation_en"]
                    
                    signals.append(DetectedSignal(
                        signal_type=signal_type,
                        risk_level=config["risk_level"],
                        confidence=min(0.9, 0.5 + 0.1 * len(matches)),
                        matched_text=", ".join(matches[:3]),
                        explanation=explanation,
                    ))
                    break  # One match per signal type
        
        # Check for missing risk parameters
        has_leverage = bool(re.search(r"\b(alavanca|leverage|margin|margem|\dx)\b", message_lower))
        has_stop = any(re.search(p, message_lower) for p in self.RISK_PARAMS["stop"])
        has_entry = any(re.search(p, message_lower) for p in self.RISK_PARAMS["entry"])
        has_target = any(re.search(p, message_lower) for p in self.RISK_PARAMS["target"])
        
        if has_leverage and not has_stop:
            signals.append(DetectedSignal(
                signal_type=SignalType.LEVERAGE_NO_STOP,
                risk_level=RiskLevel.CRITICAL,
                confidence=0.95,
                matched_text="leverage without stop-loss",
                explanation="⚠️ Alavancagem sem stop-loss é receita para liquidação." if self._is_portuguese(message) 
                           else "⚠️ Leverage without stop-loss is a recipe for liquidation.",
            ))
        
        if not has_stop:
            signals.append(DetectedSignal(
                signal_type=SignalType.MISSING_STOP_LOSS,
                risk_level=RiskLevel.MEDIUM,
                confidence=0.8,
                matched_text="no stop-loss mentioned",
                explanation="Stop-loss não mencionado. Defina seu risco antes de entrar." if self._is_portuguese(message)
                           else "No stop-loss mentioned. Define your risk before entering.",
            ))
        
        return signals
    
    def _is_portuguese(self, text: str) -> bool:
        """Simple language detection."""
        pt_words = ["que", "para", "com", "não", "uma", "por", "mais", "como", "mas", "ao"]
        text_lower = text.lower()
        pt_count = sum(1 for word in pt_words if f" {word} " in f" {text_lower} ")
        return pt_count >= 2
```

### 4.4 API Endpoints

```python
# apps/backend/monolith/api/views/intent_views.py

from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response
from rest_framework import status

from api.application.intent_guard import AnalyzeIntentUseCase


@api_view(['POST'])
@permission_classes([IsAuthenticated])
def analyze_intent(request):
    """
    Analyze a trading intent for emotional signals.
    
    Request Body:
        message (str): User's trading intent in natural language
        
    Response:
        {
            "risk_level": "HIGH",
            "signals": [...],
            "params": {...},
            "missing_params": ["stop_loss"],
            "response": "...",
            "can_proceed": false,
            "requires_acknowledgment": true
        }
    """
    message = request.data.get("message", "")
    
    if not message or len(message) < 5:
        return Response(
            {"error": "Message too short"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    if len(message) > 1000:
        return Response(
            {"error": "Message too long (max 1000 characters)"},
            status=status.HTTP_400_BAD_REQUEST
        )
    
    use_case = AnalyzeIntentUseCase()
    result = use_case.execute(
        client_id=request.user.client_id,
        message=message,
    )
    
    return Response({
        "risk_level": result.overall_risk_level.value,
        "signals": [
            {
                "type": s.signal_type.value,
                "risk_level": s.risk_level.value,
                "confidence": s.confidence,
                "matched_text": s.matched_text,
                "explanation": s.explanation,
            }
            for s in result.signals
        ],
        "params": {
            "symbol": result.params.symbol if result.params else None,
            "side": result.params.side if result.params else None,
            "entry_price": str(result.params.entry_price) if result.params and result.params.entry_price else None,
            "stop_price": str(result.params.stop_price) if result.params and result.params.stop_price else None,
            "target_price": str(result.params.target_price) if result.params and result.params.target_price else None,
        } if result.params else None,
        "missing_params": result.missing_params,
        "response": result.response_message,
        "educational_content": result.educational_content,
        "can_proceed": result.can_proceed,
        "requires_acknowledgment": result.requires_acknowledgment,
        "cooling_off_until": result.cooling_off_until.isoformat() if result.cooling_off_until else None,
    })
```

### 4.5 Frontend Components

```jsx
// apps/frontend/src/components/TradingIntentInput/TradingIntentInput.jsx

import React, { useState } from 'react';
import PropTypes from 'prop-types';
import axios from 'axios';
import './TradingIntentInput.css';

/**
 * Trading Intent Input Component
 * 
 * Allows users to describe their trading intentions in natural language.
 * Analyzes the message for emotional trading signals and provides feedback.
 */
const TradingIntentInput = ({ onAnalysisComplete }) => {
  const [message, setMessage] = useState('');
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState(null);
  const [error, setError] = useState(null);

  const handleSubmit = async (e) => {
    e.preventDefault();
    
    if (!message.trim() || message.length < 5) {
      setError('Please enter at least 5 characters');
      return;
    }

    setLoading(true);
    setError(null);
    setResult(null);

    try {
      const response = await axios.post('/api/intent/analyze/', {
        message: message.trim(),
      });
      
      setResult(response.data);
      
      if (onAnalysisComplete) {
        onAnalysisComplete(response.data);
      }
    } catch (err) {
      setError(err.response?.data?.error || 'Analysis failed');
    } finally {
      setLoading(false);
    }
  };

  const getRiskLevelColor = (level) => {
    const colors = {
      NONE: '#22c55e',    // green
      LOW: '#84cc16',     // lime
      MEDIUM: '#eab308',  // yellow
      HIGH: '#f97316',    // orange
      CRITICAL: '#ef4444', // red
    };
    return colors[level] || '#6b7280';
  };

  const getRiskLevelEmoji = (level) => {
    const emojis = {
      NONE: '✅',
      LOW: '💡',
      MEDIUM: '⚠️',
      HIGH: '🚨',
      CRITICAL: '🛑',
    };
    return emojis[level] || '❓';
  };

  return (
    <div className="trading-intent-container">
      <h3 className="intent-title">What trade are you considering?</h3>
      
      <form onSubmit={handleSubmit} className="intent-form">
        <textarea
          value={message}
          onChange={(e) => setMessage(e.target.value)}
          placeholder="Describe your trading idea... e.g., 'I want to buy BTC with entry at 95000, stop at 93000, target 100000'"
          className="intent-textarea"
          rows={4}
          maxLength={1000}
          disabled={loading}
        />
        
        <div className="intent-actions">
          <span className="char-count">{message.length}/1000</span>
          <button 
            type="submit" 
            className="analyze-button"
            disabled={loading || message.length < 5}
          >
            {loading ? 'Analyzing...' : '🔍 Analyze Intent'}
          </button>
        </div>
      </form>

      {error && (
        <div className="error-message">
          ❌ {error}
        </div>
      )}

      {result && (
        <div 
          className="analysis-result"
          style={{ borderColor: getRiskLevelColor(result.risk_level) }}
        >
          <div className="risk-header">
            <span className="risk-emoji">{getRiskLevelEmoji(result.risk_level)}</span>
            <span 
              className="risk-badge"
              style={{ backgroundColor: getRiskLevelColor(result.risk_level) }}
            >
              {result.risk_level}
            </span>
          </div>

          {result.signals.length > 0 && (
            <div className="signals-section">
              <h4>Detected Signals:</h4>
              <ul className="signals-list">
                {result.signals.map((signal, idx) => (
                  <li key={idx} className="signal-item">
                    <span 
                      className="signal-badge"
                      style={{ backgroundColor: getRiskLevelColor(signal.risk_level) }}
                    >
                      {signal.type}
                    </span>
                    <span className="signal-explanation">{signal.explanation}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {result.missing_params.length > 0 && (
            <div className="missing-params">
              <h4>Missing Parameters:</h4>
              <ul>
                {result.missing_params.map((param, idx) => (
                  <li key={idx}>❌ {param}</li>
                ))}
              </ul>
            </div>
          )}

          <div className="response-message">
            {result.response}
          </div>

          {result.educational_content && (
            <div className="educational-content">
              💡 {result.educational_content}
            </div>
          )}

          <div className="action-buttons">
            {result.can_proceed ? (
              <button className="proceed-button">
                ✅ Proceed to Position Sizing
              </button>
            ) : (
              <button className="reconsider-button" disabled>
                🛑 Trade Blocked - Reconsider
              </button>
            )}
          </div>

          {result.cooling_off_until && (
            <div className="cooling-off-notice">
              ⏰ Cooling-off period until: {new Date(result.cooling_off_until).toLocaleString()}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

TradingIntentInput.propTypes = {
  onAnalysisComplete: PropTypes.func,
};

export default TradingIntentInput;
```

---

## 5. Implementation Tasks

### Phase 1: Backend Foundation (Day 1-2)

| Task | Description | File |
|------|-------------|------|
| 1.1 | Create domain entities | `core/domain/intent_guard.py` |
| 1.2 | Implement signal detector | `core/application/signal_detector.py` |
| 1.3 | Implement param extractor | `core/application/param_extractor.py` |
| 1.4 | Create AnalyzeIntentUseCase | `core/application/intent_use_cases.py` |
| 1.5 | Add API endpoint | `api/views/intent_views.py` |
| 1.6 | Add URL routing | `api/urls.py` |
| 1.7 | Write unit tests | `api/tests/test_intent_guard.py` |

### Phase 2: Frontend Implementation (Day 2-3)

| Task | Description | File |
|------|-------------|------|
| 2.1 | Create TradingIntentInput component | `components/TradingIntentInput/` |
| 2.2 | Create component styles | `TradingIntentInput.css` |
| 2.3 | Integrate into Dashboard | `pages/Dashboard.jsx` |
| 2.4 | Add API service | `services/intentApi.js` |
| 2.5 | Write component tests | `tests/TradingIntentInput.test.jsx` |

### Phase 3: Enhanced Detection (Day 3-4)

| Task | Description |
|------|-------------|
| 3.1 | Add more signal patterns |
| 3.2 | Improve language detection |
| 3.3 | Add parameter extraction with NLP |
| 3.4 | Add educational content database |
| 3.5 | Implement cooling-off period logic |

### Phase 4: Integration & Polish (Day 4-5)

| Task | Description |
|------|-------------|
| 4.1 | Connect to position sizing flow |
| 4.2 | Add audit trail logging |
| 4.3 | Add analytics events |
| 4.4 | UI/UX polish |
| 4.5 | End-to-end testing |
| 4.6 | Documentation |

---

## 6. Test Strategy

### Unit Tests

```python
# Test signal detection
def test_detects_urgency():
    detector = EmotionalSignalDetector()
    signals = detector.detect("Preciso comprar BTC agora urgente!!!")
    assert any(s.signal_type == SignalType.URGENCY for s in signals)
    assert any(s.risk_level == RiskLevel.HIGH for s in signals)

def test_detects_revenge_trading():
    detector = EmotionalSignalDetector()
    signals = detector.detect("Perdi 500 USDC, preciso recuperar")
    assert any(s.signal_type == SignalType.REVENGE_TRADING for s in signals)
    assert any(s.risk_level == RiskLevel.CRITICAL for s in signals)

def test_detects_missing_stop():
    detector = EmotionalSignalDetector()
    signals = detector.detect("Quero comprar BTC alavancado")
    assert any(s.signal_type == SignalType.LEVERAGE_NO_STOP for s in signals)
```

### Integration Tests

```python
@pytest.mark.django_db
def test_analyze_intent_api(client, user):
    client.force_authenticate(user=user)
    
    response = client.post('/api/intent/analyze/', {
        "message": "Quero comprar BTC agora urgente com toda minha grana!!!"
    })
    
    assert response.status_code == 200
    assert response.data["risk_level"] == "CRITICAL"
    assert response.data["can_proceed"] == False
```

### Frontend Tests

```javascript
describe('TradingIntentInput', () => {
  it('shows warning for urgent messages', async () => {
    render(<TradingIntentInput />);
    
    fireEvent.change(screen.getByRole('textbox'), {
      target: { value: 'Buy BTC now urgently!' }
    });
    fireEvent.click(screen.getByText('Analyze Intent'));
    
    await waitFor(() => {
      expect(screen.getByText(/URGENCY/)).toBeInTheDocument();
    });
  });
});
```

---

## 7. Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Signal Detection Accuracy | > 90% | Manual review of 100 samples |
| False Positive Rate | < 10% | User feedback |
| Response Time | < 2s | API latency monitoring |
| User Engagement | > 50% use prompt | Analytics |
| Blocked Trades That Would Have Lost | Track | Compare blocked vs allowed |

---

## 8. Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Over-blocking valid trades | User frustration | Allow override with acknowledgment |
| False positives on normal messages | Poor UX | Tune patterns, add confidence thresholds |
| Performance impact | Slow response | Cache patterns, optimize regex |
| Bypass by savvy users | Reduced effectiveness | Log attempts, add variety to detection |

---

## 9. Future Enhancements

1. **LLM Integration**: Use DeepSeek for more sophisticated intent analysis
2. **Learning System**: Learn from user feedback to improve detection
3. **Personalization**: Adjust sensitivity based on user trading history
4. **Voice Input**: Allow voice-to-text for intent input
5. **Mobile Optimization**: Touch-friendly interface

---

## 10. References

- ADR-0007: Robson is Risk Assistant
- CLAUDE.md: Core Philosophy
- docs/requirements/STRATEGY-SEMANTIC-CLARITY.md

---

**Approved By**: _________________  
**Date**: _________________  
**Version**: 1.0


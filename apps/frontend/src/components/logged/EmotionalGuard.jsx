import React, { useState, useContext } from 'react';
import { Card, Form, Button, Alert, Badge, Spinner, ProgressBar } from 'react-bootstrap';
import AuthContext from '../../context/AuthContext';
import { EmotionalGuardHttp } from '../../adapters/http/EmotionalGuardHttp';

/**
 * Emotional Trading Guard Component
 * 
 * A textarea prompt where users can describe their trading intentions.
 * The system analyzes the message for emotional patterns and provides
 * educational feedback to prevent impulsive decisions.
 */
function EmotionalGuard() {
  const { authTokens } = useContext(AuthContext);
  const [message, setMessage] = useState('');
  const [analysis, setAnalysis] = useState(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);
  const [showTip, setShowTip] = useState(null);

  const guardService = new EmotionalGuardHttp({
    baseUrl: import.meta.env.VITE_BACKEND_URL || '',
    getAuthToken: () => authTokens?.access,
  });

  const analyzeIntent = async () => {
    if (!message.trim()) return;
    
    setLoading(true);
    setError(null);
    setAnalysis(null);

    try {
      const result = await guardService.analyzeIntent(message);
      setAnalysis(result);
    } catch (err) {
      setError(err.message);
    } finally {
      setLoading(false);
    }
  };

  const fetchRandomTip = async () => {
    try {
      const result = await guardService.getTips(true);
      setShowTip(result.tip);
    } catch (err) {
      console.error('Failed to fetch tip:', err);
    }
  };

  const getRiskLevelVariant = (level) => {
    switch (level) {
      case 'low': return 'success';
      case 'medium': return 'warning';
      case 'high': return 'danger';
      case 'critical': return 'dark';
      default: return 'secondary';
    }
  };

  const getRiskLevelIcon = (level) => {
    switch (level) {
      case 'low': return '✅';
      case 'medium': return '⚠️';
      case 'high': return '🚨';
      case 'critical': return '🛑';
      default: return '❓';
    }
  };

  return (
    <Card className="mb-4" style={{ 
      background: 'linear-gradient(135deg, #1a1a2e 0%, #16213e 100%)',
      border: '1px solid #0f3460',
      borderRadius: '16px',
    }}>
      <Card.Header style={{ 
        background: 'transparent',
        borderBottom: '1px solid #0f3460',
        padding: '1.25rem',
      }}>
        <div className="d-flex align-items-center justify-content-between">
          <div className="d-flex align-items-center gap-2">
            <span style={{ fontSize: '1.5rem' }}>🛡️</span>
            <div>
              <h5 className="mb-0" style={{ color: '#e94560', fontWeight: 600 }}>
                Emotional Trading Guard
              </h5>
              <small style={{ color: '#94a3b8' }}>
                Describe your trade idea – I'll help you check for emotional biases
              </small>
            </div>
          </div>
          <Button 
            variant="outline-info" 
            size="sm"
            onClick={fetchRandomTip}
            style={{ borderColor: '#0f3460' }}
          >
            💡 Random Tip
          </Button>
        </div>
      </Card.Header>

      <Card.Body style={{ padding: '1.5rem' }}>
        {/* Random Tip Display */}
        {showTip && (
          <Alert 
            variant="info" 
            dismissible 
            onClose={() => setShowTip(null)}
            style={{ 
              background: 'rgba(56, 189, 248, 0.1)',
              border: '1px solid rgba(56, 189, 248, 0.3)',
              color: '#e2e8f0',
            }}
          >
            <Alert.Heading style={{ fontSize: '0.9rem', color: '#38bdf8' }}>
              💡 {showTip.category}
            </Alert.Heading>
            <p className="mb-1" style={{ fontWeight: 500 }}>{showTip.tip}</p>
            <small style={{ color: '#94a3b8' }}>{showTip.explanation}</small>
          </Alert>
        )}

        {/* Input Form */}
        <Form.Group className="mb-3">
          <Form.Control
            as="textarea"
            rows={4}
            placeholder="Example: I want to buy BTC now! It's going to explode! Entry at 100k with 10x leverage..."
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            style={{
              background: '#0f172a',
              border: '1px solid #334155',
              color: '#e2e8f0',
              borderRadius: '12px',
              padding: '1rem',
              fontSize: '0.95rem',
              resize: 'none',
            }}
          />
          <Form.Text style={{ color: '#64748b' }}>
            Be honest about your intentions. The more detail, the better the analysis.
          </Form.Text>
        </Form.Group>

        <Button
          onClick={analyzeIntent}
          disabled={loading || !message.trim()}
          style={{
            background: 'linear-gradient(135deg, #e94560 0%, #c73659 100%)',
            border: 'none',
            borderRadius: '8px',
            padding: '0.75rem 2rem',
            fontWeight: 600,
          }}
        >
          {loading ? (
            <>
              <Spinner size="sm" animation="border" className="me-2" />
              Analyzing...
            </>
          ) : (
            <>🔍 Analyze My Intent</>
          )}
        </Button>

        {/* Error Display */}
        {error && (
          <Alert variant="danger" className="mt-3" style={{
            background: 'rgba(239, 68, 68, 0.1)',
            border: '1px solid rgba(239, 68, 68, 0.3)',
            color: '#fca5a5',
          }}>
            {error}
          </Alert>
        )}

        {/* Analysis Results */}
        {analysis && (
          <div className="mt-4">
            {/* Risk Level Banner */}
            <Alert 
              variant={getRiskLevelVariant(analysis.risk_level)}
              style={{
                borderRadius: '12px',
                padding: '1.25rem',
              }}
            >
              <div className="d-flex align-items-center gap-3">
                <span style={{ fontSize: '2rem' }}>
                  {getRiskLevelIcon(analysis.risk_level)}
                </span>
                <div className="flex-grow-1">
                  <h5 className="mb-1">
                    Risk Level: {analysis.risk_level.toUpperCase()}
                  </h5>
                  <ProgressBar 
                    now={Math.min(analysis.risk_score, 100)} 
                    variant={getRiskLevelVariant(analysis.risk_level)}
                    style={{ height: '8px', borderRadius: '4px' }}
                  />
                  <small>Score: {analysis.risk_score.toFixed(1)} / 100</small>
                </div>
                <div className="text-end">
                  <Badge bg={analysis.proceed_allowed ? 'success' : 'danger'}>
                    {analysis.proceed_allowed ? 'May Proceed' : 'Review Required'}
                  </Badge>
                </div>
              </div>
            </Alert>

            {/* Response Message */}
            <Card style={{
              background: '#0f172a',
              border: '1px solid #334155',
              borderRadius: '12px',
              marginBottom: '1rem',
            }}>
              <Card.Body>
                <div style={{ 
                  color: '#e2e8f0', 
                  whiteSpace: 'pre-line',
                  lineHeight: '1.6',
                }}>
                  {analysis.response_message}
                </div>
              </Card.Body>
            </Card>

            {/* Detected Signals */}
            {analysis.signals && analysis.signals.length > 0 && (
              <Card style={{
                background: '#0f172a',
                border: '1px solid #334155',
                borderRadius: '12px',
              }}>
                <Card.Header style={{ 
                  background: 'transparent',
                  borderBottom: '1px solid #334155',
                  color: '#e2e8f0',
                }}>
                  <strong>Detected Signals</strong>
                  <span className="ms-2">
                    <Badge bg="danger" className="me-1">{analysis.warning_count} warnings</Badge>
                    <Badge bg="success">{analysis.positive_count} positive</Badge>
                  </span>
                </Card.Header>
                <Card.Body style={{ maxHeight: '300px', overflowY: 'auto' }}>
                  {analysis.signals.map((signal, idx) => (
                    <div 
                      key={idx}
                      className="d-flex align-items-start gap-2 mb-2 p-2"
                      style={{
                        background: signal.is_positive 
                          ? 'rgba(34, 197, 94, 0.1)' 
                          : 'rgba(239, 68, 68, 0.1)',
                        borderRadius: '8px',
                        border: `1px solid ${signal.is_positive 
                          ? 'rgba(34, 197, 94, 0.3)' 
                          : 'rgba(239, 68, 68, 0.3)'}`,
                      }}
                    >
                      <span>{signal.is_positive ? '✅' : '⚠️'}</span>
                      <div>
                        <Badge 
                          bg={signal.is_positive ? 'success' : 'danger'}
                          className="me-2"
                        >
                          {signal.type.replace('_', ' ')}
                        </Badge>
                        <small style={{ color: '#94a3b8' }}>
                          matched: "{signal.matched_phrase}"
                        </small>
                        <p className="mb-0 mt-1" style={{ 
                          color: '#e2e8f0', 
                          fontSize: '0.9rem' 
                        }}>
                          {signal.explanation}
                        </p>
                      </div>
                    </div>
                  ))}
                </Card.Body>
              </Card>
            )}

            {/* Extracted Parameters */}
            {analysis.parameters && analysis.parameters.symbol && (
              <Card className="mt-3" style={{
                background: '#0f172a',
                border: '1px solid #334155',
                borderRadius: '12px',
              }}>
                <Card.Header style={{ 
                  background: 'transparent',
                  borderBottom: '1px solid #334155',
                  color: '#e2e8f0',
                }}>
                  <strong>📊 Extracted Parameters</strong>
                </Card.Header>
                <Card.Body>
                  <div className="d-flex flex-wrap gap-3">
                    {analysis.parameters.symbol && (
                      <div>
                        <small style={{ color: '#64748b' }}>Symbol</small>
                        <div style={{ color: '#e2e8f0', fontWeight: 600 }}>
                          {analysis.parameters.symbol}
                        </div>
                      </div>
                    )}
                    {analysis.parameters.side && (
                      <div>
                        <small style={{ color: '#64748b' }}>Side</small>
                        <div>
                          <Badge bg={analysis.parameters.side === 'BUY' ? 'success' : 'danger'}>
                            {analysis.parameters.side}
                          </Badge>
                        </div>
                      </div>
                    )}
                    {analysis.parameters.entry_price && (
                      <div>
                        <small style={{ color: '#64748b' }}>Entry</small>
                        <div style={{ color: '#e2e8f0', fontWeight: 600 }}>
                          ${analysis.parameters.entry_price}
                        </div>
                      </div>
                    )}
                    {analysis.parameters.stop_price && (
                      <div>
                        <small style={{ color: '#64748b' }}>Stop</small>
                        <div style={{ color: '#ef4444', fontWeight: 600 }}>
                          ${analysis.parameters.stop_price}
                        </div>
                      </div>
                    )}
                    {analysis.parameters.leverage && (
                      <div>
                        <small style={{ color: '#64748b' }}>Leverage</small>
                        <div style={{ color: '#f59e0b', fontWeight: 600 }}>
                          {analysis.parameters.leverage}x
                        </div>
                      </div>
                    )}
                    <div>
                      <small style={{ color: '#64748b' }}>Has Stop-Loss?</small>
                      <div>
                        <Badge bg={analysis.parameters.has_risk_parameters ? 'success' : 'danger'}>
                          {analysis.parameters.has_risk_parameters ? 'Yes ✓' : 'No ✗'}
                        </Badge>
                      </div>
                    </div>
                  </div>
                </Card.Body>
              </Card>
            )}

            {/* Educational Content */}
            {analysis.educational_content && (
              <Card className="mt-3" style={{
                background: 'rgba(56, 189, 248, 0.05)',
                border: '1px solid rgba(56, 189, 248, 0.2)',
                borderRadius: '12px',
              }}>
                <Card.Header style={{ 
                  background: 'transparent',
                  borderBottom: '1px solid rgba(56, 189, 248, 0.2)',
                  color: '#38bdf8',
                }}>
                  <strong>📚 Learn More</strong>
                </Card.Header>
                <Card.Body>
                  <div style={{ 
                    color: '#e2e8f0', 
                    whiteSpace: 'pre-line',
                    lineHeight: '1.6',
                  }}>
                    {analysis.educational_content}
                  </div>
                </Card.Body>
              </Card>
            )}
          </div>
        )}
      </Card.Body>
    </Card>
  );
}

export default EmotionalGuard;


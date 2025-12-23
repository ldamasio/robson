import React, { useState, useContext } from 'react';
import { Card, Form, Button, Row, Col, Alert, Badge, InputGroup } from 'react-bootstrap';
import AuthContext from '../../context/AuthContext';
import { MarginHttp } from '../../adapters/http/MarginHttp';

/**
 * Margin Position Size Calculator
 * 
 * Calculates the optimal position size for a margin trade
 * using the 1% risk rule.
 */
function MarginPositionCalculator() {
  const { authTokens } = useContext(AuthContext);
  const [formData, setFormData] = useState({
    symbol: 'BTCUSDC',
    side: 'LONG',
    entryPrice: '',
    stopPrice: '',
    capital: '',
    leverage: '3',
    riskPercent: '1.0',
  });
  const [result, setResult] = useState(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);

  const marginService = new MarginHttp({
    baseUrl: import.meta.env.VITE_BACKEND_URL || '',
    getAuthToken: () => authTokens?.access,
  });

  const handleChange = (e) => {
    const { name, value } = e.target;
    setFormData(prev => ({ ...prev, [name]: value }));
  };

  const calculatePosition = async (e) => {
    e.preventDefault();
    setLoading(true);
    setError(null);
    setResult(null);

    try {
      const data = await marginService.calculatePositionSize({
        symbol: formData.symbol,
        side: formData.side,
        entry_price: formData.entryPrice,
        stop_price: formData.stopPrice,
        capital: formData.capital,
        leverage: parseInt(formData.leverage),
        risk_percent: formData.riskPercent,
      });
      setResult(data);
    } catch (err) {
      setError(err.message);
    } finally {
      setLoading(false);
    }
  };

  return (
    <Card style={{
      background: 'linear-gradient(135deg, #0f172a 0%, #1e293b 100%)',
      border: '1px solid #334155',
      borderRadius: '16px',
    }}>
      <Card.Header style={{
        background: 'transparent',
        borderBottom: '1px solid #334155',
        padding: '1.25rem',
      }}>
        <div className="d-flex align-items-center gap-2">
          <span style={{ fontSize: '1.5rem' }}>🧮</span>
          <div>
            <h5 className="mb-0" style={{ color: '#22c55e', fontWeight: 600 }}>
              Position Size Calculator
            </h5>
            <small style={{ color: '#94a3b8' }}>
              Calculate safe position size with the 1% risk rule
            </small>
          </div>
        </div>
      </Card.Header>

      <Card.Body style={{ padding: '1.5rem' }}>
        <Form onSubmit={calculatePosition}>
          <Row className="mb-3">
            <Col md={6}>
              <Form.Group>
                <Form.Label style={{ color: '#94a3b8' }}>Symbol</Form.Label>
                <Form.Select
                  name="symbol"
                  value={formData.symbol}
                  onChange={handleChange}
                  style={{
                    background: '#0f172a',
                    border: '1px solid #334155',
                    color: '#e2e8f0',
                    borderRadius: '8px',
                  }}
                >
                  <option value="BTCUSDC">BTC/USDC</option>
                  <option value="ETHUSDC">ETH/USDC</option>
                  <option value="SOLUSDC">SOL/USDC</option>
                </Form.Select>
              </Form.Group>
            </Col>
            <Col md={6}>
              <Form.Group>
                <Form.Label style={{ color: '#94a3b8' }}>Side</Form.Label>
                <div className="d-flex gap-2">
                  <Button
                    variant={formData.side === 'LONG' ? 'success' : 'outline-success'}
                    onClick={() => setFormData(prev => ({ ...prev, side: 'LONG' }))}
                    style={{ flex: 1, borderRadius: '8px' }}
                  >
                    📈 LONG
                  </Button>
                  <Button
                    variant={formData.side === 'SHORT' ? 'danger' : 'outline-danger'}
                    onClick={() => setFormData(prev => ({ ...prev, side: 'SHORT' }))}
                    style={{ flex: 1, borderRadius: '8px' }}
                  >
                    📉 SHORT
                  </Button>
                </div>
              </Form.Group>
            </Col>
          </Row>

          <Row className="mb-3">
            <Col md={6}>
              <Form.Group>
                <Form.Label style={{ color: '#94a3b8' }}>Entry Price</Form.Label>
                <InputGroup>
                  <InputGroup.Text style={{ 
                    background: '#1e293b', 
                    border: '1px solid #334155',
                    color: '#64748b',
                  }}>$</InputGroup.Text>
                  <Form.Control
                    type="number"
                    name="entryPrice"
                    value={formData.entryPrice}
                    onChange={handleChange}
                    placeholder="100000"
                    step="0.01"
                    required
                    style={{
                      background: '#0f172a',
                      border: '1px solid #334155',
                      color: '#e2e8f0',
                      borderRadius: '0 8px 8px 0',
                    }}
                  />
                </InputGroup>
              </Form.Group>
            </Col>
            <Col md={6}>
              <Form.Group>
                <Form.Label style={{ color: '#94a3b8' }}>Stop-Loss Price</Form.Label>
                <InputGroup>
                  <InputGroup.Text style={{ 
                    background: '#1e293b', 
                    border: '1px solid #334155',
                    color: '#64748b',
                  }}>$</InputGroup.Text>
                  <Form.Control
                    type="number"
                    name="stopPrice"
                    value={formData.stopPrice}
                    onChange={handleChange}
                    placeholder="98000"
                    step="0.01"
                    required
                    style={{
                      background: '#0f172a',
                      border: '1px solid #334155',
                      color: '#ef4444',
                      borderRadius: '0 8px 8px 0',
                    }}
                  />
                </InputGroup>
              </Form.Group>
            </Col>
          </Row>

          <Row className="mb-3">
            <Col md={4}>
              <Form.Group>
                <Form.Label style={{ color: '#94a3b8' }}>Total Capital</Form.Label>
                <InputGroup>
                  <InputGroup.Text style={{ 
                    background: '#1e293b', 
                    border: '1px solid #334155',
                    color: '#64748b',
                  }}>$</InputGroup.Text>
                  <Form.Control
                    type="number"
                    name="capital"
                    value={formData.capital}
                    onChange={handleChange}
                    placeholder="10000"
                    step="0.01"
                    required
                    style={{
                      background: '#0f172a',
                      border: '1px solid #334155',
                      color: '#e2e8f0',
                      borderRadius: '0 8px 8px 0',
                    }}
                  />
                </InputGroup>
              </Form.Group>
            </Col>
            <Col md={4}>
              <Form.Group>
                <Form.Label style={{ color: '#94a3b8' }}>Leverage</Form.Label>
                <Form.Select
                  name="leverage"
                  value={formData.leverage}
                  onChange={handleChange}
                  style={{
                    background: '#0f172a',
                    border: '1px solid #334155',
                    color: '#f59e0b',
                    borderRadius: '8px',
                  }}
                >
                  <option value="1">1x (No Leverage)</option>
                  <option value="2">2x</option>
                  <option value="3">3x (Recommended)</option>
                  <option value="5">5x</option>
                  <option value="10">10x (Risky)</option>
                </Form.Select>
              </Form.Group>
            </Col>
            <Col md={4}>
              <Form.Group>
                <Form.Label style={{ color: '#94a3b8' }}>Risk %</Form.Label>
                <Form.Select
                  name="riskPercent"
                  value={formData.riskPercent}
                  onChange={handleChange}
                  style={{
                    background: '#0f172a',
                    border: '1px solid #334155',
                    color: '#22c55e',
                    borderRadius: '8px',
                  }}
                >
                  <option value="0.5">0.5% (Conservative)</option>
                  <option value="1.0">1.0% (Recommended)</option>
                  <option value="2.0">2.0% (Aggressive)</option>
                </Form.Select>
              </Form.Group>
            </Col>
          </Row>

          <Button
            type="submit"
            disabled={loading}
            style={{
              background: 'linear-gradient(135deg, #22c55e 0%, #16a34a 100%)',
              border: 'none',
              borderRadius: '8px',
              padding: '0.75rem 2rem',
              fontWeight: 600,
            }}
          >
            {loading ? 'Calculating...' : '🧮 Calculate Position Size'}
          </Button>
        </Form>

        {error && (
          <Alert variant="danger" className="mt-3" style={{
            background: 'rgba(239, 68, 68, 0.1)',
            border: '1px solid rgba(239, 68, 68, 0.3)',
            color: '#fca5a5',
            borderRadius: '12px',
          }}>
            {error}
          </Alert>
        )}

        {result && (
          <Card className="mt-4" style={{
            background: 'rgba(34, 197, 94, 0.05)',
            border: '1px solid rgba(34, 197, 94, 0.2)',
            borderRadius: '12px',
          }}>
            <Card.Header style={{
              background: 'transparent',
              borderBottom: '1px solid rgba(34, 197, 94, 0.2)',
              color: '#22c55e',
            }}>
              <strong>📊 Calculated Position</strong>
              {result.is_capped && (
                <Badge bg="warning" className="ms-2">Capped: {result.cap_reason}</Badge>
              )}
            </Card.Header>
            <Card.Body>
              <Row>
                <Col md={4} className="mb-3">
                  <div style={{ color: '#64748b' }}>Position Size</div>
                  <div style={{ 
                    color: '#e2e8f0', 
                    fontSize: '1.5rem', 
                    fontWeight: 700,
                    fontFamily: 'monospace',
                  }}>
                    {parseFloat(result.quantity).toFixed(8)}
                  </div>
                  <small style={{ color: '#94a3b8' }}>
                    {result.symbol?.replace('USDC', '')}
                  </small>
                </Col>
                <Col md={4} className="mb-3">
                  <div style={{ color: '#64748b' }}>Position Value</div>
                  <div style={{ 
                    color: '#38bdf8', 
                    fontSize: '1.5rem', 
                    fontWeight: 700,
                    fontFamily: 'monospace',
                  }}>
                    ${parseFloat(result.position_value).toFixed(2)}
                  </div>
                </Col>
                <Col md={4} className="mb-3">
                  <div style={{ color: '#64748b' }}>Margin Required</div>
                  <div style={{ 
                    color: '#f59e0b', 
                    fontSize: '1.5rem', 
                    fontWeight: 700,
                    fontFamily: 'monospace',
                  }}>
                    ${parseFloat(result.margin_required).toFixed(2)}
                  </div>
                </Col>
              </Row>
              <hr style={{ borderColor: '#334155' }} />
              <Row>
                <Col md={3}>
                  <div style={{ color: '#64748b' }}>Risk Amount</div>
                  <div style={{ color: '#ef4444', fontWeight: 600 }}>
                    ${parseFloat(result.risk_amount).toFixed(2)}
                  </div>
                </Col>
                <Col md={3}>
                  <div style={{ color: '#64748b' }}>Risk %</div>
                  <div style={{ color: '#ef4444', fontWeight: 600 }}>
                    {parseFloat(result.risk_percent).toFixed(2)}%
                  </div>
                </Col>
                <Col md={3}>
                  <div style={{ color: '#64748b' }}>Stop Distance</div>
                  <div style={{ color: '#94a3b8', fontWeight: 600 }}>
                    ${parseFloat(result.stop_distance).toFixed(2)}
                  </div>
                </Col>
                <Col md={3}>
                  <div style={{ color: '#64748b' }}>Stop Distance %</div>
                  <div style={{ color: '#94a3b8', fontWeight: 600 }}>
                    {parseFloat(result.stop_distance_percent).toFixed(2)}%
                  </div>
                </Col>
              </Row>
            </Card.Body>
          </Card>
        )}
      </Card.Body>
    </Card>
  );
}

export default MarginPositionCalculator;


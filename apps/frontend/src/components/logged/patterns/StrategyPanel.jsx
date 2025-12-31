/**
 * StrategyPanel Component
 *
 * Active strategy panel with scan and to-plan actions.
 * Allows running scans and managing pattern-to-plan workflow.
 */

import React, { useState, useContext } from 'react';
import { Card, Row, Col, Form, Button, Alert, Badge, Spinner, Table, Accordion } from 'react-bootstrap';
import { toast } from 'react-toastify';
import AuthContext from '../../../context/AuthContext';

const StrategyPanel = ({ configs, selectedStrategy, onSelectStrategy, onScan, onPatternToPlan, onRefresh }) => {
  const { authTokens } = useContext(AuthContext);

  // Scan state
  const [scanOptions, setScanOptions] = useState({
    symbols: 'BTCUSDT,ETHUSDT',
    timeframes: '15m,1h',
    allDetectors: true,
  });
  const [scanning, setScanning] = useState(false);
  const [scanResults, setScanResults] = useState(null);

  // Pattern instances for selected strategy
  const [patternInstances, setPatternInstances] = useState([]);
  const [loadingInstances, setLoadingInstances] = useState(false);

  // Get unique strategies from configs
  const strategies = React.useMemo(() => {
    const strategyMap = new Map();
    configs.forEach((config) => {
      if (!strategyMap.has(config.strategy)) {
        strategyMap.set(config.strategy, {
          id: config.strategy,
          name: config.strategy_name,
          patterns: [],
        });
      }
      strategyMap.get(config.strategy).patterns.push({
        id: config.id,
        pattern_name: config.pattern_name,
        pattern_code: config.pattern_code,
        pattern_id: config.pattern,
        is_active: config.is_active,
        auto_entry_enabled: config.auto_entry_enabled,
        symbols: config.symbols,
        timeframes: config.timeframes,
      });
    });
    return Array.from(strategyMap.values());
  }, [configs]);

  // Selected strategy data
  const currentStrategy = strategies.find((s) => s.id === selectedStrategy);
  const activePatterns = currentStrategy?.patterns.filter((p) => p.is_active) || [];

  // Handle scan
  const handleScan = async () => {
    setScanning(true);
    setScanResults(null);
    try {
      const result = await onScan(scanOptions);
      setScanResults(result);
      toast.success(`Scan complete: ${result.summary?.total_patterns || 0} patterns detected`);
      // Refresh pattern instances
      if (selectedStrategy) {
        await fetchPatternInstances();
      }
    } catch (err) {
      console.error('Scan failed:', err);
    } finally {
      setScanning(false);
    }
  };

  // Fetch pattern instances for selected strategy
  const fetchPatternInstances = async () => {
    if (!selectedStrategy) return;

    setLoadingInstances(true);
    try {
      const response = await fetch(`${import.meta.env.VITE_API_BASE_URL}/api/patterns/instances/?limit=20`, {
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${authTokens.access}`,
        },
      });
      if (response.ok) {
        const data = await response.json();
        setPatternInstances(data.results || []);
      }
    } catch (err) {
      console.error('Failed to fetch pattern instances:', err);
    } finally {
      setLoadingInstances(false);
    }
  };

  // Load instances when strategy changes
  React.useEffect(() => {
    if (selectedStrategy) {
      fetchPatternInstances();
    } else {
      setPatternInstances([]);
    }
  }, [selectedStrategy]);

  // Format timestamp
  const formatTime = (timestamp) => {
    if (!timestamp) return '-';
    const date = new Date(timestamp);
    return date.toLocaleString();
  };

  // Get status badge color
  const getStatusBadge = (status) => {
    const colors = {
      CONFIRMED: 'success',
      FORMING: 'warning',
      INVALIDATED: 'danger',
      FAILED: 'danger',
    };
    return colors[status] || 'secondary';
  };

  return (
    <div>
      {/* Header */}
      <div className="d-flex justify-content-between align-items-center mb-4">
        <div>
          <h4>Active Strategy Panel</h4>
          <p className="text-muted mb-0">
            Run scans and manage pattern-to-plan workflow
          </p>
        </div>
        <Button variant="outline-primary" size="sm" onClick={onRefresh}>
          🔄 Refresh
        </Button>
      </div>

      {/* Important Notice */}
      <Alert variant="warning" className="mb-4">
        ⚠️ <strong>Important:</strong> This does NOT place orders directly.
        Patterns are fed into Robson's execution pipeline (PLAN → VALIDATE → EXECUTE).
        You always have the final say before any order is placed.
      </Alert>

      <Row>
        {/* Strategy Selection */}
        <Col lg={4} className="mb-4">
          <Card>
            <Card.Header>
              <strong>1. Select Strategy</strong>
            </Card.Header>
            <Card.Body>
              {strategies.length === 0 ? (
                <p className="text-muted mb-0">
                  No strategies configured. Go to <strong>Strategy Configuration</strong> to create one.
                </p>
              ) : (
                <Form.Select
                  value={selectedStrategy || ''}
                  onChange={(e) => onSelectStrategy(parseInt(e.target.value) || null)}
                >
                  <option value="">Select a strategy...</option>
                  {strategies.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name}
                    </option>
                  ))}
                </Form.Select>
              )}
            </Card.Body>
          </Card>

          {/* Strategy Configs Summary */}
          {currentStrategy && (
            <Card className="mt-3">
              <Card.Header>
                <strong>Active Patterns for {currentStrategy.name}</strong>
              </Card.Header>
              <Card.Body className="p-0">
                <div style={{ maxHeight: '200px', overflowY: 'auto' }}>
                  <Table size="sm" className="mb-0">
                    <tbody>
                      {activePatterns.map((p) => (
                        <tr key={p.id}>
                          <td>
                            <div>{p.pattern_name}</div>
                            <small className="text-muted">{p.pattern_code}</small>
                          </td>
                          <td className="text-end">
                            {p.auto_entry_enabled ? (
                              <Badge bg="success" size="sm">Auto</Badge>
                            ) : (
                              <Badge bg="warning" size="sm">Manual</Badge>
                            )}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </Table>
                </div>
              </Card.Body>
            </Card>
          )}
        </Col>

        {/* Scan Controls */}
        <Col lg={8} className="mb-4">
          <Card>
            <Card.Header>
              <strong>2. Run Pattern Scan</strong>
            </Card.Header>
            <Card.Body>
              <Row>
                <Col md={6} className="mb-3">
                  <Form.Group>
                    <Form.Label>Symbols</Form.Label>
                    <Form.Control
                      type="text"
                      placeholder="BTCUSDT,ETHUSDT"
                      value={scanOptions.symbols}
                      onChange={(e) => setScanOptions({ ...scanOptions, symbols: e.target.value })}
                    />
                    <Form.Text className="text-muted">Comma-separated symbol list</Form.Text>
                  </Form.Group>
                </Col>
                <Col md={6} className="mb-3">
                  <Form.Group>
                    <Form.Label>Timeframes</Form.Label>
                    <Form.Control
                      type="text"
                      placeholder="15m,1h"
                      value={scanOptions.timeframes}
                      onChange={(e) => setScanOptions({ ...scanOptions, timeframes: e.target.value })}
                    />
                    <Form.Text className="text-muted">Comma-separated timeframes</Form.Text>
                  </Form.Group>
                </Col>
              </Row>
              <Form.Check
                type="checkbox"
                label="Use all pattern detectors"
                checked={scanOptions.allDetectors}
                onChange={(e) => setScanOptions({ ...scanOptions, allDetectors: e.target.checked })}
                className="mb-3"
              />
              <Button
                variant="primary"
                onClick={handleScan}
                disabled={scanning || !selectedStrategy}
              >
                {scanning ? (
                  <>
                    <Spinner animation="border" size="sm" className="me-2" />
                    Scanning...
                  </>
                ) : (
                  <>
                    🔍 Run Scan
                  </>
                )}
              </Button>
            </Card.Body>

            {/* Scan Results */}
            {scanResults && (
              <Card.Footer className="bg-dark">
                <h6 className="text-light mb-2">Scan Results</h6>
                <Row>
                  <Col>
                    <div className="text-light small">Patterns Detected</div>
                    <div className="h4 mb-0 text-success">{scanResults.summary?.total_patterns || 0}</div>
                  </Col>
                  <Col>
                    <div className="text-light small">Confirmations</div>
                    <div className="h4 mb-0 text-info">{scanResults.summary?.total_confirmations || 0}</div>
                  </Col>
                  <Col>
                    <div className="text-light small">Invalidations</div>
                    <div className="h4 mb-0 text-warning">{scanResults.summary?.total_invalidations || 0}</div>
                  </Col>
                </Row>
              </Card.Footer>
            )}
          </Card>
        </Col>
      </Row>

      {/* Confirmed Patterns */}
      {selectedStrategy && (
        <Card>
          <Card.Header>
            <strong>3. Confirmed Patterns - Send to Plan</strong>
          </Card.Header>
          <Card.Body className="p-0">
            {loadingInstances ? (
              <div className="text-center py-4">
                <Spinner animation="border" />
                <p className="text-muted mt-2">Loading pattern instances...</p>
              </div>
            ) : patternInstances.length === 0 ? (
              <div className="text-center py-4 text-muted">
                <div style={{ fontSize: '2rem' }}>🔍</div>
                <p>No confirmed patterns found.</p>
                <p className="small">Run a scan to detect new patterns.</p>
              </div>
            ) : (
              <Table hover responsive className="mb-0">
                <thead>
                  <tr>
                    <th>Pattern</th>
                    <th>Symbol / TF</th>
                    <th>Status</th>
                    <th>Confidence</th>
                    <th>Detected At</th>
                    <th>Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {patternInstances.map((instance) => (
                    <tr key={instance.id}>
                      <td>
                        <strong>{instance.pattern_name}</strong>
                        <br />
                        <small className="text-muted">{instance.pattern_code}</small>
                      </td>
                      <td>
                        <Badge bg="secondary">{instance.symbol}</Badge>
                        <span className="mx-1">|</span>
                        <Badge bg="info">{instance.timeframe}</Badge>
                      </td>
                      <td>
                        <Badge bg={getStatusBadge(instance.status)}>{instance.status}</Badge>
                      </td>
                      <td>
                        {instance.confidence ? (
                          <Badge bg={instance.confidence >= 0.75 ? 'success' : 'warning'}>
                            {(instance.confidence * 100).toFixed(0)}%
                          </Badge>
                        ) : (
                          '-'
                        )}
                      </td>
                      <td>
                        <small>{formatTime(instance.detected_at)}</small>
                      </td>
                      <td>
                        {instance.status === 'CONFIRMED' && (
                          <Button
                            size="sm"
                            variant="success"
                            onClick={() => {
                              onPatternToPlan(instance.id);
                              toast.success('Pattern sent to execution pipeline');
                            }}
                          >
                            ▶️ To Plan
                          </Button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </Table>
            )}
          </Card.Body>
        </Card>
      )}
    </div>
  );
};

export default StrategyPanel;

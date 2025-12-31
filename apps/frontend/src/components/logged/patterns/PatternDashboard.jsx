/**
 * PatternDashboard Component
 *
 * Displays pattern detection dashboard summary with statistics and recent alerts.
 */

import React from 'react';
import { Row, Col, Card, Button, Table, Badge, Alert } from 'react-bootstrap';

const PatternDashboard = ({ data, recentAlerts, onRefresh }) => {
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

  // Get direction badge color
  const getDirectionBadge = (direction) => {
    const colors = {
      BULLISH: 'success',
      BEARISH: 'danger',
      NEUTRAL: 'secondary',
    };
    return colors[direction] || 'secondary';
  };

  return (
    <div>
      {/* Header with refresh */}
      <div className="d-flex justify-content-between align-items-center mb-4">
        <h4>Pattern Detection Overview</h4>
        <Button variant="outline-primary" size="sm" onClick={onRefresh}>
          🔄 Refresh
        </Button>
      </div>

      {/* Summary Cards */}
      {data && (
        <Row className="mb-4">
          {/* Patterns Detected */}
          <Col md={3} className="mb-3">
            <Card className="stat-card h-100">
              <Card.Body>
                <div className="d-flex align-items-center">
                  <div className="me-3 text-primary" style={{ fontSize: '2rem' }}>
                    📈
                  </div>
                  <div>
                    <h6 className="text-muted mb-1">Patterns (24h)</h6>
                    <h3 className="mb-0">{data.patterns?.total_detected || 0}</h3>
                  </div>
                </div>
              </Card.Body>
            </Card>
          </Col>

          {/* Confirmed Patterns */}
          <Col md={3} className="mb-3">
            <Card className="stat-card h-100">
              <Card.Body>
                <div className="d-flex align-items-center">
                  <div className="me-3 text-success" style={{ fontSize: '2rem' }}>
                    ✅
                  </div>
                  <div>
                    <h6 className="text-muted mb-1">Confirmed</h6>
                    <h3 className="mb-0">{data.patterns?.by_status?.CONFIRMED || 0}</h3>
                  </div>
                </div>
              </Card.Body>
            </Card>
          </Col>

          {/* Alerts Generated */}
          <Col md={3} className="mb-3">
            <Card className="stat-card h-100">
              <Card.Body>
                <div className="d-flex align-items-center">
                  <div className="me-3 text-warning" style={{ fontSize: '2rem' }}>
                    🔔
                  </div>
                  <div>
                    <h6 className="text-muted mb-1">Alerts (24h)</h6>
                    <h3 className="mb-0">{data.alerts?.total || 0}</h3>
                  </div>
                </div>
              </Card.Body>
            </Card>
          </Col>

          {/* Active Auto-Entry Configs */}
          <Col md={3} className="mb-3">
            <Card className="stat-card h-100">
              <Card.Body>
                <div className="d-flex align-items-center">
                  <div className="me-3 text-info" style={{ fontSize: '2rem' }}>
                    ⚙️
                  </div>
                  <div>
                    <h6 className="text-muted mb-1">Auto-Entry Active</h6>
                    <h3 className="mb-0">{data.configs?.active_auto_entry || 0}</h3>
                  </div>
                </div>
              </Card.Body>
            </Card>
          </Col>
        </Row>
      )}

      {/* Status Breakdown */}
      {data?.patterns?.by_status && (
        <Card className="mb-4">
          <Card.Header>
            <strong>Pattern Status Breakdown (Last 24h)</strong>
          </Card.Header>
          <Card.Body>
            <Row>
              {Object.entries(data.patterns.by_status).map(([status, count]) => (
                <Col key={status} md={2} className="mb-2">
                  <div className="d-flex justify-content-between align-items-center">
                    <Badge bg={getStatusBadge(status)}>{status}</Badge>
                    <span className="ms-2">{count}</span>
                  </div>
                </Col>
              ))}
            </Row>
          </Card.Body>
        </Card>
      )}

      {/* Recent Confirmed Alerts */}
      <Card>
        <Card.Header>
          <strong>Recent Confirmed Patterns (Last 6 hours)</strong>
        </Card.Header>
        <Card.Body className="p-0">
          {recentAlerts.length === 0 ? (
            <div className="p-4 text-center text-muted">
              No confirmed patterns detected in the last 6 hours.
              <br />
              <small>Run a scan to detect new patterns.</small>
            </div>
          ) : (
            <Table hover responsive className="mb-0">
              <thead>
                <tr>
                  <th>Pattern</th>
                  <th>Symbol</th>
                  <th>Timeframe</th>
                  <th>Direction</th>
                  <th>Confidence</th>
                  <th>Detected At</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {recentAlerts.map((alert) => (
                  <tr key={alert.id}>
                    <td>
                      <strong>{alert.pattern_name}</strong>
                      <br />
                      <small className="text-muted">{alert.pattern_code}</small>
                    </td>
                    <td>
                      <Badge bg="secondary">{alert.symbol}</Badge>
                    </td>
                    <td>{alert.timeframe}</td>
                    <td>
                      <Badge bg={getDirectionBadge(alert.direction_bias)}>
                        {alert.direction_bias}
                      </Badge>
                    </td>
                    <td>
                      {alert.confidence ? (
                        <Badge bg={alert.confidence >= 0.75 ? 'success' : 'warning'}>
                          {(alert.confidence * 100).toFixed(0)}%
                        </Badge>
                      ) : (
                        '-'
                      )}
                    </td>
                    <td>
                      <small>{formatTime(alert.alert_ts)}</small>
                    </td>
                    <td>
                      <small className="text-muted">
                        Use Strategy Panel to create plan
                      </small>
                    </td>
                  </tr>
                ))}
              </tbody>
            </Table>
          )}
        </Card.Body>
      </Card>

      {/* Empty State - No Data */}
      {!data && (
        <Alert variant="info">
          <Alert.Heading>No Dashboard Data Available</Alert.Heading>
          <p>
            Pattern detection data will appear here once scans have been run.
            Go to the <strong>Active Strategy Panel</strong> tab to trigger a scan.
          </p>
        </Alert>
      )}
    </div>
  );
};

export default PatternDashboard;

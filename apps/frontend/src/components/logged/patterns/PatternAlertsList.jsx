/**
 * PatternAlertsList Component
 *
 * Displays recent confirmed pattern alerts with actions to send to plan.
 */

import React, { useState } from 'react';
import { Button, Table, Badge, Card, Alert, Form, Spinner, Modal } from 'react-bootstrap';
import { toast } from 'react-toastify';

const PatternAlertsList = ({ alerts, onRefresh, onPatternToPlan }) => {
  const [searchTerm, setSearchTerm] = useState('');
  const [hoursFilter, setHoursFilter] = useState(6);
  const [sendingToPlan, setSendingToPlan] = useState(null);
  const [showDetailModal, setShowDetailModal] = useState(false);
  const [selectedAlert, setSelectedAlert] = useState(null);

  // Filter alerts
  const filteredAlerts = alerts.filter((alert) => {
    const searchLower = searchTerm.toLowerCase();
    return (
      alert.pattern_name?.toLowerCase().includes(searchLower) ||
      alert.pattern_code?.toLowerCase().includes(searchLower) ||
      alert.symbol?.toLowerCase().includes(searchLower)
    );
  });

  // Format timestamp
  const formatTime = (timestamp) => {
    if (!timestamp) return '-';
    const date = new Date(timestamp);
    const now = new Date();
    const diffMs = now - date;
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMs / 3600000);

    if (diffMins < 60) {
      return `${diffMins}m ago`;
    } else if (diffHours < 24) {
      return `${diffHours}h ago`;
    } else {
      return date.toLocaleDateString();
    }
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

  // Handle send to plan
  const handleSendToPlan = async (alert) => {
    setSendingToPlan(alert.id);
    try {
      await onPatternToPlan(alert.instance);
      toast.success(`Pattern "${alert.pattern_name}" sent to execution pipeline`);
    } catch (err) {
      console.error('Failed to send to plan:', err);
    } finally {
      setSendingToPlan(null);
    }
  };

  // Show alert detail
  const showAlertDetail = (alert) => {
    setSelectedAlert(alert);
    setShowDetailModal(true);
  };

  return (
    <div>
      {/* Header with actions */}
      <div className="d-flex justify-content-between align-items-center mb-4">
        <div>
          <h4>Recent Confirmed Patterns</h4>
          <p className="text-muted mb-0">
            {filteredAlerts.length} confirmed patterns found
          </p>
        </div>
        <div className="d-flex gap-2 align-items-center">
          <Form.Select
            size="sm"
            style={{ width: 'auto' }}
            value={hoursFilter}
            onChange={(e) => {
              setHoursFilter(parseInt(e.target.value));
              onRefresh?.();
            }}
          >
            <option value={1}>Last 1 hour</option>
            <option value={6}>Last 6 hours</option>
            <option value={24}>Last 24 hours</option>
            <option value={72}>Last 3 days</option>
          </Form.Select>
          <Button variant="outline-primary" size="sm" onClick={onRefresh}>
            🔄 Refresh
          </Button>
        </div>
      </div>

      {/* Search */}
      <div className="mb-3">
        <div className="input-group">
          <span className="input-group-text">🔍</span>
          <Form.Control
            placeholder="Search by pattern, symbol..."
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
          />
        </div>
      </div>

      {/* Info Alert */}
      <Alert variant="info" className="mb-3">
        <strong>ℹ️ Sending Patterns to Plan</strong>
        <p className="mb-0">
          Clicking "Send to Plan" feeds the confirmed pattern into Robson's execution pipeline.
          This <strong>does NOT place orders</strong>. The pattern goes through PLAN → VALIDATE → EXECUTE flow.
        </p>
      </Alert>

      {/* Empty State */}
      {alerts.length === 0 ? (
        <Alert variant="warning">
          <Alert.Heading>No Confirmed Patterns Found</Alert.Heading>
          <p>
            No confirmed patterns detected in the selected time period.
            Try running a scan from the <strong>Active Strategy Panel</strong> tab.
          </p>
          <Button variant="primary" onClick={() => window.location.hash = '#strategy'}>
            📈 Go to Strategy Panel
          </Button>
        </Alert>
      ) : (
        <Card>
          <Card.Body className="p-0">
            <Table hover responsive className="mb-0">
              <thead>
                <tr>
                  <th>Pattern</th>
                  <th>Symbol / Timeframe</th>
                  <th>Direction</th>
                  <th>Confidence</th>
                  <th>Detected</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {filteredAlerts.length === 0 ? (
                  <tr>
                    <td colSpan={6} className="text-center text-muted py-4">
                      No alerts match your search.
                    </td>
                  </tr>
                ) : (
                  filteredAlerts.map((alert) => (
                    <tr key={alert.id}>
                      <td
                        style={{ cursor: 'pointer' }}
                        onClick={() => showAlertDetail(alert)}
                      >
                        <strong>{alert.pattern_name}</strong>
                        <br />
                        <small className="text-muted">{alert.pattern_code}</small>
                      </td>
                      <td>
                        <Badge bg="secondary">{alert.symbol}</Badge>
                        <span className="mx-1">|</span>
                        <Badge bg="info">{alert.timeframe}</Badge>
                      </td>
                      <td>
                        <Badge bg={getDirectionBadge(alert.direction_bias)}>
                          {alert.direction_bias}
                        </Badge>
                      </td>
                      <td>
                        {alert.confidence !== null && alert.confidence !== undefined ? (
                          <Badge bg={alert.confidence >= 0.75 ? 'success' : 'warning'}>
                            {(alert.confidence * 100).toFixed(0)}%
                          </Badge>
                        ) : (
                          <span className="text-muted">-</span>
                        )}
                      </td>
                      <td>
                        <small>{formatTime(alert.alert_ts)}</small>
                      </td>
                      <td>
                        {sendingToPlan === alert.id ? (
                          <Button size="sm" variant="success" disabled>
                            <Spinner animation="border" size="sm" />
                          </Button>
                        ) : (
                          <Button
                            size="sm"
                            variant="success"
                            onClick={() => handleSendToPlan(alert)}
                            title="Send to execution pipeline"
                          >
                            📨 Send to Plan
                          </Button>
                        )}
                      </td>
                    </tr>
                  ))
                )}
              </tbody>
            </Table>
          </Card.Body>
        </Card>
      )}

      {/* Alert Detail Modal */}
      <Modal show={showDetailModal} onHide={() => setShowDetailModal(false)} size="lg">
        <Modal.Header closeButton>
          <Modal.Title>
            Pattern Details: {selectedAlert?.pattern_name}
          </Modal.Title>
        </Modal.Header>
        <Modal.Body>
          {selectedAlert && (
            <div>
              <Row>
                <Col md={6}>
                  <p><strong>Pattern:</strong> {selectedAlert.pattern_name}</p>
                  <p><strong>Code:</strong> {selectedAlert.pattern_code}</p>
                  <p><strong>Symbol:</strong> {selectedAlert.symbol}</p>
                  <p><strong>Timeframe:</strong> {selectedAlert.timeframe}</p>
                  <p><strong>Direction:</strong> {selectedAlert.direction_bias}</p>
                </Col>
                <Col md={6}>
                  <p><strong>Alert Type:</strong> {selectedAlert.alert_type_display}</p>
                  <p><strong>Confidence:</strong> {selectedAlert.confidence ? `${(selectedAlert.confidence * 100).toFixed(1)}%` : 'N/A'}</p>
                  <p><strong>Detected At:</strong> {new Date(selectedAlert.alert_ts).toLocaleString()}</p>
                </Col>
              </Row>
              {selectedAlert.payload && (
                <div className="mt-3">
                  <strong>Additional Data:</strong>
                  <pre className="bg-dark text-light p-3 rounded mt-2" style={{ fontSize: '0.8rem' }}>
                    {JSON.stringify(selectedAlert.payload, null, 2)}
                  </pre>
                </div>
              )}
            </div>
          )}
        </Modal.Body>
        <Modal.Footer>
          <Button variant="secondary" onClick={() => setShowDetailModal(false)}>
            Close
          </Button>
          {selectedAlert && (
            <Button
              variant="success"
              onClick={() => {
                handleSendToPlan(selectedAlert);
                setShowDetailModal(false);
              }}
              disabled={sendingToPlan !== null}
            >
              {sendingToPlan === selectedAlert.id ? (
                <>
                  <Spinner animation="border" size="sm" className="me-2" />
                  Sending...
                </>
              ) : (
                <>
                  📨 Send to Plan
                </>
              )}
            </Button>
          )}
        </Modal.Footer>
      </Modal>
    </div>
  );
};

export default PatternAlertsList;

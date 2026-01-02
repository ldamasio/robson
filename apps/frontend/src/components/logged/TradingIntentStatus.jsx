import React, { useState, useContext } from 'react';
import { useNavigate } from 'react-router-dom';
import Card from 'react-bootstrap/Card';
import Badge from 'react-bootstrap/Badge';
import Button from 'react-bootstrap/Button';
import Spinner from 'react-bootstrap/Spinner';
import Alert from 'react-bootstrap/Alert';
import Accordion from 'react-bootstrap/Accordion';
import Row from 'react-bootstrap/Row';
import Col from 'react-bootstrap/Col';
import OverlayTrigger from 'react-bootstrap/OverlayTrigger';
import Tooltip from 'react-bootstrap/Tooltip';
import Table from 'react-bootstrap/Table';
import AuthContext from '../../context/AuthContext';
import { useTradingIntent } from '../../hooks/useTradingIntent';
import TradingIntentResults from './TradingIntentResults';
import { showSuccess, showInfo, showError, showWarning } from '../../utils/notifications';
import PropTypes from 'prop-types';

/**
 * TradingIntentStatus - Display agentic workflow status (PLAN → VALIDATE → EXECUTE).
 *
 * Shows a trading intent's lifecycle, validation results, and execution results in real-time.
 * Automatically polls for status updates during transitional states.
 *
 * @param {Object} props
 * @param {string} props.intentId - Trading intent ID to display
 * @param {boolean} props.showDetails - Show validation/execution details by default
 * @param {Function} props.onValidate - Callback for "Validate Now" button
 * @param {Function} props.onExecute - Callback for "Execute Now" button
 */
function TradingIntentStatus({ intentId, showDetails = true, onValidate, onExecute }) {
  const { authTokens } = useContext(AuthContext);
  const navigate = useNavigate();

  // Component state
  const [expanded, setExpanded] = useState({ validation: true, execution: true });
  const [executionMode, setExecutionMode] = useState('dry-run');
  const [isExecuting, setIsExecuting] = useState(false);

  // Fetch trading intent with polling
  const { intent, isLoading, error, refetch, isPolling } = useTradingIntent(
    intentId,
    authTokens,
    { pollingInterval: 5000, enablePolling: true }
  );

  // Handle validate button click
  const handleValidate = async () => {
    if (!onValidate) {
      // Default behavior: call API directly
      try {
        const response = await fetch(
          `${import.meta.env.VITE_API_BASE_URL}/api/trading-intents/${intentId}/validate/`,
          {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
              Authorization: `Bearer ${authTokens.access}`,
            },
          }
        );
        if (!response.ok) {
          throw new Error('Validation failed');
        }
        const result = await response.json();
        if (result.validation_result?.status === 'PASS') {
          showSuccess('Validation passed! Ready to execute.');
        } else if (result.validation_result?.status === 'FAIL') {
          showError('Validation failed. Please check the details below.');
        } else {
          showWarning('Validation completed with warnings. Check details below.');
        }
        refetch();
      } catch (err) {
        console.error('Failed to validate:', err);
        showError('Validation failed. Please try again.');
      }
    } else {
      onValidate(intentId);
    }
  };

  // Handle execute button click
  const handleExecute = async () => {
    if (executionMode === 'live') {
      // Show warning toast for live execution
      showWarning('LIVE execution mode: Real orders will be placed on Binance!');
      // Still require typed confirmation for safety
      const typed = window.prompt('Type "CONFIRM" to proceed with LIVE execution:');
      if (typed !== 'CONFIRM') {
        showError('Live execution cancelled.');
        return;
      }
    }

    setIsExecuting(true);
    // Show waiting state
    if (executionMode === 'live') {
      showInfo('Connecting to exchange. Placing order...');
    } else {
      showInfo('Running simulation...');
    }
    try {
      const payload = {
        mode: executionMode.toUpperCase(),
        acknowledge_risk: executionMode === 'live',
      };

      if (!onExecute) {
        // Default behavior: call API directly
        const response = await fetch(
          `${import.meta.env.VITE_API_BASE_URL}/api/trading-intents/${intentId}/execute/`,
          {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
              Authorization: `Bearer ${authTokens.access}`,
            },
            body: JSON.stringify(payload),
          }
        );
        if (!response.ok) {
          const errorData = await response.json();
          throw new Error(errorData.detail || 'Execution failed');
        }

        const result = await response.json();
        if (result.execution_result?.status === 'SUCCESS') {
          if (executionMode === 'live') {
            showSuccess('Live execution successful! Orders placed on Binance.');
          } else {
            showInfo('Dry-run completed. No real orders placed.');
          }
        } else {
          showError('Execution failed. Check the details below.');
        }
      } else {
        onExecute(intentId, payload);
      }
      refetch();
    } catch (err) {
      console.error('Failed to execute:', err);
      showError(err.message || 'Failed to execute trading intent');
    } finally {
      setIsExecuting(false);
    }
  };

  // Handle cancel button click
  const handleCancel = async () => {
    if (!window.confirm('Are you sure you want to cancel this trading intent?')) {
      return;
    }

    try {
      const response = await fetch(
        `${import.meta.env.VITE_API_BASE_URL}/api/trading-intents/${intentId}/cancel/`,
        {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: `Bearer ${authTokens.access}`,
          },
        }
      );
      if (!response.ok) {
        throw new Error('Cancel failed');
      }
      showInfo('Trading intent cancelled successfully.');
      refetch();
    } catch (err) {
      console.error('Failed to cancel:', err);
      showError('Failed to cancel trading intent');
    }
  };

  // Copy intent ID to clipboard
  const copyIntentId = () => {
    navigator.clipboard.writeText(intentId);
  };

  // Get status badge variant
  const getStatusVariant = (status) => {
    switch (status) {
      case 'PENDING':
        return 'warning';
      case 'VALIDATED':
        return 'info';
      case 'EXECUTING':
        return 'primary';
      case 'EXECUTED':
        return 'success';
      case 'FAILED':
        return 'danger';
      case 'CANCELLED':
        return 'secondary';
      default:
        return 'secondary';
    }
  };

  // Loading state
  if (isLoading && !intent) {
    return (
      <Card>
        <Card.Body className="text-center py-5">
          <Spinner animation="border" role="status">
            <span className="visually-hidden">Loading...</span>
          </Spinner>
          <p className="mt-3 mb-0">Loading trading intent...</p>
        </Card.Body>
      </Card>
    );
  }

  // Error state
  if (error && !intent) {
    return (
      <Alert variant="danger">
        <Alert.Heading>Error Loading Trading Intent</Alert.Heading>
        <p>{error}</p>
        <hr />
        <div className="d-flex justify-content-end">
          <Button variant="outline-danger" onClick={refetch}>
            Retry
          </Button>
        </div>
      </Alert>
    );
  }

  if (!intent) {
    return (
      <Alert variant="warning">
        Trading intent not found. It may have been deleted or you may not have permission to view it.
      </Alert>
    );
  }

  const {
    status,
    symbol,
    strategy,
    side,
    quantity,
    entry_price,
    stop_price,
    capital,
    risk_amount,
    risk_percent,
    validation_result,
    execution_result,
    created_at,
    updated_at,
    // Pattern trigger metadata (Phase 5 MVP)
    pattern_code,
    pattern_source,
    pattern_event_id,
    pattern_triggered_at,
  } = intent;

  // Calculate stop distance percentage
  const stopDistance = entry_price && stop_price
    ? Math.abs((parseFloat(entry_price) - parseFloat(stop_price)) / parseFloat(entry_price) * 100)
    : 0;

  // Format relative time
  const formatRelativeTime = (dateString) => {
    const date = new Date(dateString);
    const now = new Date();
    const diffMs = now - date;
    const diffMins = Math.floor(diffMs / 60000);

    if (diffMins < 1) return 'Just now';
    if (diffMins < 60) return `${diffMins} minute${diffMins > 1 ? 's' : ''} ago`;
    const diffHours = Math.floor(diffMins / 60);
    if (diffHours < 24) return `${diffHours} hour${diffHours > 1 ? 's' : ''} ago`;
    const diffDays = Math.floor(diffHours / 24);
    return `${diffDays} day${diffDays > 1 ? 's' : ''} ago`;
  };

  return (
    <div>
      {/* Live updates indicator */}
      {isPolling && (
        <Alert variant="info" className="mb-3 d-flex align-items-center">
          <Spinner animation="border" size="sm" className="me-2" />
          <span className="me-auto">Live updates enabled</span>
          <Button variant="link" className="p-0 text-decoration-none" onClick={refetch}>
            Refresh now
          </Button>
        </Alert>
      )}

      {/* Pattern trigger info (Phase 5 MVP) */}
      {pattern_source === 'pattern' && (
        <Alert variant="primary" className="mb-3">
          <strong>Triggered by Pattern:</strong> {pattern_code || 'Unknown'}
          {pattern_triggered_at && (
            <small className="ms-2">
              at {new Date(pattern_triggered_at).toLocaleString()}
            </small>
          )}
          {pattern_event_id && (
            <div className="mt-1">
              <small className="text-muted">
                Event ID: {pattern_event_id.substring(0, 32)}...
              </small>
            </div>
          )}
        </Alert>
      )}

      {/* Main Card */}
      <Card>
        {/* Header */}
        <Card.Header className="d-flex justify-content-between align-items-center flex-wrap gap-2">
          <div className="d-flex align-items-center gap-2 flex-wrap">
            <OverlayTrigger
              placement="top"
              overlay={<Tooltip>{intentId}</Tooltip>}
            >
              <code className="text-primary">
                {intentId.substring(0, 8)}...{intentId.slice(-4)}
              </code>
            </OverlayTrigger>
            <Button variant="link" size="sm" className="p-0" onClick={copyIntentId}>
              📋
            </Button>
            <Badge bg={getStatusVariant(status)} className="fs-6">
              {status}
            </Badge>
          </div>
          <small className="text-muted">
            Updated {formatRelativeTime(updated_at)}
          </small>
        </Card.Header>

        <Card.Body>
          {/* Trade Details */}
          <Row className="mb-3">
            <Col md={6}>
              <p className="mb-1"><strong>Symbol:</strong> {symbol?.name || symbol?.base_asset + '/' + symbol?.quote_asset}</p>
              <p className="mb-1"><strong>Strategy:</strong> {strategy?.name || '-'}</p>
              <p className="mb-1"><strong>Side:</strong> <Badge bg={side === 'BUY' ? 'success' : 'danger'}>{side}</Badge></p>
            </Col>
            <Col md={6}>
              <p className="mb-1"><strong>Entry Price:</strong> ${entry_price}</p>
              <p className="mb-1"><strong>Stop Price:</strong> ${stop_price}</p>
              <p className="mb-1"><strong>Quantity:</strong> {quantity}</p>
            </Col>
          </Row>

          <Row className="mb-3">
            <Col md={6}>
              <p className="mb-1"><strong>Capital:</strong> ${capital}</p>
              <p className="mb-1"><strong>Risk Amount:</strong> ${risk_amount} ({risk_percent}%)</p>
            </Col>
            <Col md={6}>
              <p className="mb-1"><strong>Stop Distance:</strong> {stopDistance.toFixed(2)}%</p>
              <p className="mb-1"><strong>Created:</strong> {new Date(created_at).toLocaleString()}</p>
            </Col>
          </Row>

          {/* Validation Section */}
          {validation_result && showDetails && (
            <Accordion defaultActiveKey="0" className="mb-3">
              <Accordion.Item eventKey="validation">
                <Accordion.Header onClick={() => setExpanded({ ...expanded, validation: !expanded.validation })}>
                  <div className="d-flex align-items-center gap-2 w-100">
                    <span className="me-auto">
                      <strong>Validation Results</strong>
                    </span>
                    <Badge bg={validation_result.status === 'PASS' ? 'success' : 'danger'}>
                      {validation_result.status}
                    </Badge>
                  </div>
                </Accordion.Header>
                <Accordion.Body>
                  {/* Guards */}
                  <h6>Guards</h6>
                  {validation_result.guards?.map((guard, index) => (
                    <Alert
                      key={index}
                      variant={guard.status === 'PASS' ? 'success' : 'danger'}
                      className="d-flex align-items-start"
                    >
                      <span className="me-2">
                        {guard.status === 'PASS' ? '✓' : '✗'}
                      </span>
                      <div className="flex-grow-1">
                        <strong>{guard.name}</strong>
                        <p className="mb-0">{guard.message}</p>
                        {guard.details && (
                          <small className="text-muted">{guard.details}</small>
                        )}
                      </div>
                    </Alert>
                  ))}

                  {/* Warnings */}
                  {validation_result.warnings?.length > 0 && (
                    <>
                      <h6 className="mt-3">Warnings</h6>
                      {validation_result.warnings.map((warning, index) => (
                        <Alert key={index} variant="warning" className="d-flex align-items-start">
                          <span className="me-2">⚠️</span>
                          <div className="flex-grow-1">
                            <p className="mb-0">{warning}</p>
                          </div>
                        </Alert>
                      ))}
                    </>
                  )}

                  <p className="text-muted mb-0 mt-2">
                    <small>Validated at: {new Date(validation_result.validated_at).toLocaleString()}</small>
                  </p>
                </Accordion.Body>
              </Accordion.Item>
            </Accordion>
          )}

          {/* Execution Section */}
          {execution_result && showDetails && (
            <TradingIntentResults
              executionResult={execution_result}
              intentId={intentId}
            />
          )}

          {/* Action Buttons */}
          <div className="d-flex flex-wrap gap-2 mt-3">
            {/* Refresh button */}
            <Button variant="outline-secondary" onClick={refetch} disabled={isLoading}>
              Refresh
            </Button>

            {/* Validate button (PENDING state) */}
            {status === 'PENDING' && (
              <Button variant="primary" onClick={handleValidate}>
                Validate Now
              </Button>
            )}

            {/* Execute buttons (VALIDATED state) */}
            {status === 'VALIDATED' && (
              <>
                <Button
                  variant={executionMode === 'dry-run' ? 'primary' : 'outline-primary'}
                  onClick={() => setExecutionMode('dry-run')}
                  disabled={isExecuting}
                >
                  Dry-Run
                </Button>
                <Button
                  variant={executionMode === 'live' ? 'danger' : 'outline-danger'}
                  onClick={() => setExecutionMode('live')}
                  disabled={isExecuting}
                >
                  Live
                </Button>
                <Button
                  variant="success"
                  onClick={handleExecute}
                  disabled={isExecuting}
                >
                  {isExecuting ? (
                    <>
                      <Spinner as="span" animation="border" size="sm" className="me-2" />
                      Executing...
                    </>
                  ) : (
                    `Execute (${executionMode.toUpperCase()})`
                  )}
                </Button>
              </>
            )}

            {/* Cancel button (PENDING or VALIDATED state) */}
            {(status === 'PENDING' || status === 'VALIDATED') && (
              <Button variant="outline-danger" onClick={handleCancel}>
                Cancel
              </Button>
            )}

            {/* View in Binance (if executed with live mode) */}
            {status === 'EXECUTED' && execution_result?.mode === 'LIVE' && execution_result?.actions?.[0]?.order_id && (
              <Button
                variant="outline-dark"
                href={`https://www.binance.com/en/my/orders`}
                target="_blank"
                rel="noopener noreferrer"
              >
                View in Binance →
              </Button>
            )}
          </div>
        </Card.Body>
      </Card>

      {/* Execution Result Panel - shows for EXECUTED intents */}
      {status === 'EXECUTED' && execution_result && (
        <Card className="mt-3 border-success">
          <Card.Header className="bg-success text-white d-flex justify-content-between align-items-center">
            <h5 className="mb-0">
              {execution_result.mode === 'LIVE' ? '🔴 Live Execution' : '🔵 Dry-Run Complete'}
            </h5>
            <Badge bg={execution_result.mode === 'LIVE' ? 'danger' : 'primary'}>
              {execution_result.mode}
            </Badge>
          </Card.Header>
          <Card.Body>
            <p className="mb-2">
              <strong>Causality:</strong> You authorized execution → {
                execution_result.mode === 'LIVE' ? 'Authorization submitted' : 'Simulation completed'
              }
            </p>

            {/* Order details from execution_result.actions - defensive rendering */}
            {execution_result.actions && execution_result.actions.length > 0 ? (
              <div className="mt-3">
                <h6>Order Details</h6>
                <Table size="sm" bordered>
                  <thead>
                    <tr>
                      <th>Type</th>
                      <th>Symbol</th>
                      <th>Side</th>
                      <th>Quantity</th>
                      <th>Price</th>
                      <th>Status</th>
                    </tr>
                  </thead>
                  <tbody>
                    {execution_result.actions.map((action, idx) => (
                      <tr key={idx}>
                        <td><Badge bg="secondary">{action.type || 'UNKNOWN'}</Badge></td>
                        <td>{action.symbol || '-'}</td>
                        <td>
                          {action.side ? (
                            <Badge bg={action.side === 'BUY' ? 'success' : 'danger'}>{action.side}</Badge>
                          ) : '-'}
                        </td>
                        <td>{action.quantity || '-'}</td>
                        <td>{action.price || action.stop_price || '-'}</td>
                        <td>
                          <Badge bg={
                            action.status === 'SIMULATED' ? 'primary' :
                            action.status === 'FILLED' ? 'success' :
                            action.status === 'PENDING' ? 'warning' :
                            action.status === 'FAILED' ? 'danger' : 'secondary'
                          }>
                            {action.status || 'UNKNOWN'}
                          </Badge>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </Table>

                {/* Stop price from SIMULATED_STOP action - defensive */}
                {execution_result.actions.find(a => a?.type === 'SIMULATED_STOP' && a?.stop_price) && (
                  <p className="mt-2 mb-0 small text-muted">
                    Stop Loss: {execution_result.actions.find(a => a?.type === 'SIMULATED_STOP')?.stop_price}
                  </p>
                )}
              </div>
            ) : (
              <p className="text-muted small mb-0">No order details available.</p>
            )}

            {/* External verification link for LIVE mode - defensive */}
            {execution_result.mode === 'LIVE' && (
              <div className="mt-3">
                <Alert variant="info" className="mb-0">
                  <strong>Verify on Binance:</strong> Check your orders on exchange.{' '}
                  <Alert.Link href="https://www.binance.com/en/my/orders" target="_blank" rel="noopener noreferrer">
                    View Binance orders →
                  </Alert.Link>
                </Alert>
              </div>
            )}
          </Card.Body>
        </Card>
      )}
    </div>
  );
}

TradingIntentStatus.propTypes = {
  intentId: PropTypes.string.isRequired,
  showDetails: PropTypes.bool,
  onValidate: PropTypes.func,
  onExecute: PropTypes.func,
};

export default TradingIntentStatus;

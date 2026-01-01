import React, { useState } from 'react';
import Card from 'react-bootstrap/Card';
import Table from 'react-bootstrap/Table';
import Badge from 'react-bootstrap/Badge';
import Alert from 'react-bootstrap/Alert';
import Accordion from 'react-bootstrap/Accordion';
import Button from 'react-bootstrap/Button';
import PropTypes from 'prop-types';

/**
 * TradingIntentResults - Detailed view of execution results.
 *
 * Displays execution results including:
 * - Execution mode (DRY-RUN / LIVE)
 * - Status badge (SUCCESS / FAILED)
 * - Actions table with order details
 * - Errors and warnings
 *
 * @param {Object} props
 * @param {Object} props.executionResult - Execution result from API
 * @param {string} props.intentId - Trading intent ID
 */
function TradingIntentResults({ executionResult, intentId }) {
  const [showDetails, setShowDetails] = useState(true);

  if (!executionResult) {
    return null;
  }

  const { status, mode, actions, audit_trail, errors, warnings, executed_at } = executionResult;

  // Get status badge variant
  const getStatusVariant = (executionStatus) => {
    switch (executionStatus) {
      case 'SUCCESS':
        return 'success';
      case 'FAILED':
        return 'danger';
      case 'PARTIAL':
        return 'warning';
      default:
        return 'secondary';
    }
  };

  // Get mode badge variant
  const getModeVariant = (executionMode) => {
    return executionMode === 'LIVE' ? 'danger' : 'primary';
  };

  // Calculate total fees
  const totalFees = actions?.reduce((sum, action) => {
    return sum + (action.fee ? parseFloat(action.fee) : 0);
  }, 0) || 0;

  return (
    <Card className="mt-3 border-success">
      <Card.Header className="d-flex justify-content-between align-items-center bg-success text-white">
        <div className="d-flex align-items-center gap-2">
          <h5 className="mb-0">Execution Results</h5>
          <Badge bg={getStatusVariant(status)}>{status}</Badge>
          <Badge bg={getModeVariant(mode)}>{mode}</Badge>
        </div>
        <Button
          variant="link"
          className="text-white"
          onClick={() => setShowDetails(!showDetails)}
        >
          {showDetails ? '▲' : '▼'}
        </Button>
      </Card.Header>

      {showDetails && (
        <Card.Body>
          {/* Timestamp */}
          {executed_at && (
            <p className="text-muted mb-3">
              <small>Executed at: {new Date(executed_at).toLocaleString()}</small>
            </p>
          )}

          {/* Actions Table */}
          {actions && actions.length > 0 && (
            <div className="mb-3">
              <h6>Actions</h6>
              <Table striped bordered hover size="sm">
                <thead>
                  <tr>
                    <th>Type</th>
                    <th>Asset</th>
                    <th>Quantity</th>
                    <th>Price</th>
                    <th>Order ID</th>
                    <th>Status</th>
                  </tr>
                </thead>
                <tbody>
                  {actions.map((action, index) => (
                    <tr key={index}>
                      <td>
                        <Badge bg="secondary">{action.type}</Badge>
                      </td>
                      <td>{action.asset}</td>
                      <td>{action.quantity}</td>
                      <td>{action.price || '-'}</td>
                      <td>
                        {action.order_id ? (
                          <a
                            href={`https://www.binance.com/en/my/orders/${action.order_id}`}
                            target="_blank"
                            rel="noopener noreferrer"
                            className="text-decoration-none"
                          >
                            {action.order_id}
                          </a>
                        ) : (
                          '-'
                        )}
                      </td>
                      <td>
                        <Badge bg={action.status === 'FILLED' ? 'success' : 'warning'}>
                          {action.status}
                        </Badge>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </Table>

              {/* Total Fees */}
              {totalFees > 0 && (
                <p className="text-muted mb-0">
                  <small>Total fees: {totalFees.toFixed(8)} USDT</small>
                </p>
              )}
            </div>
          )}

          {/* Audit Trail */}
          {audit_trail && audit_trail.length > 0 && (
            <div className="mb-3">
              <h6>Audit Trail</h6>
              <ul className="list-unstyled">
                {audit_trail.map((entry, index) => (
                  <li key={index} className="mb-1">
                    <Badge bg="info" className="me-2">
                      {entry.action}
                    </Badge>
                    <span>
                      {entry.amount && `${entry.amount} `}
                      {entry.asset && `${entry.asset} `}
                      {entry.stop_price && `@ ${entry.stop_price}`}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {/* Errors */}
          {errors && errors.length > 0 && (
            <Accordion className="mb-3">
              <Accordion.Item eventKey="errors">
                <Accordion.Header>
                  <span className="text-danger">
                    <strong>Errors</strong> ({errors.length})
                  </span>
                </Accordion.Header>
                <Accordion.Body>
                  {errors.map((error, index) => (
                    <Alert variant="danger" key={index} className="mb-2">
                      {error}
                    </Alert>
                  ))}
                </Accordion.Body>
              </Accordion.Item>
            </Accordion>
          )}

          {/* Warnings */}
          {warnings && warnings.length > 0 && (
            <Accordion className="mb-3">
              <Accordion.Item eventKey="warnings">
                <Accordion.Header>
                  <span className="text-warning">
                    <strong>Warnings</strong> ({warnings.length})
                  </span>
                </Accordion.Header>
                <Accordion.Body>
                  {warnings.map((warning, index) => (
                    <Alert variant="warning" key={index} className="mb-2">
                      {warning}
                    </Alert>
                  ))}
                </Accordion.Body>
              </Accordion.Item>
            </Accordion>
          )}

          {/* Success Message */}
          {status === 'SUCCESS' && !errors?.length && (
            <Alert variant="success">
              <strong>Success!</strong> Trading intent executed successfully in {mode} mode.
            </Alert>
          )}
        </Card.Body>
      )}
    </Card>
  );
}

TradingIntentResults.propTypes = {
  executionResult: PropTypes.object,
  intentId: PropTypes.string,
};

export default TradingIntentResults;

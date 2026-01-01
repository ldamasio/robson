import React, { useContext } from 'react';
import Card from 'react-bootstrap/Card';
import Row from 'react-bootstrap/Row';
import Col from 'react-bootstrap/Col';
import Badge from 'react-bootstrap/Badge';
import Button from 'react-bootstrap/Button';
import Spinner from 'react-bootstrap/Spinner';
import Placeholder from 'react-bootstrap/Placeholder';
import Alert from 'react-bootstrap/Alert';
import AuthContext from '../../context/AuthContext';
import { useNavigate } from 'react-router-dom';
import { useTradingIntentsList } from '../../hooks/useTradingIntentsList';
import PropTypes from 'prop-types';

/**
 * TradingIntentsList - Display recent trading intents on Dashboard.
 *
 * Shows cards for recent PENDING and VALIDATED trading intents with action buttons.
 *
 * @param {Object} props
 * @param {string} props.highlightIntentId - Intent ID to highlight (flash animation)
 */
function TradingIntentsList({ highlightIntentId }) {
  const { authTokens } = useContext(AuthContext);
  const navigate = useNavigate();

  const {
    intents,
    isLoading,
    error,
    refetch,
    autoRefreshEnabled,
    toggleAutoRefresh,
  } = useTradingIntentsList({
    authToken: authTokens?.access,
    statuses: ['PENDING', 'VALIDATED'],
    limit: 10,
    enableAutoRefresh: true,
    refreshInterval: 30000,
  });

  // Handle view details
  const handleViewDetails = (intentId) => {
    navigate(`/trading-intent/${intentId}`);
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
      default:
        return 'secondary';
    }
  };

  // Format date safely
  const formatDate = (dateString) => {
    if (!dateString) return 'Unknown';
    try {
      return new Date(dateString).toLocaleString();
    } catch {
      return 'Invalid date';
    }
  };

  // Get safe intent ID display
  const getIntentIdDisplay = (intent) => {
    if (!intent?.id) return 'Unknown ID';
    return `${intent.id.substring(0, 8)}...`;
  };

  // Render loading skeleton
  const renderSkeleton = () => (
    <Row xs={1} md={2} lg={3} className="g-3">
      {[1, 2, 3].map((i) => (
        <Col key={i}>
          <Card>
            <Card.Body>
              <Placeholder as={Card.Title} animation="wave" />
              <Placeholder as={Card.Text} animation="wave">
                <Placeholder xs={7} />
                <Placeholder xs={4} />
              </Placeholder>
              <Placeholder.Button variant="primary" xs={4} />
            </Card.Body>
          </Card>
        </Col>
      ))}
    </Row>
  );

  // Render empty state
  const renderEmpty = () => (
    <Alert variant="info" className="text-center py-4">
      <h4>No Trading Plans Yet</h4>
      <p className="mb-3">
        Create your first trading plan to get started. It will be validated against
        your account constraints before execution.
      </p>
      <Button variant="primary" onClick={() => document.getElementById('start-new-operation-btn')?.scrollIntoView({ behavior: 'smooth' })}>
        Go to Start New Operation
      </Button>
    </Alert>
  );

  // Render intent card
  const renderIntentCard = (intent) => (
    <Col key={intent.id} xs={12} md={6} lg={4} className="mb-3">
      <Card
        className={`h-100 ${highlightIntentId === intent.id ? 'border-warning' : ''}`}
        style={
          highlightIntentId === intent.id
            ? { animation: 'flash 1s ease-in-out 3' }
            : {}
        }
      >
        <Card.Header className="d-flex justify-content-between align-items-start">
          <div>
            <small className="text-muted">ID: {getIntentIdDisplay(intent)}</small>
            <br />
            <Badge bg={getStatusVariant(intent.status)} className="mt-1">
              {intent.status}
            </Badge>
          </div>
          <small className="text-muted">
            {formatDate(intent.created_at)}
          </small>
        </Card.Header>

        <Card.Body>
          <h6>{intent.symbol_display || intent.symbol?.name || 'Unknown'}</h6>
          <p className="mb-1">
            <strong>Strategy:</strong> {intent.strategy?.name || '-'}
          </p>
          <p className="mb-1">
            <strong>Side:</strong>{' '}
            <Badge bg={intent.side === 'BUY' ? 'success' : 'danger'}>
              {intent.side}
            </Badge>
          </p>
          <p className="mb-1">
            <strong>Entry:</strong> ${intent.entry_price}
          </p>
          <p className="mb-1">
            <strong>Stop:</strong> ${intent.stop_price}
          </p>
          <p className="mb-0">
            <strong>Quantity:</strong> {intent.quantity}
          </p>
        </Card.Body>

        <Card.Footer className="d-flex flex-wrap gap-2">
          <Button
            variant="outline-primary"
            size="sm"
            onClick={() => handleViewDetails(intent.id)}
            className="flex-grow-1"
          >
            Open
          </Button>
          {intent.status === 'PENDING' && (
            <Button
              variant="outline-success"
              size="sm"
              onClick={() => handleViewDetails(intent.id)}
            >
              Validate →
            </Button>
          )}
          {intent.status === 'VALIDATED' && (
            <Button
              variant="outline-success"
              size="sm"
              onClick={() => handleViewDetails(intent.id)}
            >
              Execute →
            </Button>
          )}
        </Card.Footer>
      </Card>
    </Col>
  );

  return (
    <div className="mb-4">
      <div className="d-flex justify-content-between align-items-center mb-3">
        <h3>Trading Plans</h3>
        <div className="d-flex gap-2">
          <Button
            variant="outline-secondary"
            size="sm"
            onClick={refetch}
            disabled={isLoading}
          >
            {isLoading ? (
              <Spinner as="span" animation="border" size="sm" />
            ) : (
              'Refresh'
            )}
          </Button>
          <Button
            variant={autoRefreshEnabled ? 'primary' : 'outline-primary'}
            size="sm"
            onClick={toggleAutoRefresh}
          >
            {autoRefreshEnabled ? 'Auto-refresh On' : 'Auto-refresh Off'}
          </Button>
        </div>
      </div>

      {error && (
        <Alert variant="danger" dismissible onClose={() => {}}>
          Failed to load trading plans: {error}
        </Alert>
      )}

      {isLoading ? (
        renderSkeleton()
      ) : intents.length === 0 ? (
        renderEmpty()
      ) : (
        <Row xs={1} md={2} lg={3} className="g-3">
          {intents.map(renderIntentCard)}
        </Row>
      )}

      <style>{`
        @keyframes flash {
          0%, 100% { opacity: 1; }
          50% { opacity: 0.5; }
        }
      `}</style>
    </div>
  );
}

TradingIntentsList.propTypes = {
  highlightIntentId: PropTypes.string,
};

export default TradingIntentsList;

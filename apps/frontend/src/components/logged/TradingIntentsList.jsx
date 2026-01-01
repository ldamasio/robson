import React, { useState, useEffect, useContext } from 'react';
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
import PropTypes from 'prop-types';

/**
 * TradingIntentsList - Display recent trading intents on Dashboard.
 *
 * Shows cards for recent PENDING and VALIDATED trading intents with action buttons.
 *
 * @param {Object} props
 * @param {Function} props.onIntentCreated - Callback when a new intent is created
 * @param {string} props.highlightIntentId - Intent ID to highlight (flash animation)
 */
function TradingIntentsList({ onIntentCreated, highlightIntentId }) {
  const { authTokens } = useContext(AuthContext);
  const navigate = useNavigate();

  const [intents, setIntents] = useState([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState(null);
  const [autoRefresh, setAutoRefresh] = useState(true);

  // Fetch trading intents
  const fetchIntents = async () => {
    if (!authTokens?.access) return;

    try {
      setError(null);
      const response = await fetch(
        `${import.meta.env.VITE_API_BASE_URL}/api/trading-intents/?limit=10&status=PENDING&status=VALIDATED`,
        {
          headers: {
            'Content-Type': 'application/json',
            Authorization: `Bearer ${authTokens.access}`,
          },
        }
      );

      if (!response.ok) {
        throw new Error('Failed to fetch trading intents');
      }

      const data = await response.json();
      setIntents(data.results || data);
    } catch (err) {
      console.error('Failed to fetch trading intents:', err);
      setError(err.message);
    } finally {
      setIsLoading(false);
    }
  };

  // Initial fetch and auto-refresh
  useEffect(() => {
    fetchIntents();

    let interval;
    if (autoRefresh) {
      interval = setInterval(fetchIntents, 30000); // Refresh every 30s
    }

    return () => {
      if (interval) clearInterval(interval);
    };
  }, [authTokens, autoRefresh]);

  // Handle intent created callback
  useEffect(() => {
    if (highlightIntentId) {
      // Refresh to show the new intent
      fetchIntents();
    }
  }, [highlightIntentId]);

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
      <Button variant="primary" onClick={() => navigate('/dashboard')}>
        Create Your First Plan
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
            <small className="text-muted">ID: {intent.id?.substring(0, 8)}...</small>
            <br />
            <Badge bg={getStatusVariant(intent.status)} className="mt-1">
              {intent.status}
            </Badge>
          </div>
          <small className="text-muted">
            {new Date(intent.created_at).toLocaleString()}
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
            View Details
          </Button>
          {intent.status === 'PENDING' && (
            <Button
              variant="outline-success"
              size="sm"
              onClick={() => handleViewDetails(intent.id)}
            >
              Validate
            </Button>
          )}
          {intent.status === 'VALIDATED' && (
            <Button
              variant="outline-success"
              size="sm"
              onClick={() => handleViewDetails(intent.id)}
            >
              Execute
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
            onClick={fetchIntents}
            disabled={isLoading}
          >
            {isLoading ? (
              <Spinner as="span" animation="border" size="sm" />
            ) : (
              'Refresh'
            )}
          </Button>
          <Button
            variant={autoRefresh ? 'primary' : 'outline-primary'}
            size="sm"
            onClick={() => setAutoRefresh(!autoRefresh)}
          >
            {autoRefresh ? 'Auto-refresh On' : 'Auto-refresh Off'}
          </Button>
        </div>
      </div>

      {error && (
        <Alert variant="danger" dismissible onClose={() => setError(null)}>
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
  onIntentCreated: PropTypes.func,
  highlightIntentId: PropTypes.string,
};

export default TradingIntentsList;

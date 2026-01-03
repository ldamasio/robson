import React, { useState, useEffect, useContext } from 'react';
import Button from 'react-bootstrap/Button';
import Col from 'react-bootstrap/Col';
import Container from 'react-bootstrap/Container';
import Modal from 'react-bootstrap/Modal';
import Row from 'react-bootstrap/Row';
import Form from 'react-bootstrap/Form';
import Alert from 'react-bootstrap/Alert';
import Spinner from 'react-bootstrap/Spinner';
import PropTypes from 'prop-types';
import AuthContext from '../../../context/AuthContext';

/**
 * StartNewOperationModal - Simplified one-click trading plan creation.
 *
 * User only selects:
 * 1. Trading pair (symbol)
 * 2. Strategy
 *
 * Backend automatically calculates:
 * - Entry price (current market price)
 * - Stop price (technical analysis)
 * - Capital (from strategy config)
 * - Side (from strategy market_bias)
 * - Position size (1% risk rule)
 *
 * @param {Object} props
 * @param {boolean} props.show - Whether modal is visible
 * @param {Function} props.onHide - Callback to close modal
 * @param {Function} props.onSuccess - Callback when intent is created successfully
 */
function StartNewOperationModal({ show, onHide, onSuccess }) {
  const { authTokens } = useContext(AuthContext);

  // Data from API
  const [symbols, setSymbols] = useState([]);
  const [strategies, setStrategies] = useState([]);
  const [loadingData, setLoadingData] = useState(false);

  // Form state
  const [formData, setFormData] = useState({
    symbol: '',
    strategy: '',
  });

  // UI state
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState(null);
  const [fieldErrors, setFieldErrors] = useState({});

  // Fetch symbols and strategies on mount
  useEffect(() => {
    const fetchData = async () => {
      if (!authTokens?.access) return;

      setLoadingData(true);
      try {
        // Fetch symbols
        const symbolsResponse = await fetch(
          `${import.meta.env.VITE_API_BASE_URL}/api/symbols/`,
          {
            headers: {
              'Content-Type': 'application/json',
              Authorization: `Bearer ${authTokens.access}`,
            },
          }
        );
        if (symbolsResponse.ok) {
          const symbolsData = await symbolsResponse.json();
          setSymbols(symbolsData.results || symbolsData);
        }

        // Fetch strategies
        const strategiesResponse = await fetch(
          `${import.meta.env.VITE_API_BASE_URL}/api/strategies/`,
          {
            headers: {
              'Content-Type': 'application/json',
              Authorization: `Bearer ${authTokens.access}`,
            },
          }
        );
        if (strategiesResponse.ok) {
          const strategiesData = await strategiesResponse.json();
          setStrategies(strategiesData.results || strategiesData);
        }
      } catch (err) {
        console.error('Failed to fetch data:', err);
      } finally {
        setLoadingData(false);
      }
    };

    if (show) {
      fetchData();
    }
  }, [authTokens, show]);

  // Handle field changes
  const handleFieldChange = (field, value) => {
    setFormData((prev) => ({ ...prev, [field]: value }));
    setError(null);
    setFieldErrors((prev) => ({ ...prev, [field]: null }));
  };

  // Validate form
  const validateForm = () => {
    const errors = {};
    const { symbol, strategy } = formData;

    if (!symbol) errors.symbol = 'Symbol is required';
    if (!strategy) errors.strategy = 'Strategy is required';

    setFieldErrors(errors);
    return Object.keys(errors).length === 0;
  };

  // Handle form submission
  const handleSubmit = async (e) => {
    e.preventDefault();

    // Validate
    if (!validateForm()) {
      setError('Please select both symbol and strategy.');
      return;
    }

    setIsSubmitting(true);
    setError(null);

    try {
      // Prepare payload - only symbol and strategy
      // Backend will auto-calculate all other parameters
      const payload = {
        symbol: parseInt(formData.symbol),
        strategy: parseInt(formData.strategy),
      };

      // Submit to API
      const response = await fetch(
        `${import.meta.env.VITE_API_BASE_URL}/api/trading-intents/create/`,
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
        throw new Error(errorData.error || errorData.detail || errorData.message || 'Failed to create trading intent');
      }

      const createdIntent = await response.json();

      // Success
      if (onSuccess) {
        onSuccess(createdIntent);
      }

      // Reset form
      setFormData({
        symbol: '',
        strategy: '',
      });

      // Close modal
      onHide();
    } catch (err) {
      console.error('Failed to create trading intent:', err);
      setError(err.message || 'An error occurred while creating the trading intent. Please try again.');
    } finally {
      setIsSubmitting(false);
    }
  };

  // Handle modal close
  const handleClose = () => {
    if (!isSubmitting) {
      setError(null);
      setFieldErrors({});
      onHide();
    }
  };

  return (
    <Modal show={show} onHide={handleClose} aria-labelledby="contained-modal-title-vcenter">
      <Modal.Header closeButton>
        <Modal.Title id="contained-modal-title-vcenter">Create Trading Plan</Modal.Title>
      </Modal.Header>
      <Modal.Body>
        <Container>
          {error && (
            <Alert variant="danger" dismissible onClose={() => setError(null)}>
              {error}
            </Alert>
          )}

          {/* Simplified explanation */}
          <Alert variant="info" className="mb-3">
            <strong>One-click plan creation</strong>
            <p className="mb-0 mt-2 small">
              Select symbol and strategy. Backend will automatically calculate entry, stop, capital, and position size
              using technical analysis and your strategy settings.
            </p>
          </Alert>

          <Form onSubmit={handleSubmit}>
            {/* Symbol Selection */}
            <Form.Group className="mb-3">
              <Form.Label>
                Trading Pair <span className="text-danger">*</span>
              </Form.Label>
              <Form.Select
                value={formData.symbol}
                onChange={(e) => handleFieldChange('symbol', e.target.value)}
                disabled={isSubmitting || loadingData}
                isInvalid={!!fieldErrors.symbol}
              >
                <option value="">Select trading pair...</option>
                {symbols.map((symbol) => (
                  <option key={symbol.id} value={symbol.id}>
                    {symbol.base_asset}/{symbol.quote_asset}
                  </option>
                ))}
              </Form.Select>
              {fieldErrors.symbol && (
                <Form.Control.Feedback type="invalid">{fieldErrors.symbol}</Form.Control.Feedback>
              )}
            </Form.Group>

            {/* Strategy Selection */}
            <Form.Group className="mb-3">
              <Form.Label>
                Strategy <span className="text-danger">*</span>
              </Form.Label>
              <Form.Select
                value={formData.strategy}
                onChange={(e) => handleFieldChange('strategy', e.target.value)}
                disabled={isSubmitting || loadingData}
                isInvalid={!!fieldErrors.strategy}
              >
                <option value="">Select strategy...</option>
                {strategies.map((strategy) => (
                  <option key={strategy.id} value={strategy.id}>
                    {strategy.name}
                  </option>
                ))}
              </Form.Select>
              {fieldErrors.strategy && (
                <Form.Control.Feedback type="invalid">{fieldErrors.strategy}</Form.Control.Feedback>
              )}
              <Form.Text className="text-muted">
                Strategy settings determine side, risk level, and capital allocation
              </Form.Text>
            </Form.Group>
          </Form>
        </Container>
      </Modal.Body>
      <Modal.Footer>
        <Button variant="secondary" onClick={handleClose} disabled={isSubmitting}>
          Cancel
        </Button>
        <Button variant="primary" onClick={handleSubmit} disabled={isSubmitting || loadingData}>
          {isSubmitting ? (
            <>
              <Spinner as="span" animation="border" size="sm" role="status" aria-hidden="true" className="me-2" />
              Creating Plan...
            </>
          ) : (
            'Create Plan'
          )}
        </Button>
      </Modal.Footer>
    </Modal>
  );
}

StartNewOperationModal.propTypes = {
  show: PropTypes.bool.isRequired,
  onHide: PropTypes.func.isRequired,
  onSuccess: PropTypes.func,
};

export default StartNewOperationModal;

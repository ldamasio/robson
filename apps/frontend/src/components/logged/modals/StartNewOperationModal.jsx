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
import DecimalInput from '../../shared/DecimalInput';

/**
 * StartNewOperationModal - Modal for creating new trading intents (PLAN step).
 *
 * This modal allows users to:
 * 1. Select trading pair (symbol)
 * 2. Choose strategy
 * 3. Specify side (BUY/SELL)
 * 4. Enter entry price and stop price (technical invalidation level)
 * 5. Enter capital amount
 *
 * The backend will calculate optimal position size using the 1% risk rule:
 * Position Size = (Capital × 1%) / |Entry Price - Stop Price|
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
    symbol_id: '',
    strategy_id: '',
    side: 'BUY',
    entry_price: '',
    stop_price: '',
    capital: '',
  });

  // UI state
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState(null);
  const [fieldErrors, setFieldErrors] = useState({});
  const [calculatedSize, setCalculatedSize] = useState(null);

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

  // Calculate position size preview (optional feature)
  useEffect(() => {
    const { entry_price, stop_price, capital } = formData;

    if (entry_price && stop_price && capital) {
      try {
        const entry = parseFloat(entry_price);
        const stop = parseFloat(stop_price);
        const cap = parseFloat(capital);

        if (!isNaN(entry) && !isNaN(stop) && !isNaN(cap) && entry !== stop) {
          const stopDistance = Math.abs(entry - stop);
          const maxRisk = cap * 0.01; // 1% risk
          const size = maxRisk / stopDistance;
          setCalculatedSize(size.toFixed(8));
        } else {
          setCalculatedSize(null);
        }
      } catch {
        setCalculatedSize(null);
      }
    } else {
      setCalculatedSize(null);
    }
  }, [formData.entry_price, formData.stop_price, formData.capital]);

  // Handle field changes
  const handleFieldChange = (field, value) => {
    setFormData((prev) => ({ ...prev, [field]: value }));
    setError(null);
    setFieldErrors((prev) => ({ ...prev, [field]: null }));
  };

  // Validate form
  const validateForm = () => {
    const errors = {};
    const { symbol_id, strategy_id, entry_price, stop_price, capital } = formData;

    if (!symbol_id) errors.symbol_id = 'Symbol is required';
    if (!strategy_id) errors.strategy_id = 'Strategy is required';
    if (!entry_price) {
      errors.entry_price = 'Entry price is required';
    } else if (parseFloat(entry_price) <= 0) {
      errors.entry_price = 'Entry price must be greater than 0';
    }
    if (!stop_price) {
      errors.stop_price = 'Stop price is required';
    } else if (parseFloat(stop_price) <= 0) {
      errors.stop_price = 'Stop price must be greater than 0';
    }
    if (!capital) {
      errors.capital = 'Capital is required';
    } else if (parseFloat(capital) <= 0) {
      errors.capital = 'Capital must be greater than 0';
    }

    // Entry price must not equal stop price
    if (entry_price && stop_price && parseFloat(entry_price) === parseFloat(stop_price)) {
      errors.stop_price = 'Stop price must be different from entry price';
    }

    setFieldErrors(errors);
    return Object.keys(errors).length === 0;
  };

  // Handle form submission
  const handleSubmit = async (e) => {
    e.preventDefault();

    // Validate
    if (!validateForm()) {
      setError('Please fix the errors above before submitting.');
      return;
    }

    setIsSubmitting(true);
    setError(null);

    try {
      // Prepare payload
      const payload = {
        symbol: parseInt(formData.symbol_id),
        strategy: parseInt(formData.strategy_id),
        side: formData.side,
        entry_price: formData.entry_price,
        stop_price: formData.stop_price,
        capital: formData.capital,
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
        throw new Error(errorData.detail || errorData.message || 'Failed to create trading intent');
      }

      const createdIntent = await response.json();

      // Success
      if (onSuccess) {
        onSuccess(createdIntent);
      }

      // Reset form
      setFormData({
        symbol_id: '',
        strategy_id: '',
        side: 'BUY',
        entry_price: '',
        stop_price: '',
        capital: '',
      });
      setCalculatedSize(null);

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

  // Get selected symbol for display
  const selectedSymbol = symbols.find((s) => s.id === parseInt(formData.symbol_id));
  const baseAsset = selectedSymbol?.base_asset || '';

  return (
    <Modal show={show} onHide={handleClose} aria-labelledby="contained-modal-title-vcenter" size="lg">
      <Modal.Header closeButton>
        <Modal.Title id="contained-modal-title-vcenter">Start New Operation</Modal.Title>
      </Modal.Header>
      <Modal.Body className="grid-example">
        <Container>
          {error && (
            <Alert variant="danger" dismissible onClose={() => setError(null)}>
              {error}
            </Alert>
          )}

          <Form onSubmit={handleSubmit}>
            {/* Symbol and Strategy Selection */}
            <Row>
              <Col xs={12} md={6}>
                <Form.Group className="mb-3">
                  <Form.Label>
                    Trading Pair <span className="text-danger">*</span>
                  </Form.Label>
                  <Form.Select
                    value={formData.symbol_id}
                    onChange={(e) => handleFieldChange('symbol_id', e.target.value)}
                    disabled={isSubmitting || loadingData}
                    isInvalid={!!fieldErrors.symbol_id}
                  >
                    <option value="">Select trading pair...</option>
                    {symbols.map((symbol) => (
                      <option key={symbol.id} value={symbol.id}>
                        {symbol.base_asset}/{symbol.quote_asset}
                      </option>
                    ))}
                  </Form.Select>
                  {fieldErrors.symbol_id && (
                    <Form.Control.Feedback type="invalid">{fieldErrors.symbol_id}</Form.Control.Feedback>
                  )}
                </Form.Group>
              </Col>

              <Col xs={12} md={6}>
                <Form.Group className="mb-3">
                  <Form.Label>
                    Strategy <span className="text-danger">*</span>
                  </Form.Label>
                  <Form.Select
                    value={formData.strategy_id}
                    onChange={(e) => handleFieldChange('strategy_id', e.target.value)}
                    disabled={isSubmitting || loadingData}
                    isInvalid={!!fieldErrors.strategy_id}
                  >
                    <option value="">Select strategy...</option>
                    {strategies.map((strategy) => (
                      <option key={strategy.id} value={strategy.id}>
                        {strategy.name}
                      </option>
                    ))}
                  </Form.Select>
                  {fieldErrors.strategy_id && (
                    <Form.Control.Feedback type="invalid">{fieldErrors.strategy_id}</Form.Control.Feedback>
                  )}
                </Form.Group>
              </Col>
            </Row>

            {/* Side Selection */}
            <Row>
              <Col xs={12}>
                <Form.Group className="mb-3">
                  <Form.Label>
                    Side <span className="text-danger">*</span>
                  </Form.Label>
                  <div>
                    <Form.Check
                      inline
                      type="radio"
                      label="BUY (Long)"
                      name="side"
                      id="side-buy"
                      value="BUY"
                      checked={formData.side === 'BUY'}
                      onChange={(e) => handleFieldChange('side', e.target.value)}
                      disabled={isSubmitting}
                    />
                    <Form.Check
                      inline
                      type="radio"
                      label="SELL (Short)"
                      name="side"
                      id="side-sell"
                      value="SELL"
                      checked={formData.side === 'SELL'}
                      onChange={(e) => handleFieldChange('side', e.target.value)}
                      disabled={isSubmitting}
                    />
                  </div>
                </Form.Group>
              </Col>
            </Row>

            {/* Price Inputs */}
            <Row>
              <Col xs={12} md={6}>
                <DecimalInput
                  label="Entry Price"
                  value={formData.entry_price}
                  onChange={(value) => handleFieldChange('entry_price', value)}
                  placeholder="0.00"
                  min="0.01"
                  step="0.01"
                  disabled={isSubmitting}
                  error={fieldErrors.entry_price}
                  helpText="Price at which you want to enter the position"
                  required
                />
              </Col>

              <Col xs={12} md={6}>
                <DecimalInput
                  label="Stop Price"
                  value={formData.stop_price}
                  onChange={(value) => handleFieldChange('stop_price', value)}
                  placeholder="0.00"
                  min="0.01"
                  step="0.01"
                  disabled={isSubmitting}
                  error={fieldErrors.stop_price}
                  helpText={
                    formData.side === 'BUY'
                      ? 'Technical invalidation level (2nd support level)'
                      : 'Technical invalidation level (2nd resistance level)'
                  }
                  required
                />
              </Col>
            </Row>

            {/* Capital Input */}
            <Row>
              <Col xs={12}>
                <DecimalInput
                  label="Capital"
                  value={formData.capital}
                  onChange={(value) => handleFieldChange('capital', value)}
                  placeholder="1000.00"
                  min="1"
                  step="1"
                  disabled={isSubmitting}
                  error={fieldErrors.capital}
                  helpText="Total capital to risk (position size will be calculated to risk 1%)"
                  required
                />
              </Col>
            </Row>

            {/* Calculated Position Size Preview */}
            {calculatedSize && (
              <Row>
                <Col xs={12}>
                  <Alert variant="info">
                    <strong>Calculated Position Size:</strong> {calculatedSize} {baseAsset}
                    <br />
                    <small>
                      Based on 1% risk rule: risking ${(parseFloat(formData.capital) * 0.01).toFixed(2)} on this
                      trade
                    </small>
                  </Alert>
                </Col>
              </Row>
            )}
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

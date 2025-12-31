/**
 * PatternConfigForm Component
 *
 * Form for creating or editing strategy-pattern configurations.
 */

import React, { useState, useEffect, useContext } from 'react';
import { Form, Button, Row, Col, Card, Alert } from 'react-bootstrap';
import { toast } from 'react-toastify';
import AuthContext from '../../../context/AuthContext';

const PatternConfigForm = ({ catalog, editConfig, onSave, onCancel }) => {
  const { authTokens } = useContext(AuthContext);

  // Form state
  const [formData, setFormData] = useState({
    strategy: '',
    pattern: '',
    auto_entry_enabled: false,
    min_confidence: 0.75,
    max_entries_per_day: 5,
    max_entries_per_week: 20,
    cooldown_minutes: 60,
    position_size_pct: null,
    require_confirmation: true,
    timeframes: [],
    symbols: [],
    require_volume_confirmation: false,
    only_with_trend: false,
    is_active: true,
  });

  const [strategies, setStrategies] = useState([]);
  const [loading, setLoading] = useState(false);
  const [errors, setErrors] = useState({});

  // Load strategies on mount
  useEffect(() => {
    const fetchStrategies = async () => {
      try {
        const response = await fetch(`${import.meta.env.VITE_API_BASE_URL}/api/strategies/`, {
          headers: {
            'Content-Type': 'application/json',
            'Authorization': `Bearer ${authTokens.access}`,
          },
        });
        if (response.ok) {
          const data = await response.json();
          setStrategies(data.results || data);
        }
      } catch (err) {
        console.error('Failed to fetch strategies:', err);
        toast.error('Failed to load strategies');
      }
    };
    fetchStrategies();
  }, [authTokens]);

  // Initialize form with edit data
  useEffect(() => {
    if (editConfig) {
      setFormData({
        strategy: editConfig.strategy,
        pattern: editConfig.pattern,
        auto_entry_enabled: editConfig.auto_entry_enabled || false,
        min_confidence: editConfig.min_confidence ?? 0.75,
        max_entries_per_day: editConfig.max_entries_per_day ?? 5,
        max_entries_per_week: editConfig.max_entries_per_week ?? 20,
        cooldown_minutes: editConfig.cooldown_minutes ?? 60,
        position_size_pct: editConfig.position_size_pct,
        require_confirmation: editConfig.require_confirmation ?? true,
        timeframes: editConfig.timeframes || [],
        symbols: editConfig.symbols || [],
        require_volume_confirmation: editConfig.require_volume_confirmation || false,
        only_with_trend: editConfig.only_with_trend || false,
        is_active: editConfig.is_active ?? true,
      });
    }
  }, [editConfig]);

  // Handle form field change
  const handleChange = (e) => {
    const { name, value, type, checked } = e.target;
    setFormData((prev) => ({
      ...prev,
      [name]: type === 'checkbox' ? checked : value,
    }));
    // Clear error for this field
    if (errors[name]) {
      setErrors((prev) => ({ ...prev, [name]: null }));
    }
  };

  // Handle multi-select change (timeframes, symbols)
  const handleMultiSelectChange = (name, value) => {
    setFormData((prev) => ({
      ...prev,
      [name]: value,
    }));
  };

  // Validate form
  const validateForm = () => {
    const newErrors = {};

    if (!formData.strategy) newErrors.strategy = 'Strategy is required';
    if (!formData.pattern) newErrors.pattern = 'Pattern is required';
    if (formData.min_confidence < 0 || formData.min_confidence > 1) {
      newErrors.min_confidence = 'Must be between 0 and 1';
    }
    if (formData.position_size_pct && (formData.position_size_pct <= 0 || formData.position_size_pct > 100)) {
      newErrors.position_size_pct = 'Must be between 0 and 100';
    }
    if (formData.timeframes.length === 0) {
      newErrors.timeframes = 'At least one timeframe is required';
    }
    if (formData.symbols.length === 0) {
      newErrors.symbols = 'At least one symbol is required';
    }

    setErrors(newErrors);
    return Object.keys(newErrors).length === 0;
  };

  // Handle submit
  const handleSubmit = async (e) => {
    e.preventDefault();

    if (!validateForm()) {
      toast.error('Please fix form errors');
      return;
    }

    setLoading(true);

    try {
      const payload = {
        strategy: parseInt(formData.strategy),
        pattern: parseInt(formData.pattern),
        auto_entry_enabled: formData.auto_entry_enabled,
        min_confidence: parseFloat(formData.min_confidence),
        max_entries_per_day: parseInt(formData.max_entries_per_day),
        max_entries_per_week: parseInt(formData.max_entries_per_week),
        cooldown_minutes: parseInt(formData.cooldown_minutes),
        position_size_pct: formData.position_size_pct ? parseFloat(formData.position_size_pct) : null,
        require_confirmation: formData.require_confirmation,
        timeframes: formData.timeframes,
        symbols: formData.symbols.map(s => s.toUpperCase().trim()),
        require_volume_confirmation: formData.require_volume_confirmation,
        only_with_trend: formData.only_with_trend,
        is_active: formData.is_active,
      };

      if (editConfig) {
        await onSave(editConfig.id, payload);
      } else {
        await onSave(payload);
      }
    } catch (err) {
      console.error('Form submission error:', err);
    } finally {
      setLoading(false);
    }
  };

  // Timeframe options
  const timeframeOptions = ['1m', '3m', '5m', '15m', '30m', '1h', '2h', '4h', '6h', '8h', '12h', '1d', '3d', '1w'];

  // Common symbol suggestions
  const commonSymbols = ['BTCUSDT', 'ETHUSDT', 'BNBUSDT', 'ADAUSDT', 'XRPUSDT', 'SOLUSDT', 'DOGEUSDT', 'MATICUSDT'];

  return (
    <Card>
      <Card.Header>
        <strong>{editConfig ? 'Edit Pattern Configuration' : 'Create Pattern Configuration'}</strong>
      </Card.Header>
      <Card.Body>
        <Form onSubmit={handleSubmit}>
          <Row>
            {/* Strategy Selection */}
            <Col md={6} className="mb-3">
              <Form.Group>
                <Form.Label>Strategy *</Form.Label>
                <Form.Select
                  name="strategy"
                  value={formData.strategy}
                  onChange={handleChange}
                  isInvalid={!!errors.strategy}
                >
                  <option value="">Select a strategy...</option>
                  {strategies.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name}
                    </option>
                  ))}
                </Form.Select>
                <Form.Control.Feedback type="invalid">{errors.strategy}</Form.Control.Feedback>
              </Form.Group>
            </Col>

            {/* Pattern Selection */}
            <Col md={6} className="mb-3">
              <Form.Group>
                <Form.Label>Pattern *</Form.Label>
                <Form.Select
                  name="pattern"
                  value={formData.pattern}
                  onChange={handleChange}
                  isInvalid={!!errors.pattern}
                >
                  <option value="">Select a pattern...</option>
                  {catalog.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name} ({p.pattern_code}) - {p.category_display}
                    </option>
                  ))}
                </Form.Select>
                <Form.Control.Feedback type="invalid">{errors.pattern}</Form.Control.Feedback>
              </Form.Group>
            </Col>
          </Row>

          {/* Auto Entry Settings */}
          <Row>
            <Col md={4} className="mb-3">
              <Form.Check
                type="switch"
                name="auto_entry_enabled"
                label="Enable Auto-Entry"
                checked={formData.auto_entry_enabled}
                onChange={handleChange}
              />
              <Form.Text className="text-muted">
                When enabled, patterns will automatically feed into the execution pipeline.
              </Form.Text>
            </Col>

            <Col md={4} className="mb-3">
              <Form.Check
                type="switch"
                name="require_confirmation"
                label="Require Confirmation"
                checked={formData.require_confirmation}
                onChange={handleChange}
              />
              <Form.Text className="text-muted">
                Only process patterns that have been confirmed.
              </Form.Text>
            </Col>

            <Col md={4} className="mb-3">
              <Form.Check
                type="switch"
                name="is_active"
                label="Active"
                checked={formData.is_active}
                onChange={handleChange}
              />
            </Col>
          </Row>

          <Row>
            {/* Minimum Confidence */}
            <Col md={4} className="mb-3">
              <Form.Group>
                <Form.Label>Min Confidence (0-1)</Form.Label>
                <Form.Control
                  type="number"
                  name="min_confidence"
                  step="0.01"
                  min="0"
                  max="1"
                  value={formData.min_confidence}
                  onChange={handleChange}
                  isInvalid={!!errors.min_confidence}
                />
                <Form.Control.Feedback type="invalid">{errors.min_confidence}</Form.Control.Feedback>
              </Form.Group>
            </Col>

            {/* Position Size % */}
            <Col md={4} className="mb-3">
              <Form.Group>
                <Form.Label>Position Size % (optional)</Form.Label>
                <Form.Control
                  type="number"
                  name="position_size_pct"
                  step="0.1"
                  min="0"
                  max="100"
                  placeholder="Default"
                  value={formData.position_size_pct || ''}
                  onChange={handleChange}
                  isInvalid={!!errors.position_size_pct}
                />
                <Form.Control.Feedback type="invalid">{errors.position_size_pct}</Form.Control.Feedback>
                <Form.Text className="text-muted">Leave empty for strategy default</Form.Text>
              </Form.Group>
            </Col>

            {/* Cooldown Minutes */}
            <Col md={4} className="mb-3">
              <Form.Group>
                <Form.Label>Cooldown (minutes)</Form.Label>
                <Form.Control
                  type="number"
                  name="cooldown_minutes"
                  min="0"
                  value={formData.cooldown_minutes}
                  onChange={handleChange}
                />
              </Form.Group>
            </Col>
          </Row>

          {/* Entry Limits */}
          <Row>
            <Col md={6} className="mb-3">
              <Form.Group>
                <Form.Label>Max Entries Per Day</Form.Label>
                <Form.Control
                  type="number"
                  name="max_entries_per_day"
                  min="0"
                  value={formData.max_entries_per_day}
                  onChange={handleChange}
                />
              </Form.Group>
            </Col>

            <Col md={6} className="mb-3">
              <Form.Group>
                <Form.Label>Max Entries Per Week</Form.Label>
                <Form.Control
                  type="number"
                  name="max_entries_per_week"
                  min="0"
                  value={formData.max_entries_per_week}
                  onChange={handleChange}
                />
              </Form.Group>
            </Col>
          </Row>

          {/* Trend Settings */}
          <Row>
            <Col md={6} className="mb-3">
              <Form.Check
                type="switch"
                name="require_volume_confirmation"
                label="Require Volume Confirmation"
                checked={formData.require_volume_confirmation}
                onChange={handleChange}
              />
              <Form.Text className="text-muted">
                Only process patterns with volume spike confirmation.
              </Form.Text>
            </Col>

            <Col md={6} className="mb-3">
              <Form.Check
                type="switch"
                name="only_with_trend"
                label="Only With Trend"
                checked={formData.only_with_trend}
                onChange={handleChange}
              />
              <Form.Text className="text-muted">
                Only trade patterns aligned with the prevailing trend.
              </Form.Text>
            </Col>
          </Row>

          {/* Timeframes Selection */}
          <Form.Group className="mb-3">
            <Form.Label>Timeframes *</Form.Label>
            <div className="border rounded p-2 bg-dark">
              {timeframeOptions.map((tf) => (
                <Form.Check
                  key={tf}
                  inline
                  type="checkbox"
                  label={tf}
                  value={tf}
                  checked={formData.timeframes.includes(tf)}
                  onChange={(e) => {
                    const newValue = e.target.checked
                      ? [...formData.timeframes, tf]
                      : formData.timeframes.filter((t) => t !== tf);
                    handleMultiSelectChange('timeframes', newValue);
                  }}
                />
              ))}
            </div>
            {errors.timeframes && <div className="text-danger small mt-1">{errors.timeframes}</div>}
          </Form.Group>

          {/* Symbols Selection */}
          <Form.Group className="mb-3">
            <Form.Label>Symbols *</Form.Label>
            <Form.Control
              as="textarea"
              rows={2}
              placeholder="Enter symbols separated by commas (e.g., BTCUSDT, ETHUSDT)"
              value={formData.symbols.join(', ')}
              onChange={(e) => {
                const symbols = e.target.value
                  .split(',')
                  .map((s) => s.trim().toUpperCase())
                  .filter((s) => s.length > 0);
                handleMultiSelectChange('symbols', symbols);
              }}
              isInvalid={!!errors.symbols}
            />
            <Form.Control.Feedback type="invalid">{errors.symbols}</Form.Control.Feedback>
            <Form.Text className="text-muted">
              Common: {commonSymbols.join(', ')}
            </Form.Text>
          </Form.Group>

          {/* Action Buttons */}
          <div className="d-flex justify-content-end gap-2">
            <Button variant="secondary" onClick={onCancel} disabled={loading}>
              Cancel
            </Button>
            <Button variant="primary" type="submit" disabled={loading}>
              {loading ? 'Saving...' : editConfig ? 'Update Configuration' : 'Create Configuration'}
            </Button>
          </div>
        </Form>
      </Card.Body>
    </Card>
  );
};

export default PatternConfigForm;

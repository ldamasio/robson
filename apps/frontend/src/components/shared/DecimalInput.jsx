import React from 'react';
import PropTypes from 'prop-types';
import { Form } from 'react-bootstrap';

/**
 * DecimalInput component for handling decimal number inputs with validation.
 * Designed for financial inputs like prices and capital amounts.
 *
 * Features:
 * - Validates decimal format (no scientific notation)
 * - Limits to max 8 decimal places
 * - Shows error states with Bootstrap styling
 * - Prevents invalid characters during input
 *
 * @param {Object} props
 * @param {string} props.value - Current input value (as string to preserve decimal precision)
 * @param {Function} props.onChange - Callback when value changes: (newValue: string) => void
 * @param {string} props.label - Field label
 * @param {string} [props.placeholder] - Placeholder text
 * @param {string} [props.min] - Minimum value
 * @param {string} [props.max] - Maximum value
 * @param {string} [props.step] - Step value for increment buttons
 * @param {boolean} [props.disabled] - Whether input is disabled
 * @param {string} [props.error] - Error message to display
 * @param {string} [props.helpText] - Help text to display below input
 * @param {boolean} [props.required] - Whether field is required
 */
const DecimalInput = ({
  value,
  onChange,
  label,
  placeholder = '',
  min = '0',
  max,
  step = '0.01',
  disabled = false,
  error = null,
  helpText = null,
  required = false,
}) => {
  /**
   * Validates decimal input:
   * - Allows only numbers, one decimal point, and leading minus
   * - Limits to 8 decimal places
   * - No scientific notation
   */
  const validateDecimalInput = (inputValue) => {
    // Allow empty string
    if (inputValue === '') return true;

    // Allow single minus at start
    if (inputValue === '-') return true;

    // Check for valid decimal format (no scientific notation)
    const decimalRegex = /^-?\d*\.?\d*$/;
    if (!decimalRegex.test(inputValue)) return false;

    // Check decimal places (max 8)
    const decimalParts = inputValue.split('.');
    if (decimalParts.length > 2) return false;
    if (decimalParts[1] && decimalParts[1].length > 8) return false;

    return true;
  };

  const handleChange = (e) => {
    const newValue = e.target.value;

    // Validate before allowing change
    if (validateDecimalInput(newValue)) {
      onChange(newValue);
    }
  };

  const handleBlur = (e) => {
    const currentValue = e.target.value;

    // Clean up trailing decimal point
    if (currentValue.endsWith('.')) {
      onChange(currentValue.slice(0, -1));
    }

    // Clean up leading zeros (except "0" or "0.xxx")
    if (currentValue.startsWith('0') && currentValue.length > 1 && !currentValue.startsWith('0.')) {
      onChange(parseFloat(currentValue).toString());
    }
  };

  return (
    <Form.Group className="mb-3">
      <Form.Label>
        {label}
        {required && <span className="text-danger"> *</span>}
      </Form.Label>
      <Form.Control
        type="text"
        value={value}
        onChange={handleChange}
        onBlur={handleBlur}
        placeholder={placeholder}
        disabled={disabled}
        isInvalid={!!error}
        min={min}
        max={max}
        step={step}
        inputMode="decimal"
        autoComplete="off"
      />
      {error && (
        <Form.Control.Feedback type="invalid">
          {error}
        </Form.Control.Feedback>
      )}
      {helpText && !error && (
        <Form.Text className="text-muted">
          {helpText}
        </Form.Text>
      )}
    </Form.Group>
  );
};

DecimalInput.propTypes = {
  value: PropTypes.string.isRequired,
  onChange: PropTypes.func.isRequired,
  label: PropTypes.string.isRequired,
  placeholder: PropTypes.string,
  min: PropTypes.string,
  max: PropTypes.string,
  step: PropTypes.string,
  disabled: PropTypes.bool,
  error: PropTypes.string,
  helpText: PropTypes.string,
  required: PropTypes.bool,
};

export default DecimalInput;

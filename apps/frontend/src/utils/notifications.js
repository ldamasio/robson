import { toast } from 'react-toastify';

/**
 * Toast notification helper utilities.
 *
 * Provides consistent, user-friendly notifications for all user actions.
 *
 * Usage patterns:
 * - Basic notifications (showSuccess, showError, showInfo, showWarning): Used for
 *   immediate feedback on user actions (manual and auto-trigger flows)
 *
 * - Async flow notifications (showLoading, updateLoadingToSuccess, updateLoadingToError):
 *   Used by auto-trigger async flows where validation/execution happens in background
 *   Example: Pattern detected → "Validating..." → "Validation passed" → "Executing..."
 *
 * - Dismissal utilities (dismissToast, dismissAllToasts): Used for cleanup
 */

/**
 * Show success notification (green toast with checkmark).
 *
 * Used by: Manual actions (create, validate, execute), auto-trigger success states
 *
 * @param {string} message - Success message to display
 * @param {Object} options - Additional toast options
 */
export const showSuccess = (message, options = {}) => {
  return toast.success(message, {
    position: 'top-right',
    autoClose: 5000,
    hideProgressBar: false,
    closeOnClick: true,
    pauseOnHover: true,
    draggable: true,
    ...options,
  });
};

/**
 * Show error notification (red toast with error icon).
 *
 * Used by: Validation failures, execution failures, API errors
 * Note: Errors don't auto-dismiss (user must acknowledge)
 *
 * @param {string} message - Error message to display
 * @param {Object} options - Additional toast options
 */
export const showError = (message, options = {}) => {
  return toast.error(message, {
    position: 'top-right',
    autoClose: false, // Errors don't auto-dismiss
    hideProgressBar: false,
    closeOnClick: true,
    pauseOnHover: true,
    draggable: true,
    ...options,
  });
};

/**
 * Show info notification (blue toast with info icon).
 *
 * Used by: Dry-run completions, cancellations, informational updates
 *
 * @param {string} message - Info message to display
 * @param {Object} options - Additional toast options
 */
export const showInfo = (message, options = {}) => {
  return toast.info(message, {
    position: 'top-right',
    autoClose: 5000,
    hideProgressBar: false,
    closeOnClick: true,
    pauseOnHover: true,
    draggable: true,
    ...options,
  });
};

/**
 * Show warning notification (yellow toast with warning icon).
 *
 * Used by: LIVE mode warnings, validation warnings, guard alerts
 * Note: Warnings stay longer (7s) for visibility
 *
 * @param {string} message - Warning message to display
 * @param {Object} options - Additional toast options
 */
export const showWarning = (message, options = {}) => {
  return toast.warn(message, {
    position: 'top-right',
    autoClose: 7000, // Warnings stay longer
    hideProgressBar: false,
    closeOnClick: true,
    pauseOnHover: true,
    draggable: true,
    ...options,
  });
};

/**
 * Show loading notification (toast with spinner).
 * Returns a toast ID that can be used to update or dismiss the loading toast.
 *
 * Used by: Auto-trigger async flows (validation, execution)
 * Example: Pattern detected → showLoading("Validating pattern...") → update to success/error
 *
 * @param {string} message - Loading message to display
 * @returns {string} Toast ID for later update/dismiss
 */
export const showLoading = (message) => {
  return toast.loading(message, {
    position: 'top-right',
    closeOnClick: false,
    pauseOnHover: false,
    draggable: false,
  });
};

/**
 * Update an existing loading toast to success state.
 *
 * Used by: Auto-trigger async flows after successful completion
 * Example: "Validating pattern..." → "Pattern validated successfully!"
 *
 * @param {string} toastId - The toast ID returned from showLoading
 * @param {string} message - Success message
 * @param {Object} options - Additional toast options
 */
export const updateLoadingToSuccess = (toastId, message, options = {}) => {
  return toast.update(toastId, {
    render: message,
    type: 'success',
    isLoading: false,
    autoClose: 5000,
    hideProgressBar: false,
    closeOnClick: true,
    pauseOnHover: true,
    draggable: true,
    ...options,
  });
};

/**
 * Update an existing loading toast to error state.
 *
 * Used by: Auto-trigger async flows after failure
 * Example: "Validating pattern..." → "Validation failed: Insufficient balance"
 *
 * @param {string} toastId - The toast ID returned from showLoading
 * @param {string} message - Error message
 * @param {Object} options - Additional toast options
 */
export const updateLoadingToError = (toastId, message, options = {}) => {
  return toast.update(toastId, {
    render: message,
    type: 'error',
    isLoading: false,
    autoClose: false,
    hideProgressBar: false,
    closeOnClick: true,
    pauseOnHover: true,
    draggable: true,
    ...options,
  });
};

/**
 * Dismiss a specific toast.
 *
 * Used by: Cleanup after user action cancellation
 *
 * @param {string} toastId - The toast ID to dismiss
 */
export const dismissToast = (toastId) => {
  toast.dismiss(toastId);
};

/**
 * Dismiss all active toasts.
 *
 * Used by: Bulk cleanup, logout, session reset
 */
export const dismissAllToasts = () => {
  toast.dismiss();
};

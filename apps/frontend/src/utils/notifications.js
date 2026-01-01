import { toast } from 'react-toastify';

/**
 * Toast notification helper utilities.
 *
 * Provides consistent, user-friendly notifications for all user actions.
 */

/**
 * Show success notification (green toast with checkmark).
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
 * @param {string} toastId - The toast ID to dismiss
 */
export const dismissToast = (toastId) => {
  toast.dismiss(toastId);
};

/**
 * Dismiss all active toasts.
 */
export const dismissAllToasts = () => {
  toast.dismiss();
};

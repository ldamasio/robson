import { useState, useEffect, useCallback, useRef } from 'react';

/**
 * Custom hook for fetching and polling TradingIntent status.
 *
 * This hook provides real-time updates for trading intents with automatic polling
 * during transitional states (PENDING, EXECUTING).
 *
 * @param {string} intentId - The trading intent ID to fetch
 * @param {Object} authToken - Auth token object with access property
 * @param {Object} options - Configuration options
 * @param {number} options.pollingInterval - Polling interval in milliseconds (default: 5000)
 * @param {boolean} options.enablePolling - Enable/disable polling (default: true)
 * @returns {Object} { intent, isLoading, error, refetch, isPolling }
 */
export const useTradingIntent = (intentId, authToken, options = {}) => {
  const {
    pollingInterval = 5000,
    enablePolling = true,
  } = options;

  const [intent, setIntent] = useState(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState(null);
  const [isPolling, setIsPolling] = useState(false);

  // Use ref to track polling interval
  const pollingRef = useRef(null);
  const abortControllerRef = useRef(null);

  /**
   * Fetch trading intent from API
   */
  const fetchIntent = useCallback(async () => {
    if (!intentId || !authToken?.access) {
      return;
    }

    // Cancel any previous request
    if (abortControllerRef.current) {
      abortControllerRef.current.abort();
    }

    // Create new abort controller for this request
    abortControllerRef.current = new AbortController();

    try {
      const response = await fetch(
        `${import.meta.env.VITE_API_BASE_URL}/api/trading-intents/${intentId}/`,
        {
          headers: {
            'Content-Type': 'application/json',
            Authorization: `Bearer ${authToken.access}`,
          },
          signal: abortControllerRef.current.signal,
        }
      );

      if (!response.ok) {
        if (response.status === 404) {
          throw new Error('Trading intent not found');
        } else if (response.status === 401) {
          throw new Error('Unauthorized. Please log in again.');
        } else {
          throw new Error(`Failed to fetch trading intent: ${response.statusText}`);
        }
      }

      const data = await response.json();
      setIntent(data);
      setError(null);
    } catch (err) {
      // Ignore abort errors
      if (err.name === 'AbortError') {
        return;
      }
      console.error('Failed to fetch trading intent:', err);
      setError(err.message);
    } finally {
      setIsLoading(false);
    }
  }, [intentId, authToken]);

  /**
   * Manual refetch function
   */
  const refetch = useCallback(() => {
    setIsLoading(true);
    return fetchIntent();
  }, [fetchIntent]);

  /**
   * Setup polling for transitional states
   */
  useEffect(() => {
    if (!enablePolling || !intentId || !authToken?.access) {
      return;
    }

    // Determine if we should poll based on status
    const shouldPoll = intent?.status === 'PENDING' ||
                       intent?.status === 'EXECUTING' ||
                       (!intent && isLoading);

    if (shouldPoll) {
      setIsPolling(true);

      // Set up polling interval
      pollingRef.current = setInterval(() => {
        fetchIntent();
      }, pollingInterval);

      // Initial fetch
      if (!intent) {
        fetchIntent();
      }
    } else {
      // Stop polling if status is no longer transitional
      setIsPolling(false);
      if (pollingRef.current) {
        clearInterval(pollingRef.current);
        pollingRef.current = null;
      }
    }

    // Cleanup on unmount or when conditions change
    return () => {
      if (pollingRef.current) {
        clearInterval(pollingRef.current);
        pollingRef.current = null;
      }
      if (abortControllerRef.current) {
        abortControllerRef.current.abort();
      }
    };
  }, [intent?.status, enablePolling, intentId, authToken, pollingInterval, isLoading, intent, fetchIntent]);

  return {
    intent,
    isLoading,
    error,
    refetch,
    isPolling,
  };
};

export default useTradingIntent;

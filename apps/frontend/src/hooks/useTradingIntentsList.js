import { useState, useEffect, useCallback, useRef } from 'react';
import { listTradingIntents } from '../services/tradingIntents';

/**
 * Custom hook for fetching and auto-refreshing trading intents list.
 *
 * Handles data fetching, error handling, loading states, and auto-refresh
 * with proper cleanup on unmount.
 *
 * @param {Object} options - Hook options
 * @param {string} options.authToken - JWT access token
 * @param {string} [options.baseUrl] - API base URL (defaults to VITE_API_BASE_URL)
 * @param {number} [options.limit=10] - Max number of results
 * @param {string[]} [options.statuses] - Filter by statuses
 * @param {number} [options.refreshInterval=30000] - Auto-refresh interval in ms (0 to disable)
 * @param {boolean} [options.enableAutoRefresh=true] - Whether to enable auto-refresh
 * @returns {Object} Hook state and methods
 */
export const useTradingIntentsList = ({
  authToken,
  baseUrl = import.meta.env.VITE_API_BASE_URL,
  limit = 10,
  statuses,
  refreshInterval = 30000,
  enableAutoRefresh = true,
}) => {
  const [intents, setIntents] = useState([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState(null);
  const [autoRefreshEnabled, setAutoRefreshEnabled] = useState(enableAutoRefresh);

  // Use ref to track if component is mounted (prevent state updates after unmount)
  const isMountedRef = useRef(true);
  const intervalRef = useRef(null);

  /**
   * Fetch trading intents from API.
   */
  const fetchIntents = useCallback(async () => {
    if (!authToken) {
      setIsLoading(false);
      return;
    }

    try {
      setError(null);
      const data = await listTradingIntents({
        baseUrl,
        token: authToken,
        limit,
        statuses,
      });

      if (isMountedRef.current) {
        setIntents(data.results || data);
      }
    } catch (err) {
      console.error('Failed to fetch trading intents:', err);
      if (isMountedRef.current) {
        setError(err.message);
        // Disable auto-refresh if endpoint doesn't exist (404)
        if (err.message?.includes('404') || err.message?.includes('Not Found')) {
          console.warn('Endpoint not found (404), disabling auto-refresh');
          setAutoRefreshEnabled(false);
        }
      }
    } finally {
      if (isMountedRef.current) {
        setIsLoading(false);
      }
    }
  }, [authToken, baseUrl, limit, statuses]);

  /**
   * Manually trigger a refresh.
   */
  const refetch = useCallback(() => {
    setIsLoading(true);
    fetchIntents();
  }, [fetchIntents]);

  /**
   * Toggle auto-refresh on/off.
   */
  const toggleAutoRefresh = useCallback(() => {
    setAutoRefreshEnabled((prev) => !prev);
  }, []);

  // Initial fetch
  useEffect(() => {
    fetchIntents();
  }, [fetchIntents]);

  // Auto-refresh with proper cleanup
  useEffect(() => {
    if (!autoRefreshEnabled || refreshInterval === 0) {
      return;
    }

    intervalRef.current = setInterval(() => {
      fetchIntents();
    }, refreshInterval);

    return () => {
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
    };
  }, [autoRefreshEnabled, refreshInterval, fetchIntents]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      isMountedRef.current = false;
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
      }
    };
  }, []);

  return {
    intents,
    isLoading,
    error,
    refetch,
    autoRefreshEnabled,
    toggleAutoRefresh,
  };
};

/**
 * Trading Intents API service.
 *
 * Provides a clean interface for fetching trading intents with consistent
 * error handling and URL parameter building.
 */

/**
 * Build URL query parameters from an object.
 * Handles array parameters (e.g., status=[PENDING, VALIDATED]) correctly.
 *
 * @param {Object} params - Query parameters
 * @returns {string} URL-encoded query string
 */
const buildQueryParams = (params) => {
  const searchParams = new URLSearchParams();

  Object.entries(params).forEach(([key, value]) => {
    if (value === undefined || value === null) return;

    if (Array.isArray(value)) {
      value.forEach((v) => searchParams.append(key, v));
    } else {
      searchParams.append(key, value);
    }
  });

  return searchParams.toString();
};

/**
 * List trading intents with optional filtering.
 *
 * Note: Backend currently only supports single status filter, so for multiple
 * statuses we fetch all and filter client-side (temporary pragmatic solution).
 *
 * @param {Object} options - Fetch options
 * @param {string} options.baseUrl - API base URL
 * @param {string} options.token - Auth bearer token
 * @param {number} [options.limit=10] - Max number of results
 * @param {string[]} [options.statuses] - Filter by statuses (e.g., ['PENDING', 'VALIDATED'])
 * @returns {Promise<Object>} Response with results array
 * @throws {Error} If fetch fails or returns non-OK status
 */
export const listTradingIntents = async ({
  baseUrl,
  token,
  limit = 10,
  statuses,
}) => {
  if (!baseUrl) {
    throw new Error('baseUrl is required');
  }
  if (!token) {
    throw new Error('token is required');
  }

  // Backend only supports single status filter, so fetch with larger limit
  // and filter client-side if multiple statuses requested
  const shouldFilterClientSide = statuses && statuses.length > 1;
  const fetchLimit = shouldFilterClientSide ? 100 : limit;

  const params = { limit: fetchLimit };
  if (statuses && statuses.length === 1) {
    params.status = statuses[0];
  }

  const queryString = buildQueryParams(params);
  const url = `${baseUrl}/api/trading-intents/${queryString ? `?${queryString}` : ''}`;

  const response = await fetch(url, {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
  });

  if (!response.ok) {
    const errorData = await response.json().catch(() => ({}));
    throw new Error(errorData.detail || `Failed to fetch trading intents (${response.status})`);
  }

  const data = await response.json();

  // Client-side filtering for multiple statuses
  if (shouldFilterClientSide && data.results) {
    const statusSet = new Set(statuses);
    const filteredResults = data.results.filter((intent) => statusSet.has(intent.status));
    return {
      ...data,
      results: filteredResults.slice(0, limit),
      count: filteredResults.length,
    };
  }

  // Apply limit if backend returns more than requested
  if (data.results && data.results.length > limit) {
    return {
      ...data,
      results: data.results.slice(0, limit),
    };
  }

  return data;
};

/**
 * Get a single trading intent by ID.
 *
 * @param {Object} options - Fetch options
 * @param {string} options.baseUrl - API base URL
 * @param {string} options.token - Auth bearer token
 * @param {string} options.intentId - Trading intent ID
 * @returns {Promise<Object>} Trading intent object
 * @throws {Error} If fetch fails or returns non-OK status
 */
export const getTradingIntent = async ({
  baseUrl,
  token,
  intentId,
}) => {
  if (!baseUrl) {
    throw new Error('baseUrl is required');
  }
  if (!token) {
    throw new Error('token is required');
  }
  if (!intentId) {
    throw new Error('intentId is required');
  }

  const url = `${baseUrl}/api/trading-intents/${intentId}/`;

  const response = await fetch(url, {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
  });

  if (!response.ok) {
    if (response.status === 404) {
      throw new Error('Trading intent not found');
    }
    const errorData = await response.json().catch(() => ({}));
    throw new Error(errorData.detail || `Failed to fetch trading intent (${response.status})`);
  }

  return response.json();
};

/**
 * HTTP Adapter for Pattern Detection Engine API.
 *
 * Implements the PatternService port using fetch API.
 * All methods require authentication via Bearer token.
 */

import { PatternService } from '../../ports/PatternService';

export class PatternHttp extends PatternService {
  /**
   * @param {Object} options
   * @param {string} options.baseUrl - API base URL
   * @param {Function} options.getAuthToken - Function that returns auth token
   */
  constructor({ baseUrl, getAuthToken }) {
    super();
    this.baseUrl = baseUrl;
    this.getAuthToken = getAuthToken || (() => null);
  }

  /**
   * Make authenticated API request.
   * @private
   */
  async _request(endpoint, options = {}) {
    const token = this.getAuthToken();
    const url = `${this.baseUrl}${endpoint}`;

    const headers = {
      'Content-Type': 'application/json',
      ...(token ? { 'Authorization': `Bearer ${token}` } : {}),
      ...options.headers,
    };

    const response = await fetch(url, { ...options, headers });

    if (!response.ok) {
      const errorData = await response.json().catch(() => ({}));
      throw new Error(
        errorData.error || errorData.detail || `HTTP ${response.status}`
      );
    }

    // Handle 204 NO CONTENT responses (no body to parse)
    if (response.status === 204) {
      return null;
    }

    return response.json();
  }

  /**
   * Get pattern catalog (all available patterns).
   * GET /api/patterns/catalog/
   */
  async getCatalog() {
    return this._request('/api/patterns/catalog/');
  }

  /**
   * Get pattern dashboard summary.
   * GET /api/patterns/dashboard/
   */
  async getDashboard() {
    return this._request('/api/patterns/dashboard/');
  }

  /**
   * Get recent confirmed alerts.
   * GET /api/patterns/alerts/recent-confirms/
   * @param {Object} options
   * @param {number} options.hours - Time window in hours (default: 6)
   * @param {string} options.symbol - Filter by symbol (optional)
   * @param {string} options.patternCode - Filter by pattern code (optional)
   */
  async getRecentConfirms({ hours = 6, symbol, patternCode } = {}) {
    const params = new URLSearchParams({ hours: String(hours) });
    if (symbol) params.append('symbol', symbol);
    if (patternCode) params.append('pattern_code', patternCode);
    return this._request(`/api/patterns/alerts/recent-confirms/?${params}`);
  }

  /**
   * List pattern instances with filters.
   * GET /api/patterns/instances/
   * @param {Object} options
   * @param {string} options.symbol - Filter by symbol
   * @param {string} options.timeframe - Filter by timeframe
   * @param {string} options.status - Filter by status
   * @param {string} options.patternCode - Filter by pattern code
   * @param {number} options.limit - Max results (default: 50)
   */
  async getInstances({ symbol, timeframe, status, patternCode, limit = 50 } = {}) {
    const params = new URLSearchParams({ limit: String(limit) });
    if (symbol) params.append('symbol', symbol);
    if (timeframe) params.append('timeframe', timeframe);
    if (status) params.append('status', status);
    if (patternCode) params.append('pattern_code', patternCode);
    return this._request(`/api/patterns/instances/?${params}`);
  }

  /**
   * List pattern alerts with filters.
   * GET /api/patterns/alerts/
   * @param {Object} options
   * @param {string} options.symbol - Filter by symbol
   * @param {string} options.timeframe - Filter by timeframe
   * @param {string} options.alertType - Filter by alert type
   * @param {number} options.hours - Time window in hours (default: 24)
   * @param {number} options.limit - Max results (default: 50)
   */
  async getAlerts({ symbol, timeframe, alertType, hours = 24, limit = 50 } = {}) {
    const params = new URLSearchParams({ hours: String(hours), limit: String(limit) });
    if (symbol) params.append('symbol', symbol);
    if (timeframe) params.append('timeframe', timeframe);
    if (alertType) params.append('alert_type', alertType);
    return this._request(`/api/patterns/alerts/?${params}`);
  }

  /**
   * Get strategy pattern configurations.
   * GET /api/patterns/configs/
   */
  async getConfigs() {
    return this._request('/api/patterns/configs/');
  }

  /**
   * Create strategy pattern configuration.
   * POST /api/patterns/configs/create/
   * @param {Object} configData
   * @param {number} configData.strategy - Strategy ID
   * @param {number} configData.pattern - Pattern ID
   * @param {boolean} configData.auto_entry_enabled - Auto-entry enabled flag
   * @param {number} configData.min_confidence - Minimum confidence (0-1)
   * @param {string[]} configData.timeframes - Timeframes to scan
   * @param {string[]} configData.symbols - Symbols to scan
   * @param {boolean} configData.require_volume_confirmation - Require volume confirmation
   * @param {boolean} configData.only_with_trend - Only trade with trend
   */
  async createConfig(configData) {
    return this._request('/api/patterns/configs/create/', {
      method: 'POST',
      body: JSON.stringify(configData),
    });
  }

  /**
   * Update strategy pattern configuration.
   * PUT /api/patterns/configs/{id}/
   * @param {number} configId - Configuration ID
   * @param {Object} configData - Partial config data to update
   */
  async updateConfig(configId, configData) {
    return this._request(`/api/patterns/configs/${configId}/`, {
      method: 'PUT',
      body: JSON.stringify(configData),
    });
  }

  /**
   * Delete strategy pattern configuration.
   * DELETE /api/patterns/configs/{id}/
   * @param {number} configId - Configuration ID
   */
  async deleteConfig(configId) {
    return this._request(`/api/patterns/configs/${configId}/`, {
      method: 'DELETE',
    });
  }

  /**
   * Trigger pattern scan.
   * POST /api/patterns/scan/
   * @param {Object} options
   * @param {string} options.symbols - Comma-separated symbols (e.g., "BTCUSDT,ETHUSDT")
   * @param {string} options.timeframes - Comma-separated timeframes (e.g., "15m,1h")
   * @param {boolean} options.allDetectors - Use all detectors (default: true)
   */
  async triggerScan({ symbols = 'BTCUSDT', timeframes = '15m,1h', allDetectors = true } = {}) {
    return this._request('/api/patterns/scan/', {
      method: 'POST',
      body: JSON.stringify({
        symbols,
        timeframes,
        all_detectors: allDetectors,
      }),
    });
  }

  /**
   * Process confirmed pattern to trading plan.
   * POST /api/patterns/to-plan/
   * @param {Object} options
   * @param {number} options.patternInstanceId - Pattern instance ID
   * @param {boolean} options.forceCreate - Force plan creation
   */
  async patternToPlan({ patternInstanceId, forceCreate = false }) {
    return this._request('/api/patterns/to-plan/', {
      method: 'POST',
      body: JSON.stringify({
        pattern_instance_id: patternInstanceId,
        force_create: forceCreate,
      }),
    });
  }
}

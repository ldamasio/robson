import { MarginService } from '../../ports/MarginService'

/**
 * HTTP Adapter for Margin Trading API
 * 
 * Implements MarginService port using REST API calls.
 */
export class MarginHttp extends MarginService {
  constructor({ baseUrl, getAuthToken }) {
    super()
    // Use provided baseUrl, or fallback to VITE_API_BASE_URL
    this.baseUrl = baseUrl || import.meta.env.VITE_API_BASE_URL || ''
    this.getAuthToken = getAuthToken || (() => null)
  }

  _headers() {
    const token = this.getAuthToken()
    return {
      'Content-Type': 'application/json',
      ...(token ? { 'Authorization': `Bearer ${token}` } : {}),
    }
  }

  async _fetch(url, options = {}) {
    const res = await fetch(`${this.baseUrl}${url}`, {
      ...options,
      headers: this._headers(),
    })
    const data = await res.json()
    if (!res.ok) {
      throw new Error(data.error || `HTTP ${res.status}`)
    }
    return data
  }

  async getMarginAccount(symbol) {
    return this._fetch(`/api/margin/account/${symbol}/`)
  }

  async transferToMargin(symbol, asset, amount) {
    return this._fetch('/api/margin/transfer/to/', {
      method: 'POST',
      body: JSON.stringify({ symbol, asset, amount }),
    })
  }

  async transferFromMargin(symbol, asset, amount) {
    return this._fetch('/api/margin/transfer/from/', {
      method: 'POST',
      body: JSON.stringify({ symbol, asset, amount }),
    })
  }

  async calculatePositionSize(params) {
    return this._fetch('/api/margin/position/calculate/', {
      method: 'POST',
      body: JSON.stringify(params),
    })
  }

  async openPosition(params) {
    return this._fetch('/api/margin/position/open/', {
      method: 'POST',
      body: JSON.stringify(params),
    })
  }

  async closePosition(positionId, params = {}) {
    return this._fetch(`/api/margin/position/${positionId}/close/`, {
      method: 'POST',
      body: JSON.stringify(params),
    })
  }

  async listPositions(filters = {}) {
    const queryParams = new URLSearchParams(filters).toString()
    const url = queryParams 
      ? `/api/margin/positions/?${queryParams}`
      : '/api/margin/positions/'
    return this._fetch(url)
  }

  async getPosition(positionId) {
    return this._fetch(`/api/margin/positions/${positionId}/`)
  }

  async monitorMargins() {
    return this._fetch('/api/margin/monitor/')
  }
}


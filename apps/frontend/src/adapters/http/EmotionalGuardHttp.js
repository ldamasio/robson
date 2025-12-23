import { EmotionalGuardService } from '../../ports/EmotionalGuardService'

/**
 * HTTP Adapter for Emotional Trading Guard API
 * 
 * Implements EmotionalGuardService port using REST API calls.
 */
export class EmotionalGuardHttp extends EmotionalGuardService {
  constructor({ baseUrl, getAuthToken }) {
    super()
    this.baseUrl = baseUrl
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

  async analyzeIntent(message) {
    return this._fetch('/api/guard/analyze/', {
      method: 'POST',
      body: JSON.stringify({ message }),
    })
  }

  async listSignals() {
    return this._fetch('/api/guard/signals/')
  }

  async getTips(random = false, category = null) {
    const params = new URLSearchParams()
    if (random) params.set('random', 'true')
    if (category) params.set('category', category)
    const query = params.toString()
    return this._fetch(`/api/guard/tips/${query ? '?' + query : ''}`)
  }

  async getRiskLevels() {
    return this._fetch('/api/guard/risk-levels/')
  }
}


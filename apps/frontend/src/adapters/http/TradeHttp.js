import { TradeService } from '../../ports/TradeService'

export class TradeHttp extends TradeService {
  constructor({ baseUrl, getAuthToken }) {
    super()
    this.baseUrl = baseUrl
    this.getAuthToken = getAuthToken || (() => null)
  }

  async getStrategies() {
    const token = this.getAuthToken()
    const res = await fetch(`${this.baseUrl}/api/strategies/`, {
      method: 'GET',
      headers: {
        'Content-Type': 'application/json',
        ...(token ? { 'Authorization': `Bearer ${token}` } : {}),
      }
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    return await res.json()
  }
}


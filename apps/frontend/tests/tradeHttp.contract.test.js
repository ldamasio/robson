import { describe, it, expect, beforeEach, vi } from 'vitest'
import { TradeHttp } from '../src/adapters/http/TradeHttp'

describe('TradeHttp adapter', () => {
  beforeEach(() => {
    global.fetch = vi.fn(async () => ({ ok: true, json: async () => ([{ id: 1, name: 'S1' }]) }))
  })

  it('calls strategies endpoint with auth header when token provided', async () => {
    const adapter = new TradeHttp({ baseUrl: 'http://backend.local', getAuthToken: () => 'abc' })
    const result = await adapter.getStrategies()
    expect(result).toEqual([{ id: 1, name: 'S1' }])
    expect(global.fetch).toHaveBeenCalledWith('http://backend.local/api/strategies/', expect.objectContaining({
      method: 'GET',
      headers: expect.objectContaining({ 'Authorization': 'Bearer abc' })
    }))
  })

  it('omits auth header when token not provided', async () => {
    const adapter = new TradeHttp({ baseUrl: 'http://backend.local' })
    await adapter.getStrategies()
    const callArgs = global.fetch.mock.calls[0][1]
    expect(callArgs.headers.Authorization).toBeUndefined()
  })
})


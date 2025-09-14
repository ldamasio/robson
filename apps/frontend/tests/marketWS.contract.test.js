import { describe, it, expect, beforeEach, vi } from 'vitest'
import { MarketWS } from '../src/adapters/ws/MarketWS'

class FakeWS {
  constructor(url) {
    this.url = url
    FakeWS.instances.push(this)
    // open immediately (sync) for deterministic test
    this.onopen && this.onopen()
  }
  send(data) { this.lastSent = data }
  close() { this.closed = true }
  // test helper
  emit(msg) { this.onmessage && this.onmessage({ data: JSON.stringify(msg) }) }
}
FakeWS.instances = []

describe('MarketWS adapter', () => {
  beforeEach(() => {
    FakeWS.instances = []
    global.WebSocket = FakeWS
  })

  it('subscribes and forwards tick messages for the pair', async () => {
    const adapter = new MarketWS({ url: 'ws://test.local/ws' })
    let received = []
    const unsubscribe = adapter.subscribe('BTCUSDT', (tick) => received.push(tick))

    // simulate server tick
    const ws = FakeWS.instances[0]
    ws.emit({ type: 'tick', pair: 'BTCUSDT', bid: '1', ask: '2' })

    // assertions
    expect(ws.url).toBe('ws://test.local/ws')
    expect(ws.lastSent).toBe(JSON.stringify({ type: 'sub', pair: 'BTCUSDT' }))
    expect(received.length).toBe(1)
    expect(received[0]).toEqual({ bid: '1', ask: '2', pair: 'BTCUSDT' })

    // unsubscribe should close
    unsubscribe()
    expect(ws.closed).toBe(true)
  })
})

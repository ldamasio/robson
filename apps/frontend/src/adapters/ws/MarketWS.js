import { MarketDataWS } from '../../ports/MarketDataWS'

export class MarketWS extends MarketDataWS {
  constructor({ url } = {}) {
    super()
    this.url = url || (import.meta?.env?.VITE_WS_URL || 'ws://localhost:8000/ws')
  }

  subscribe(pair, onTick) {
    const ws = new WebSocket(this.url)
    ws.onopen = () => {
      try {
        ws.send(JSON.stringify({ type: 'sub', pair }))
      } catch (_) {}
    }
    ws.onmessage = (ev) => {
      try {
        const msg = typeof ev.data === 'string' ? JSON.parse(ev.data) : ev.data
        if (msg && msg.type === 'tick' && (!msg.pair || msg.pair === pair)) {
          onTick({ bid: msg.bid, ask: msg.ask, pair: msg.pair || pair })
        }
      } catch (_) {}
    }
    ws.onerror = () => {}
    return () => {
      try { ws.close() } catch (_) {}
    }
  }
}


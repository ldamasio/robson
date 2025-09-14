// Domain types (documentation-level in JS)
// Symbol: { base: string, quote: string }
// Order: { id: string, symbol: string, side: 'BUY'|'SELL', qty: string, price: string }

export const pair = (symbol) => `${symbol.base}${symbol.quote}`.toUpperCase();


# Evidence: Binance exchangeInfo extracts for BTCUSDT

Retrieved 2026-11-04T08:03:59Z by `curl` from the public endpoints below; raw responses hashed with SHA-256. Extracts are verbatim JSON.

## Spot

Source: `https://api.binance.com/api/v3/exchangeInfo?symbol=BTCUSDT`  
Raw response SHA-256: `b0e47fe8c52a4e1241d1d4a5051955350ac7fa7846c811580430b7752c5c15d5`  
Server time field: `1793779438676`

```json
{
  "symbol": "BTCUSDT",
  "status": "TRADING",
  "orderTypes": [
    "LIMIT",
    "LIMIT_MAKER",
    "MARKET",
    "STOP_LOSS",
    "STOP_LOSS_LIMIT",
    "TAKE_PROFIT",
    "TAKE_PROFIT_LIMIT"
  ],
  "isSpotTradingAllowed": true,
  "filters": [
    {
      "filterType": "LOT_SIZE",
      "minQty": "0.00001000",
      "maxQty": "9000.00000000",
      "stepSize": "0.00001000"
    },
    {
      "filterType": "MARKET_LOT_SIZE",
      "minQty": "0.00000000",
      "maxQty": "121.13273245",
      "stepSize": "0.00000000"
    },
    {
      "filterType": "NOTIONAL",
      "minNotional": "5.00000000",
      "applyMinToMarket": true,
      "maxNotional": "9000000.00000000",
      "applyMaxToMarket": false,
      "avgPriceMins": 5
    },
    {
      "filterType": "PRICE_FILTER",
      "minPrice": "0.01000000",
      "maxPrice": "1000000.00000000",
      "tickSize": "0.01000000"
    }
  ]
}
```

## USDⓈ-M futures

Source: `https://fapi.binance.com/fapi/v1/exchangeInfo` (symbol BTCUSDT selected locally)  
Raw response SHA-256: `0a1cb6e480b443d4939b572d91c40a2a202aeb48cdb044b5e8a349780f2868ec`  
Server time field: `1793754061895`

```json
{
  "symbol": "BTCUSDT",
  "status": "TRADING",
  "contractType": "PERPETUAL",
  "orderTypes": [
    "LIMIT",
    "MARKET",
    "STOP",
    "STOP_MARKET",
    "TAKE_PROFIT",
    "TAKE_PROFIT_MARKET",
    "TRAILING_STOP_MARKET"
  ],
  "filters": [
    {
      "minQty": "0.001",
      "maxQty": "1000",
      "filterType": "LOT_SIZE",
      "stepSize": "0.001"
    },
    {
      "filterType": "MARKET_LOT_SIZE",
      "maxQty": "120",
      "stepSize": "0.001",
      "minQty": "0.001"
    },
    {
      "filterType": "MIN_NOTIONAL",
      "notional": "50"
    },
    {
      "filterType": "PRICE_FILTER",
      "maxPrice": "4529764",
      "minPrice": "556.80",
      "tickSize": "0.10"
    }
  ]
}
```

## Reading

- Spot lot step `0.00001000`, min qty `0.00001000`; notional min `5.00000000` (applyMinToMarket=True, avgPriceMins=5); `STOP_LOSS` in orderTypes: True.
- Futures lot step `0.001`, min qty `0.001`; MIN_NOTIONAL `50`; `STOP_MARKET` in orderTypes: True.
- Symbol availability only; account authorisation for each order type is verified at venue activation (ADR-0056 §2.1).

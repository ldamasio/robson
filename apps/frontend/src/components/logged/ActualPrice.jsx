import React, { useState, useEffect, useRef } from 'react'
import { Badge } from 'react-bootstrap'
import useWebSocket from '../../hooks/useWebSocket'
import LoadingSpinner from '../common/LoadingSpinner'

function ActualPrice() {
  // Use Binance public WebSocket for market data (always available)
  const wsUrl = import.meta.env.VITE_WS_URL_BINANCE || 'wss://stream.binance.com:9443/ws/btcusdt@ticker';
  const { data: priceData, isConnected } = useWebSocket(wsUrl);

  const [direction, setDirection] = useState('neutral')
  const lastPriceRef = useRef(null)

  const formatCurrency = (value) => {
    const number = Number(value)
    if (Number.isNaN(number)) return value || 'N/A'
    return new Intl.NumberFormat('en-US', {
      style: 'currency',
      currency: 'USD',
      minimumFractionDigits: 2
    }).format(number)
  }

  useEffect(() => {
    if (priceData) {
      // Handle both custom format (price) and Binance ticker format (c = last price)
      const currentPrice = priceData.price || priceData.c;
      if (currentPrice) {
        const last = Number(currentPrice);

        if (lastPriceRef.current !== null) {
          if (last > lastPriceRef.current) {
            setDirection('up');
          } else if (last < lastPriceRef.current) {
            setDirection('down');
          }
        }
        lastPriceRef.current = last;
      }
    }
  }, [priceData]);

  if (!isConnected && !priceData) {
    return <LoadingSpinner label="Connecting to market feed..." />
  }

  if (!priceData) {
    return <div>Waiting for market data...</div>
  }

  // Handle both custom format (price) and Binance ticker format (c = last price)
  const lastPrice = priceData.price || priceData.c;
  const trendLabel = direction === 'up' ? '↑' : direction === 'down' ? '↓' : '→'
  const trendVariant = direction === 'up' ? 'success' : direction === 'down' ? 'danger' : 'secondary'

  return (
    <div className="d-flex flex-column gap-2">
      <div className="d-flex align-items-center gap-2">
        <h4 className="mb-0">{formatCurrency(lastPrice)}</h4>
        <Badge bg={trendVariant}>{trendLabel}</Badge>
      </div>
      <div className="text-secondary small">
        Real-time via WebSocket
      </div>
    </div>
  )
}

export default ActualPrice

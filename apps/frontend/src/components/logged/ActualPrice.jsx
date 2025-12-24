import React, { useState, useEffect, useRef } from 'react'
import { Badge } from 'react-bootstrap'
import useWebSocket from '../../hooks/useWebSocket'
import LoadingSpinner from '../common/LoadingSpinner'

function ActualPrice() {
  const wsUrl = import.meta.env.VITE_MARKET_DATA_WS_URL || 'ws://localhost:8080/ws';
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
    if (priceData && priceData.price) {
      const last = Number(priceData.price);

      if (lastPriceRef.current !== null) {
        if (last > lastPriceRef.current) {
          setDirection('up');
        } else if (last < lastPriceRef.current) {
          setDirection('down');
        }
      }
      lastPriceRef.current = last;
    }
  }, [priceData]);

  if (!isConnected && !priceData) {
    return <LoadingSpinner label="Connecting to market feed..." />
  }

  if (!priceData) {
    return <div>Waiting for market data...</div>
  }

  const lastPrice = priceData.price;
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

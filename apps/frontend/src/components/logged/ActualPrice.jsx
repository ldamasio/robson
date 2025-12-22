import React, { useContext, useEffect, useRef, useState } from 'react'
import axios from 'axios'
import { Badge } from 'react-bootstrap'
import { toast } from 'react-toastify'
import AuthContext from '../../context/AuthContext'
import LoadingSpinner from '../common/LoadingSpinner'

function ActualPrice() {
  const { authTokens } = useContext(AuthContext)
  const [price, setPrice] = useState(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)
  const [direction, setDirection] = useState('neutral')
  const lastPriceRef = useRef(null)

  const baseUrl = import.meta.env.VITE_API_BASE_URL || ''

  const formatCurrency = (value) => {
    const number = Number(value)
    if (Number.isNaN(number)) return value || 'N/A'
    return new Intl.NumberFormat('en-US', {
      style: 'currency',
      currency: 'USD',
      minimumFractionDigits: 2
    }).format(number)
  }

  const fetchPrice = async () => {
    try {
      const response = await axios.get(`${baseUrl}/api/market/price/BTCUSDC/`, {
        headers: {
          Authorization: `Bearer ${authTokens?.access}`
        }
      })
      const data = response.data
      const last = Number(data.last)

      if (lastPriceRef.current !== null) {
        if (last > lastPriceRef.current) {
          setDirection('up')
        } else if (last < lastPriceRef.current) {
          setDirection('down')
        } else {
          setDirection('neutral')
        }
      }

      lastPriceRef.current = last
      setPrice(data)
      setError(null)
    } catch (err) {
      setError('Failed to load price.')
      toast.error('Failed to load current price.')
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    let isActive = true
    const loadPrice = async () => {
      if (!isActive) return
      await fetchPrice()
    }

    loadPrice()
    const interval = setInterval(loadPrice, 1000)
    return () => {
      isActive = false
      clearInterval(interval)
    }
  }, [authTokens?.access])

  if (loading) {
    return <LoadingSpinner label="Loading price..." />
  }

  if (error) {
    return <div className="text-danger">{error}</div>
  }

  if (!price) {
    return <div>No price data available.</div>
  }

  const bid = Number(price.bid)
  const ask = Number(price.ask)
  const spread = Number.isNaN(bid) || Number.isNaN(ask) ? null : ask - bid
  const spreadPercent = spread !== null && price.last ? (spread / Number(price.last)) * 100 : null
  const trendLabel = direction === 'up' ? '↑' : direction === 'down' ? '↓' : '→'
  const trendVariant = direction === 'up' ? 'success' : direction === 'down' ? 'danger' : 'secondary'

  return (
    <div className="d-flex flex-column gap-2">
      <div className="d-flex align-items-center gap-2">
        <h4 className="mb-0">{formatCurrency(price.last)}</h4>
        <Badge bg={trendVariant}>{trendLabel}</Badge>
      </div>
      <div>Bid: {formatCurrency(price.bid)}</div>
      <div>Ask: {formatCurrency(price.ask)}</div>
      <div>
        Spread: {spread !== null ? formatCurrency(spread) : 'N/A'}{' '}
        {spreadPercent !== null && !Number.isNaN(spreadPercent) ? `(${spreadPercent.toFixed(3)}%)` : ''}
      </div>
    </div>
  )
}

export default ActualPrice

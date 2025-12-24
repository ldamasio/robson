import React, { useContext, useEffect, useState } from 'react'
import { Badge, Card } from 'react-bootstrap'
import axios from 'axios'
import { toast } from 'react-toastify'
import AuthContext from '../../context/AuthContext'
import LoadingSpinner from '../common/LoadingSpinner'

function Position() {
  const { authTokens } = useContext(AuthContext)
  const [positions, setPositions] = useState([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)

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

  const formatPercent = (value) => {
    const number = Number(value)
    if (Number.isNaN(number)) return value || 'N/A'
    const sign = number > 0 ? '+' : ''
    return `${sign}${number.toFixed(2)}%`
  }

  const fetchPositions = async () => {
    try {
      const response = await axios.get(`${baseUrl}/api/portfolio/positions/`, {
        headers: {
          Authorization: `Bearer ${authTokens?.access}`
        }
      })
      setPositions(response.data.positions || [])
      setError(null)
    } catch (err) {
      setError('Failed to load positions.')
      toast.error('Failed to load positions.')
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    let isActive = true
    const loadPositions = async () => {
      if (!isActive) return
      await fetchPositions()
    }

    loadPositions()
    const interval = setInterval(loadPositions, 5000)
    return () => {
      isActive = false
      clearInterval(interval)
    }
  }, [authTokens?.access])

  if (loading) {
    return <LoadingSpinner label="Loading positions..." />
  }

  if (error) {
    return <div className="text-danger">{error}</div>
  }

  if (positions.length === 0) {
    return <div>No active positions.</div>
  }

  return (
    <div className="d-grid gap-3">
      {positions.map((position) => {
        const pnl = Number(position.unrealized_pnl)
        const pnlPercent = Number(position.unrealized_pnl_percent)
        const pnlPositive = pnl > 0
        const pnlBadge = pnlPositive ? 'success' : pnl < 0 ? 'danger' : 'secondary'
        const sideLabel = position.side === 'BUY' ? 'LONG' : 'SHORT'
        const key = position.operation_id || position.id || position.symbol

        return (
          <Card key={key} className="card-premium mb-3">
            <Card.Body>
              <div className="d-flex justify-content-between align-items-center mb-3">
                <div>
                  <h5 className="mb-1 text-light fw-bold">{position.symbol}</h5>
                  <small className="text-secondary">Side: <span className={position.side === 'BUY' ? 'text-success' : 'text-danger'}>{sideLabel}</span></small>
                </div>
                <Badge bg={pnlBadge}>
                  {formatCurrency(position.unrealized_pnl)} ({formatPercent(position.unrealized_pnl_percent)})
                </Badge>
              </div>
              <div className="d-grid gap-2">
                <div>Quantity: {position.quantity}</div>
                <div>Entry: {formatCurrency(position.entry_price)}</div>
                <div>
                  Current: {formatCurrency(position.current_price)} ({formatPercent(position.unrealized_pnl_percent)})
                </div>
                <div>
                  Stop: {position.stop_loss ? formatCurrency(position.stop_loss) : 'N/A'}{' '}
                  {position.distance_to_stop_percent ? `(${position.distance_to_stop_percent}% away)` : ''}
                </div>
                <div>
                  Target: {position.take_profit ? formatCurrency(position.take_profit) : 'N/A'}{' '}
                  {position.distance_to_target_percent ? `(${position.distance_to_target_percent}% to go)` : ''}
                </div>
              </div>
            </Card.Body>
          </Card>
        )
      })}
    </div>
  )
}

export default Position

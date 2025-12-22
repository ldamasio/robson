import React, { useContext, useEffect, useMemo, useState } from 'react'
import axios from 'axios'
import { Card } from 'react-bootstrap'
import { toast } from 'react-toastify'
import {
  CartesianGrid,
  ComposedChart,
  Customized,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis
} from 'recharts'
import AuthContext from '../../context/AuthContext'
import LoadingSpinner from '../common/LoadingSpinner'

const CandleLayer = ({ data, xAxisMap, yAxisMap, offset }) => {
  const xAxis = Object.values(xAxisMap || {})[0]
  const yAxis = Object.values(yAxisMap || {})[0]
  if (!xAxis || !yAxis) return null

  const xScale = xAxis.scale
  const yScale = yAxis.scale
  if (!xScale || !yScale) return null

  const bandWidth = typeof xScale.bandwidth === 'function' ? xScale.bandwidth() : 8
  const candleWidth = Math.max(4, bandWidth * 0.6)
  const offsetX = offset?.left || 0
  const offsetY = offset?.top || 0

  return (
    <g>
      {data.map((entry) => {
        const x = xScale(entry.label)
        if (x === undefined || x === null) return null

        const centerX = offsetX + x + bandWidth / 2
        const yHigh = offsetY + yScale(entry.high)
        const yLow = offsetY + yScale(entry.low)
        const yOpen = offsetY + yScale(entry.open)
        const yClose = offsetY + yScale(entry.close)

        const candleTop = Math.min(yOpen, yClose)
        const candleHeight = Math.max(Math.abs(yClose - yOpen), 1)
        const fill = entry.close >= entry.open ? '#2e7d32' : '#c62828'

        return (
          <g key={entry.label}>
            <line x1={centerX} x2={centerX} y1={yHigh} y2={yLow} stroke={fill} strokeWidth={1} />
            <rect x={centerX - candleWidth / 2} y={candleTop} width={candleWidth} height={candleHeight} fill={fill} />
          </g>
        )
      })}
    </g>
  )
}

function Chart() {
  const { authTokens } = useContext(AuthContext)
  const [candles, setCandles] = useState([])
  const [levels, setLevels] = useState({ entry: null, stop: null, target: null })
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)

  const baseUrl = import.meta.env.VITE_API_BASE_URL || ''

  const parseCandles = (records) => {
    return records.slice(-100).map((item) => {
      const time = new Date(item.Date)
      const label = time.toLocaleString('en-US', {
        month: '2-digit',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit'
      })
      return {
        label,
        open: Number(item.Open),
        high: Number(item.High),
        low: Number(item.Low),
        close: Number(item.Close)
      }
    })
  }

  const fetchCandles = async () => {
    const response = await axios.get(`${baseUrl}/api/historical-data/`, {
      params: { symbol: 'BTCUSDC', interval: '15m', days: 7 },
      headers: {
        Authorization: `Bearer ${authTokens?.access}`
      }
    })
    const payload = response.data?.data
    const records = typeof payload === 'string' ? JSON.parse(payload) : payload
    if (Array.isArray(records)) {
      setCandles(parseCandles(records))
    } else {
      setCandles([])
    }
  }

  const fetchLevels = async () => {
    const response = await axios.get(`${baseUrl}/api/portfolio/positions/`, {
      headers: {
        Authorization: `Bearer ${authTokens?.access}`
      }
    })
    const first = response.data?.positions?.[0]
    if (first) {
      const entry = Number(first.entry_price)
      const stop = Number(first.stop_loss)
      const target = Number(first.take_profit)
      setLevels({
        entry: Number.isFinite(entry) ? entry : null,
        stop: Number.isFinite(stop) ? stop : null,
        target: Number.isFinite(target) ? target : null
      })
    }
  }

  useEffect(() => {
    let isActive = true
    const loadData = async () => {
      try {
        await Promise.all([fetchCandles(), fetchLevels()])
        if (isActive) {
          setError(null)
        }
      } catch (err) {
        if (isActive) {
          setError('Failed to load chart data.')
          toast.error('Failed to load chart data.')
        }
      } finally {
        if (isActive) {
          setLoading(false)
        }
      }
    }

    loadData()
    const interval = setInterval(() => {
      loadData()
    }, 15 * 60 * 1000)

    return () => {
      isActive = false
      clearInterval(interval)
    }
  }, [authTokens?.access])

  const yDomain = useMemo(() => {
    if (candles.length === 0) return ['auto', 'auto']
    const lows = candles.map((c) => c.low)
    const highs = candles.map((c) => c.high)
    const values = [...lows, ...highs]
    if (levels.entry) values.push(levels.entry)
    if (levels.stop) values.push(levels.stop)
    if (levels.target) values.push(levels.target)
    const min = Math.min(...values)
    const max = Math.max(...values)
    const padding = (max - min) * 0.05
    return [min - padding, max + padding]
  }, [candles, levels])

  if (loading) {
    return <LoadingSpinner label="Loading chart..." />
  }

  if (error) {
    return <div className="text-danger">{error}</div>
  }

  if (candles.length === 0) {
    return <div>No chart data available.</div>
  }

  return (
    <Card className="shadow-sm">
      <Card.Body>
        <ResponsiveContainer width="100%" height={320}>
          <ComposedChart data={candles} margin={{ top: 20, right: 30, left: 0, bottom: 20 }}>
            <CartesianGrid strokeDasharray="3 3" />
            <XAxis dataKey="label" tick={{ fontSize: 10 }} interval={9} />
            <YAxis domain={yDomain} tick={{ fontSize: 10 }} />
            <Tooltip />
            {levels.entry && <ReferenceLine y={levels.entry} stroke="#1976d2" strokeDasharray="4 4" label="Entry" />}
            {levels.stop && <ReferenceLine y={levels.stop} stroke="#d32f2f" strokeDasharray="4 4" label="Stop" />}
            {levels.target && <ReferenceLine y={levels.target} stroke="#2e7d32" strokeDasharray="4 4" label="Target" />}
            <Customized component={<CandleLayer data={candles} />} />
          </ComposedChart>
        </ResponsiveContainer>
      </Card.Body>
    </Card>
  )
}

export default Chart

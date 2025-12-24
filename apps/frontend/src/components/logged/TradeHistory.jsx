import React, { useContext, useEffect, useState } from 'react'
import { Badge, Table, Form, InputGroup } from 'react-bootstrap'
import axios from 'axios'
import { toast } from 'react-toastify'
import AuthContext from '../../context/AuthContext'
import LoadingSpinner from '../common/LoadingSpinner'

function TradeHistory() {
    const { authTokens } = useContext(AuthContext)
    const [trades, setTrades] = useState([])
    const [loading, setLoading] = useState(true)
    const [error, setError] = useState(null)
    const [filterSymbol, setFilterSymbol] = useState('')

    const baseUrl = import.meta.env.VITE_API_BASE_URL || ''

    const fetchHistory = async () => {
        try {
            setLoading(true)
            const params = {}
            if (filterSymbol) params.symbol = filterSymbol

            const response = await axios.get(`${baseUrl}/api/trade/history/`, {
                headers: {
                    Authorization: `Bearer ${authTokens?.access}`
                },
                params
            })

            setTrades(response.data.trades || [])
            setError(null)
        } catch (err) {
            console.error(err)
            setError('Failed to load trade history.')
            // Quiet fail or toast? Toast is better for user feedback on manual refresh
        } finally {
            setLoading(false)
        }
    }

    useEffect(() => {
        fetchHistory()
    }, [authTokens?.access]) // Initial load

    const handleSearch = (e) => {
        e.preventDefault()
        fetchHistory()
    }

    const formatCurrency = (value) => {
        const number = Number(value)
        if (Number.isNaN(number)) return value || '-'
        return new Intl.NumberFormat('en-US', {
            style: 'currency',
            currency: 'USD',
            minimumFractionDigits: 2
        }).format(number)
    }

    const formatDate = (isoString) => {
        if (!isoString) return '-'
        return new Date(isoString).toLocaleString()
    }

    return (
        <div className="card-premium p-4">
            <div className="d-flex justify-content-between align-items-center mb-4">
                <h4 className="mb-0 text-gradient">Trade History</h4>
                <Form onSubmit={handleSearch} className="d-flex gap-2">
                    <Form.Control
                        type="text"
                        placeholder="Filter Symbol (e.g. BTCUSDC)"
                        value={filterSymbol}
                        onChange={(e) => setFilterSymbol(e.target.value)}
                        className="bg-dark text-light border-secondary"
                    />
                    <button className="btn btn-outline-primary" type="submit">
                        Refresh
                    </button>
                </Form>
            </div>

            {loading && <LoadingSpinner label="Loading history..." />}

            {error && <div className="text-danger mb-3">{error}</div>}

            {!loading && !error && trades.length === 0 && (
                <div className="text-muted text-center py-4">No trade history found.</div>
            )}

            {!loading && !error && trades.length > 0 && (
                <div className="table-responsive">
                    <Table hover variant="dark" className="align-middle mb-0">
                        <thead>
                            <tr className="text-secondary">
                                <th>Date</th>
                                <th>Symbol</th>
                                <th>Side</th>
                                <th className="text-end">Px Entry</th>
                                <th className="text-end">Px Exit</th>
                                <th className="text-end">Qty</th>
                                <th className="text-end">PnL</th>
                                <th className="text-center">Status</th>
                            </tr>
                        </thead>
                        <tbody>
                            {trades.map((trade) => {
                                const isWin = trade.is_winner
                                const pnlColor = isWin === true ? 'text-success' : isWin === false ? 'text-danger' : 'text-light'

                                return (
                                    <tr key={trade.id}>
                                        <td className="small text-muted">{formatDate(trade.entry_time)}</td>
                                        <td className="fw-bold">{trade.symbol}</td>
                                        <td>
                                            <Badge bg={trade.side === 'BUY' ? 'success' : 'danger'}>
                                                {trade.side}
                                            </Badge>
                                        </td>
                                        <td className="text-end">{formatCurrency(trade.entry_price)}</td>
                                        <td className="text-end">{formatCurrency(trade.exit_price)}</td>
                                        <td className="text-end">{Number(trade.quantity).toFixed(5)}</td>
                                        <td className={`text-end fw-bold ${pnlColor}`}>
                                            {trade.pnl ? formatCurrency(trade.pnl) : '-'}
                                            {trade.pnl_percentage ? <small className="d-block opacity-75">{trade.pnl_percentage}%</small> : ''}
                                        </td>
                                        <td className="text-center">
                                            <Badge bg={trade.is_closed ? 'info' : 'success'}>
                                                {trade.is_closed ? 'CLOSED' : 'FILLED'}
                                            </Badge>
                                        </td>
                                    </tr>
                                )
                            })}
                        </tbody>
                    </Table>
                </div>
            )}
        </div>
    )
}

export default TradeHistory

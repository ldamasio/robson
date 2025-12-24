import React, { useContext, useEffect, useState } from 'react'
import { Badge, Table, Button, Alert, Spinner, OverlayTrigger, Tooltip } from 'react-bootstrap'
import axios from 'axios'
import AuthContext from '../../context/AuthContext'
import LoadingSpinner from '../common/LoadingSpinner'

/**
 * Activity History - Complete Audit Trail
 * 
 * Shows ALL account activity for complete transparency:
 * - Spot trades (buy/sell)
 * - Margin trades (leveraged buy/sell)
 * - Transfers (spot <-> margin)
 * - Stop-loss orders
 * - Borrows and repayments
 * 
 * This is the "transparency mode" - users can see EVERYTHING.
 */
function ActivityHistory() {
    const { authTokens } = useContext(AuthContext)
    const [activities, setActivities] = useState([])
    const [loading, setLoading] = useState(true)
    const [syncing, setSyncing] = useState(false)
    const [error, setError] = useState(null)
    const [syncResult, setSyncResult] = useState(null)

    const baseUrl = import.meta.env.VITE_API_BASE_URL || ''

    const fetchActivities = async () => {
        try {
            setLoading(true)
            const response = await axios.get(`${baseUrl}/api/audit/activity/`, {
                headers: {
                    Authorization: `Bearer ${authTokens?.access}`
                }
            })

            setActivities(response.data.activities || [])
            setError(null)
        } catch (err) {
            console.error('Failed to load activities:', err)
            setError('Failed to load activity history. Please try again.')
        } finally {
            setLoading(false)
        }
    }

    const syncFromBinance = async () => {
        try {
            setSyncing(true)
            setSyncResult(null)
            
            const response = await axios.post(
                `${baseUrl}/api/audit/sync/`,
                { days: 30, snapshot: true },
                {
                    headers: {
                        Authorization: `Bearer ${authTokens?.access}`,
                        'Content-Type': 'application/json'
                    }
                }
            )

            setSyncResult({
                success: true,
                message: `Synced ${response.data.synced_count} new transactions`
            })
            
            // Refresh activities after sync
            await fetchActivities()
        } catch (err) {
            console.error('Failed to sync:', err)
            setSyncResult({
                success: false,
                message: 'Failed to sync from Binance. Try again later.'
            })
        } finally {
            setSyncing(false)
        }
    }

    useEffect(() => {
        fetchActivities()
    }, [authTokens?.access])

    const formatDate = (isoString) => {
        if (!isoString) return '-'
        return new Date(isoString).toLocaleString()
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

    const getActivityIcon = (type, subtype) => {
        if (type === 'margin_position') return '📊'
        if (type === 'margin_transfer') return '↔️'
        if (type === 'trade') return '💹'
        if (type === 'order') return '📝'
        if (type === 'audit_transaction') {
            if (subtype?.includes('STOP')) return '🛑'
            if (subtype?.includes('BUY')) return '🟢'
            if (subtype?.includes('SELL')) return '🔴'
            if (subtype?.includes('TRANSFER')) return '↔️'
            if (subtype?.includes('BORROW')) return '💰'
        }
        return '📋'
    }

    const getActivityBadge = (type, subtype, status) => {
        // Determine badge color based on type
        let bg = 'secondary'
        
        if (subtype?.includes('BUY')) bg = 'success'
        else if (subtype?.includes('SELL')) bg = 'danger'
        else if (subtype?.includes('TRANSFER')) bg = 'info'
        else if (subtype?.includes('BORROW')) bg = 'warning'
        else if (subtype?.includes('STOP')) bg = 'danger'
        else if (type === 'margin_position') bg = 'primary'
        
        const label = subtype?.replace('_', ' ') || type.replace('_', ' ')
        
        return (
            <Badge bg={bg} className="text-uppercase" style={{ fontSize: '0.7rem' }}>
                {label}
            </Badge>
        )
    }

    const getStatusBadge = (status) => {
        let bg = 'secondary'
        if (status === 'FILLED' || status === 'COMPLETED' || status === 'OPEN') bg = 'success'
        else if (status === 'PENDING') bg = 'warning'
        else if (status === 'FAILED' || status === 'CANCELLED') bg = 'danger'
        
        return <Badge bg={bg}>{status}</Badge>
    }

    return (
        <div className="card-premium p-4">
            <div className="d-flex justify-content-between align-items-center mb-4">
                <div>
                    <h4 className="mb-1 text-gradient">📋 Complete Activity History</h4>
                    <small className="text-muted">
                        Full transparency - every operation is recorded
                    </small>
                </div>
                <div className="d-flex gap-2">
                    <Button 
                        variant="outline-info" 
                        size="sm"
                        onClick={syncFromBinance}
                        disabled={syncing}
                    >
                        {syncing ? (
                            <>
                                <Spinner size="sm" animation="border" className="me-2" />
                                Syncing...
                            </>
                        ) : (
                            '🔄 Sync from Binance'
                        )}
                    </Button>
                    <Button 
                        variant="outline-primary" 
                        size="sm"
                        onClick={fetchActivities}
                        disabled={loading}
                    >
                        Refresh
                    </Button>
                </div>
            </div>

            {syncResult && (
                <Alert 
                    variant={syncResult.success ? 'success' : 'danger'} 
                    dismissible 
                    onClose={() => setSyncResult(null)}
                    className="mb-3"
                >
                    {syncResult.message}
                </Alert>
            )}

            {loading && <LoadingSpinner label="Loading activity history..." />}

            {error && <Alert variant="danger">{error}</Alert>}

            {!loading && !error && activities.length === 0 && (
                <Alert variant="info">
                    <Alert.Heading>No activity recorded yet</Alert.Heading>
                    <p className="mb-0">
                        Transactions will appear here as you trade. Click "Sync from Binance" 
                        to import any missing transactions from your exchange account.
                    </p>
                </Alert>
            )}

            {!loading && !error && activities.length > 0 && (
                <>
                    <div className="mb-3">
                        <small className="text-muted">
                            Showing {activities.length} activities
                        </small>
                    </div>
                    
                    <div className="table-responsive">
                        <Table hover variant="dark" className="align-middle mb-0" size="sm">
                            <thead>
                                <tr className="text-secondary">
                                    <th style={{width: '30px'}}></th>
                                    <th>Time</th>
                                    <th>Type</th>
                                    <th>Description</th>
                                    <th>Symbol</th>
                                    <th className="text-end">Qty</th>
                                    <th className="text-end">Price</th>
                                    <th className="text-center">Status</th>
                                    <th>Binance ID</th>
                                </tr>
                            </thead>
                            <tbody>
                                {activities.map((activity, index) => (
                                    <tr key={`${activity.type}-${activity.timestamp}-${index}`}>
                                        <td>
                                            {getActivityIcon(activity.type, activity.subtype)}
                                        </td>
                                        <td className="small text-muted" style={{whiteSpace: 'nowrap'}}>
                                            {formatDate(activity.timestamp)}
                                        </td>
                                        <td>
                                            {getActivityBadge(activity.type, activity.subtype, activity.status)}
                                        </td>
                                        <td className="small">
                                            {activity.description}
                                            {activity.extra && (
                                                <OverlayTrigger
                                                    placement="top"
                                                    overlay={
                                                        <Tooltip>
                                                            <div className="text-start">
                                                                {activity.extra.leverage && (
                                                                    <div>Leverage: {activity.extra.leverage}x</div>
                                                                )}
                                                                {activity.extra.stop_price && (
                                                                    <div>Stop: ${activity.extra.stop_price}</div>
                                                                )}
                                                                {activity.extra.risk_amount && (
                                                                    <div>Risk: ${activity.extra.risk_amount}</div>
                                                                )}
                                                            </div>
                                                        </Tooltip>
                                                    }
                                                >
                                                    <span className="ms-1 text-info" style={{cursor: 'help'}}>ℹ️</span>
                                                </OverlayTrigger>
                                            )}
                                        </td>
                                        <td className="fw-bold small">{activity.symbol}</td>
                                        <td className="text-end small">
                                            {Number(activity.quantity).toFixed(8)}
                                        </td>
                                        <td className="text-end small">
                                            {activity.price ? formatCurrency(activity.price) : '-'}
                                        </td>
                                        <td className="text-center">
                                            {getStatusBadge(activity.status)}
                                        </td>
                                        <td className="small text-muted">
                                            {activity.binance_id ? (
                                                <OverlayTrigger
                                                    placement="top"
                                                    overlay={<Tooltip>{activity.binance_id}</Tooltip>}
                                                >
                                                    <span style={{cursor: 'help'}}>
                                                        {activity.binance_id.substring(0, 8)}...
                                                    </span>
                                                </OverlayTrigger>
                                            ) : '-'}
                                        </td>
                                    </tr>
                                ))}
                            </tbody>
                        </Table>
                    </div>
                </>
            )}

            <style jsx="true">{`
                .text-gradient {
                    background: linear-gradient(135deg, #00d4ff, #9b59b6);
                    -webkit-background-clip: text;
                    -webkit-text-fill-color: transparent;
                    background-clip: text;
                }
            `}</style>
        </div>
    )
}

export default ActivityHistory


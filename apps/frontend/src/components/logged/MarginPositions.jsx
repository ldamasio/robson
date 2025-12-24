import React, { useState, useEffect, useContext } from 'react';
import { Card, Table, Badge, Button, Spinner, Alert, ButtonGroup } from 'react-bootstrap';
import AuthContext from '../../context/AuthContext';
import { MarginHttp } from '../../adapters/http/MarginHttp';
import MarginPositionCard from './MarginPositionCard';

/**
 * Margin Positions List
 * 
 * Displays all margin positions with their current status,
 * P&L, and margin health.
 */
function MarginPositions() {
  const { authTokens } = useContext(AuthContext);
  const [positions, setPositions] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [filter, setFilter] = useState('OPEN');
  const [monitorData, setMonitorData] = useState(null);

  const marginService = new MarginHttp({
    baseUrl: import.meta.env.VITE_API_BASE_URL || '',
    getAuthToken: () => authTokens?.access,
  });

  const loadPositions = async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await marginService.listPositions(
        filter !== 'ALL' ? { status: filter } : {}
      );
      setPositions(data.positions || []);
    } catch (err) {
      setError(err.message);
    } finally {
      setLoading(false);
    }
  };

  const monitorMargins = async () => {
    try {
      const data = await marginService.monitorMargins();
      setMonitorData(data);
    } catch (err) {
      console.error('Monitor failed:', err);
    }
  };

  useEffect(() => {
    loadPositions();
  }, [filter]);

  const getStatusBadge = (status) => {
    const variants = {
      'PENDING': 'secondary',
      'OPEN': 'primary',
      'CLOSING': 'warning',
      'CLOSED': 'success',
      'STOPPED_OUT': 'danger',
      'TAKE_PROFIT': 'success',
      'LIQUIDATED': 'dark',
    };
    return <Badge bg={variants[status] || 'secondary'}>{status}</Badge>;
  };

  const getHealthBadge = (health) => {
    const variants = {
      'SAFE': 'success',
      'CAUTION': 'info',
      'WARNING': 'warning',
      'CRITICAL': 'danger',
      'DANGER': 'dark',
    };
    const icons = {
      'SAFE': '✅',
      'CAUTION': '⚠️',
      'WARNING': '🔶',
      'CRITICAL': '🚨',
      'DANGER': '💀',
    };
    return (
      <Badge bg={variants[health] || 'secondary'}>
        {icons[health]} {health}
      </Badge>
    );
  };

  const formatPnl = (pnl) => {
    const value = parseFloat(pnl);
    const color = value >= 0 ? '#22c55e' : '#ef4444';
    const prefix = value >= 0 ? '+' : '';
    return (
      <span style={{ color, fontWeight: 600, fontFamily: 'monospace' }}>
        {prefix}${value.toFixed(2)}
      </span>
    );
  };

  return (
    <Card style={{
      background: 'linear-gradient(135deg, #0f172a 0%, #1e293b 100%)',
      border: '1px solid #334155',
      borderRadius: '16px',
    }}>
      <Card.Header style={{
        background: 'transparent',
        borderBottom: '1px solid #334155',
        padding: '1.25rem',
      }}>
        <div className="d-flex align-items-center justify-content-between">
          <div className="d-flex align-items-center gap-2">
            <span style={{ fontSize: '1.5rem' }}>📊</span>
            <div>
              <h5 className="mb-0" style={{ color: '#38bdf8', fontWeight: 600 }}>
                Margin Positions
              </h5>
              <small style={{ color: '#94a3b8' }}>
                Track your isolated margin trades
              </small>
            </div>
          </div>
          <div className="d-flex gap-2">
            <Button
              variant="outline-info"
              size="sm"
              onClick={monitorMargins}
              style={{ borderColor: '#334155' }}
            >
              🔄 Refresh Margins
            </Button>
          </div>
        </div>
      </Card.Header>

      <Card.Body style={{ padding: '1rem' }}>
        {/* Alerts from Monitor */}
        {monitorData?.alerts?.length > 0 && (
          <Alert variant="warning" style={{
            background: 'rgba(234, 179, 8, 0.1)',
            border: '1px solid rgba(234, 179, 8, 0.3)',
            borderRadius: '12px',
          }}>
            <Alert.Heading style={{ fontSize: '1rem' }}>
              ⚠️ Margin Alerts ({monitorData.alerts.length})
            </Alert.Heading>
            {monitorData.alerts.map((alert, idx) => (
              <div key={idx} style={{ color: '#fef08a' }}>
                {alert.symbol}: {alert.message}
              </div>
            ))}
          </Alert>
        )}

        {/* Filter Buttons */}
        <div className="mb-3">
          <ButtonGroup>
            {['OPEN', 'CLOSED', 'STOPPED_OUT', 'ALL'].map(status => (
              <Button
                key={status}
                variant={filter === status ? 'primary' : 'outline-secondary'}
                size="sm"
                onClick={() => setFilter(status)}
                style={{
                  borderColor: '#334155',
                  ...(filter !== status && { color: '#94a3b8' }),
                }}
              >
                {status.replace('_', ' ')}
              </Button>
            ))}
          </ButtonGroup>
        </div>

        {loading && (
          <div className="text-center py-5">
            <Spinner animation="border" variant="primary" />
            <p style={{ color: '#94a3b8', marginTop: '1rem' }}>Loading positions...</p>
          </div>
        )}

        {error && (
          <Alert variant="danger" style={{
            background: 'rgba(239, 68, 68, 0.1)',
            border: '1px solid rgba(239, 68, 68, 0.3)',
            color: '#fca5a5',
            borderRadius: '12px',
          }}>
            {error}
          </Alert>
        )}

        {!loading && !error && positions.length === 0 && (
          <div className="text-center py-5">
            <div style={{ fontSize: '3rem' }}>📭</div>
            <p style={{ color: '#94a3b8' }}>No positions found</p>
          </div>
        )}

        {!loading && !error && positions.length > 0 && filter === 'OPEN' && (
          <div className="mt-4">
            {positions.map((pos, idx) => (
              <MarginPositionCard key={pos.position_id || idx} position={pos} />
            ))}
          </div>
        )}

        {!loading && !error && positions.length > 0 && filter !== 'OPEN' && (
          <div style={{ overflowX: 'auto' }}>
            <Table
              hover
              style={{
                color: '#e2e8f0',
                marginBottom: 0,
              }}
            >
              <thead style={{ borderBottom: '2px solid #334155' }}>
                <tr>
                  <th style={{ color: '#64748b', fontWeight: 500 }}>Symbol</th>
                  <th style={{ color: '#64748b', fontWeight: 500 }}>Side</th>
                  <th style={{ color: '#64748b', fontWeight: 500 }}>Status</th>
                  <th style={{ color: '#64748b', fontWeight: 500 }}>Entry</th>
                  <th style={{ color: '#64748b', fontWeight: 500 }}>Stop</th>
                  <th style={{ color: '#64748b', fontWeight: 500 }}>Quantity</th>
                  <th style={{ color: '#64748b', fontWeight: 500 }}>Leverage</th>
                  <th style={{ color: '#64748b', fontWeight: 500 }}>P&L</th>
                  <th style={{ color: '#64748b', fontWeight: 500 }}>Health</th>
                  <th style={{ color: '#64748b', fontWeight: 500 }}>Opened</th>
                </tr>
              </thead>
              <tbody>
                {positions.map((pos, idx) => (
                  <tr
                    key={pos.position_id || idx}
                    style={{ borderBottom: '1px solid #1e293b' }}
                  >
                    <td style={{ fontWeight: 600 }}>{pos.symbol}</td>
                    <td>
                      <Badge bg={pos.side === 'LONG' ? 'success' : 'danger'}>
                        {pos.side === 'LONG' ? '📈' : '📉'} {pos.side}
                      </Badge>
                    </td>
                    <td>{getStatusBadge(pos.status)}</td>
                    <td style={{ fontFamily: 'monospace' }}>
                      ${parseFloat(pos.entry_price).toFixed(2)}
                    </td>
                    <td style={{ fontFamily: 'monospace', color: '#ef4444' }}>
                      ${parseFloat(pos.stop_price).toFixed(2)}
                    </td>
                    <td style={{ fontFamily: 'monospace' }}>
                      {parseFloat(pos.quantity).toFixed(6)}
                    </td>
                    <td>
                      <Badge bg="warning" text="dark">
                        {pos.leverage}x
                      </Badge>
                    </td>
                    <td>{formatPnl(pos.total_pnl)}</td>
                    <td>{getHealthBadge(pos.margin_health)}</td>
                    <td style={{ color: '#94a3b8', fontSize: '0.85rem' }}>
                      {pos.opened_at
                        ? new Date(pos.opened_at).toLocaleDateString()
                        : '-'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </Table>
          </div>
        )}

        {/* Summary Stats */}
        {monitorData && (
          <div
            className="d-flex justify-content-around mt-3 pt-3"
            style={{ borderTop: '1px solid #334155' }}
          >
            <div className="text-center">
              <div style={{ color: '#64748b', fontSize: '0.85rem' }}>Open Positions</div>
              <div style={{ color: '#38bdf8', fontSize: '1.5rem', fontWeight: 700 }}>
                {monitorData.total_open}
              </div>
            </div>
            <div className="text-center">
              <div style={{ color: '#64748b', fontSize: '0.85rem' }}>At Risk</div>
              <div style={{
                color: monitorData.at_risk > 0 ? '#ef4444' : '#22c55e',
                fontSize: '1.5rem',
                fontWeight: 700
              }}>
                {monitorData.at_risk}
              </div>
            </div>
            <div className="text-center">
              <div style={{ color: '#64748b', fontSize: '0.85rem' }}>Last Update</div>
              <div style={{ color: '#94a3b8', fontSize: '0.9rem' }}>
                {monitorData.timestamp
                  ? new Date(monitorData.timestamp).toLocaleTimeString()
                  : '-'}
              </div>
            </div>
          </div>
        )}
      </Card.Body>
    </Card>
  );
}

export default MarginPositions;


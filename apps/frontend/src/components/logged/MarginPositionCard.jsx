import React from 'react';
import { Card, Badge, Row, Col } from 'react-bootstrap';

/**
 * MarginPositionCard
 * 
 * A high-detail card for active margin positions.
 * Replicates the "ASCII box" technical style requested by the user.
 */
function MarginPositionCard({ position }) {
    const formatCurrency = (val) => new Intl.NumberFormat('en-US', {
        style: 'currency',
        currency: 'USD',
    }).format(val || 0);

    const formatCrypto = (val, symbol) => `${parseFloat(val || 0).toFixed(8)} ${symbol}`;

    const healthVariant = {
        'SAFE': 'success',
        'CAUTION': 'info',
        'WARNING': 'warning',
        'CRITICAL': 'danger',
        'DANGER': 'dark',
    }[position.margin_health] || 'secondary';

    // Calculate approximate USD value for quantity
    const usdValue = position.quantity * position.entry_price;

    return (
        <Card className="card-premium mb-4 bg-glass border-glow shadow-lg overflow-hidden">
            <div className="card-header-gradient p-3 d-flex justify-content-between align-items-center border-bottom border-secondary">
                <h6 className="mb-0 fw-bold tracking-wider text-uppercase">
                    <span className="me-2">📦</span>
                    POSIÇÃO ATIVA - {position.symbol} {position.side}
                </h6>
                <Badge bg={healthVariant} className="px-3 py-2 shadow-sm">
                    {position.margin_health}
                </Badge>
            </div>

            <Card.Body className="p-4 font-monospace">
                <Row className="gy-3">
                    <Col md={6}>
                        <div className="d-flex justify-content-between border-bottom border-secondary border-opacity-25 pb-2">
                            <span className="text-secondary text-uppercase small">Entrada:</span>
                            <span className="text-light fw-bold">{formatCurrency(position.entry_price)}</span>
                        </div>
                    </Col>
                    <Col md={6}>
                        <div className="d-flex justify-content-between border-bottom border-secondary border-opacity-25 pb-2">
                            <span className="text-secondary text-uppercase small">Stop:</span>
                            <span className="text-danger fw-bold">
                                {formatCurrency(position.stop_price)}
                            </span>
                        </div>
                        {position.binance_stop_order_id && (
                            <div className="text-end small opacity-50 mt-1">
                                ID: {position.binance_stop_order_id}
                            </div>
                        )}
                    </Col>

                    <Col md={6}>
                        <div className="d-flex justify-content-between border-bottom border-secondary border-opacity-25 pb-2">
                            <span className="text-secondary text-uppercase small">Qtde:</span>
                            <div className="text-end">
                                <div className="text-light">{formatCrypto(position.quantity, position.symbol.replace('USDC', ''))}</div>
                                <div className="text-secondary small">(~{formatCurrency(usdValue)})</div>
                            </div>
                        </div>
                    </Col>
                    <Col md={6}>
                        <div className="d-flex justify-content-between border-bottom border-secondary border-opacity-25 pb-2">
                            <span className="text-secondary text-uppercase small">Risco:</span>
                            <div className="text-end">
                                <span className="text-warning fw-bold">{formatCurrency(position.risk_amount)}</span>
                                <span className="text-secondary small ms-2">({position.risk_percent}% cap)</span>
                            </div>
                        </div>
                    </Col>

                    <Col md={12}>
                        <div className="d-flex justify-content-between align-items-center bg-dark bg-opacity-50 p-3 rounded-3 border border-secondary border-opacity-25">
                            <div>
                                <span className="text-secondary text-uppercase small d-block mb-1">Margin Status:</span>
                                <span className="fs-4 fw-bold text-gradient">
                                    {parseFloat(position.margin_level).toFixed(2)}x
                                </span>
                                <span className={`ms-2 badge bg-${healthVariant}-soft text-${healthVariant}`}>
                                    ({position.margin_health})
                                </span>
                            </div>
                            <div className="text-end border-start border-secondary border-opacity-25 ps-4">
                                <div className="small text-secondary mb-1">PNL UNREALIZED</div>
                                <div className={`fs-5 fw-bold ${parseFloat(position.unrealized_pnl) >= 0 ? 'text-success' : 'text-danger'}`}>
                                    {parseFloat(position.unrealized_pnl) >= 0 ? '+' : ''}{formatCurrency(position.unrealized_pnl)}
                                </div>
                            </div>
                        </div>
                    </Col>
                </Row>

                <div className="mt-4 pt-3 border-top border-secondary border-opacity-25 d-flex justify-content-between align-items-center text-muted small">
                    <div>
                        <span className="me-3">DB: <span className="text-light">MarginPosition ID: {position.id || position.position_id.substring(0, 8)}</span></span>
                        <span><span className="text-info">{position.transfer_count || 0}</span> MarginTransfers registrados</span>
                    </div>
                    <div>
                        Opened: {new Date(position.opened_at).toLocaleString()}
                    </div>
                </div>
            </Card.Body>

            <style dangerouslySetInnerHTML={{
                __html: `
        .card-header-gradient {
          background: linear-gradient(90deg, rgba(88, 101, 242, 0.1) 0%, rgba(0, 0, 0, 0) 100%);
        }
        .border-glow:hover {
          border-color: rgba(56, 189, 248, 0.5) !important;
          box-shadow: 0 0 20px rgba(56, 189, 248, 0.15) !important;
        }
        .text-gradient {
          background: linear-gradient(135deg, #38bdf8 0%, #818cf8 100%);
          -webkit-background-clip: text;
          -webkit-text-fill-color: transparent;
        }
        .bg-${healthVariant}-soft {
          background-color: rgba(var(--bs-${healthVariant}-rgb), 0.1) !important;
        }
      `}} />
        </Card>
    );
}

export default MarginPositionCard;

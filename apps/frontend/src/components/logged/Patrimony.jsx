import React, { useState, useEffect } from 'react';
import { Card, Row, Col, Badge, Spinner, Alert } from 'react-bootstrap';
import axios from 'axios';

/**
 * Patrimony Component - Portfolio Value in BTC
 *
 * Displays:
 * - Total portfolio value denominated in BTC
 * - Profit/Loss in BTC since inception
 * - Breakdown by account type (spot/margin)
 * - Auto-refreshes every 60 seconds
 *
 * Formula: Profit (BTC) = Current Balance + Withdrawals - Deposits
 */
function Patrimony() {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [portfolioData, setPortfolioData] = useState(null);
  const [profitData, setProfitData] = useState(null);

  useEffect(() => {
    const fetchData = async () => {
      setLoading(true);
      setError(null);
      try {
        const BACKEND_URL = import.meta.env.VITE_API_BASE_URL;

        // Fetch both portfolio total and profit in parallel
        const [totalRes, profitRes] = await Promise.all([
          axios.get(`${BACKEND_URL}/api/portfolio/btc/total/`),
          axios.get(`${BACKEND_URL}/api/portfolio/btc/profit/`),
        ]);

        setPortfolioData(totalRes.data);
        setProfitData(profitRes.data);
      } catch (err) {
        console.error('Failed to fetch portfolio data:', err);
        setError(err.message || 'Failed to load portfolio data');
      } finally {
        setLoading(false);
      }
    };

    fetchData();
    const interval = setInterval(fetchData, 60000); // Update every minute
    return () => clearInterval(interval);
  }, []);

  if (loading) {
    return (
      <Card>
        <Card.Body className="text-center">
          <Spinner animation="border" role="status">
            <span className="visually-hidden">Loading...</span>
          </Spinner>
          <p className="mt-2 text-muted">Loading portfolio data...</p>
        </Card.Body>
      </Card>
    );
  }

  if (error) {
    return (
      <Card className="border-danger">
        <Card.Body className="text-center">
          <Alert variant="danger">
            Error: {error}
          </Alert>
        </Card.Body>
      </Card>
    );
  }

  if (!portfolioData || !profitData) {
    return null;
  }

  // Parse values
  const totalBtc = parseFloat(portfolioData.total_btc);
  const spotBtc = parseFloat(portfolioData.spot_btc);
  const marginBtc = parseFloat(portfolioData.margin_btc);
  const profitBtc = parseFloat(profitData.profit_btc);
  const profitPercent = parseFloat(profitData.profit_percent);
  const totalDeposits = parseFloat(profitData.total_deposits_btc);
  const totalWithdrawals = parseFloat(profitData.total_withdrawals_btc);

  // Determine profit color and styling
  const isProfitable = profitBtc >= 0;
  const profitVariant = isProfitable ? 'success' : 'danger';
  const profitIcon = isProfitable ? '↑' : '↓';
  const profitSign = isProfitable ? '+' : '';

  // Format BTC value to 8 decimal places
  const formatBTC = (value) => value.toFixed(8);

  return (
    <Card className="shadow-sm">
      <Card.Body>
        <Card.Title className="mb-4 d-flex justify-content-between align-items-center">
          <span>Portfolio Value (BTC)</span>
          <Badge bg="primary" className="ms-2">Live</Badge>
        </Card.Title>

        {/* Total Portfolio Value */}
        <Row className="mb-4">
          <Col>
            <div className="text-center">
              <h2 className="mb-0">
                <strong className="text-primary">{formatBTC(totalBtc)} BTC</strong>
              </h2>
              <small className="text-muted">Total Portfolio Value</small>
            </div>
          </Col>
        </Row>

        {/* Profit/Loss Display */}
        <Row className="mb-4">
          <Col className="text-center">
            <h5>
              <Badge bg={profitVariant} className="p-2 fs-6">
                {profitIcon} {profitSign}{formatBTC(profitBtc)} BTC
                ({profitSign}{profitPercent.toFixed(2)}%)
              </Badge>
            </h5>
            <small className="text-muted">
              {isProfitable ? 'Profit' : 'Loss'} Since Inception
            </small>
          </Col>
        </Row>

        {/* Account Breakdown */}
        <Row className="mb-3">
          <Col>
            <div className="p-3 bg-light rounded">
              <h6 className="mb-3">Account Breakdown</h6>
              <div className="d-flex justify-content-between mb-2">
                <span>Spot:</span>
                <strong>{formatBTC(spotBtc)} BTC</strong>
              </div>
              <div className="d-flex justify-content-between mb-2">
                <span>Margin:</span>
                <strong>{formatBTC(marginBtc)} BTC</strong>
              </div>
              <div className="d-flex justify-content-between">
                <span>Margin Debt:</span>
                <strong className="text-danger">
                  -{formatBTC(parseFloat(portfolioData.margin_debt_btc))} BTC
                </strong>
              </div>
            </div>
          </Col>
        </Row>

        {/* Deposits and Withdrawals Summary */}
        <Row>
          <Col>
            <div className="p-3 bg-light rounded">
              <h6 className="mb-3">Transaction Summary</h6>
              <div className="d-flex justify-content-between mb-2">
                <span>Total Deposits:</span>
                <strong className="text-success">+{formatBTC(totalDeposits)} BTC</strong>
              </div>
              <div className="d-flex justify-content-between">
                <span>Total Withdrawals:</span>
                <strong className="text-warning">-{formatBTC(totalWithdrawals)} BTC</strong>
              </div>
              <hr />
              <div className="d-flex justify-content-between">
                <span>Net Inflows:</span>
                <strong>
                  {formatBTC(totalDeposits - totalWithdrawals)} BTC
                </strong>
              </div>
            </div>
          </Col>
        </Row>

        {/* Footer */}
        <div className="text-center mt-4 pt-3 border-top">
          <small className="text-muted">
            Designed by RBX Robótica
          </small>
        </div>
      </Card.Body>
    </Card>
  );
}

export default Patrimony;
